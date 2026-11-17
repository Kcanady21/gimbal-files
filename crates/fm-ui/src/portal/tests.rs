// SPDX-License-Identifier: GPL-3.0-or-later
//! The portal's wire contract, exercised over a peer-to-peer GDBus connection
//! with a fake dialog host. No GTK window is built, so these tests check the
//! service and the option/result helpers, not the picker's widget tree.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use super::picker;
use super::service::{
    self, Call, DialogHost, Method, OpenOptions, RESPONSE_CANCELLED, RESPONSE_OTHER,
    RESPONSE_SUCCESS, Request,
};

/// A scratch directory that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "gimbal-p2am-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|time| time.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One asynchronous reply slot.
#[derive(Clone, Default)]
struct Reply(Rc<RefCell<Option<Result<glib::Variant, String>>>>);

impl Reply {
    fn answered(&self) -> bool {
        self.0.borrow().is_some()
    }

    fn take(&self) -> Option<Result<glib::Variant, String>> {
        self.0.borrow_mut().take()
    }
}

/// A peer-to-peer bus: GDBusServer on a private socket, one client connection,
/// and every accepted connection kept alive (the new-connection handler owns
/// it; dropping the reference closes it).
struct Bus {
    context: glib::MainContext,
    _server: gio::DBusServer,
    _server_connection: Rc<RefCell<Option<gio::DBusConnection>>>,
    client: gio::DBusConnection,
    _scratch: Scratch,
}

impl Bus {
    /// Build a bus and install `service` when the peer connects.
    fn service(service: &Rc<service::Service>, scratch: Scratch) -> Self {
        // The context the test made thread-default: GDBus binds its sources to
        // it, so this is the one to iterate.
        let context = glib::MainContext::ref_thread_default();
        let server = gio::DBusServer::new_sync(
            &format!("unix:tmpdir={}", scratch.path().display()),
            gio::DBusServerFlags::AUTHENTICATION_ALLOW_ANONYMOUS,
            "0123456789abcdef0123456789abcdef",
            None::<&gio::DBusAuthObserver>,
            None::<&gio::Cancellable>,
        )
        .expect("a DBusServer on a private socket");
        let server_connection = Rc::new(RefCell::new(None));
        {
            let kept = server_connection.clone();
            let service = service.clone();
            server.connect_new_connection(move |_, connection| {
                *kept.borrow_mut() = Some(connection.clone());
                service.install(connection);
                true
            });
        }
        server.start();

        let connected: Rc<RefCell<Option<gio::DBusConnection>>> = Rc::new(RefCell::new(None));
        {
            let slot = connected.clone();
            gio::DBusConnection::for_address(
                &server.client_address(),
                gio::DBusConnectionFlags::AUTHENTICATION_CLIENT,
                None::<&gio::DBusAuthObserver>,
                None::<&gio::Cancellable>,
                move |result| {
                    if let Ok(connection) = result {
                        *slot.borrow_mut() = Some(connection);
                    }
                },
            );
        }
        pump_until(&context, Duration::from_secs(5), || {
            connected.borrow().is_some()
        });
        let client = connected.borrow().clone().expect("the client connection");

        Self {
            context,
            _server: server,
            _server_connection: server_connection,
            client,
            _scratch: scratch,
        }
    }

    fn call(
        &self,
        path: &str,
        interface: &str,
        method: &str,
        parameters: Option<glib::Variant>,
    ) -> Reply {
        let reply = Reply::default();
        let slot = reply.0.clone();
        let parameters = parameters.unwrap_or_else(|| ().to_variant());
        self.client.call(
            None,
            path,
            interface,
            method,
            Some(&parameters),
            None,
            gio::DBusCallFlags::NONE,
            2000,
            None::<&gio::Cancellable>,
            move |result| {
                *slot.borrow_mut() = Some(result.map_err(|error| error.to_string()));
            },
        );
        reply
    }

    /// Wait for one reply, failing the test after two seconds.
    fn wait(&self, reply: &Reply) -> Result<glib::Variant, String> {
        pump_until(&self.context, Duration::from_secs(2), || reply.answered());
        reply
            .take()
            .unwrap_or_else(|| Err("no reply within two seconds".to_string()))
    }

    /// Pump for a moment and report whether the reply has already arrived.
    fn settles_without_reply(&self, reply: &Reply) -> bool {
        pump_for(&self.context, Duration::from_millis(120));
        !reply.answered()
    }
}

fn pump_until(context: &glib::MainContext, limit: Duration, done: impl Fn() -> bool) {
    let deadline = Instant::now() + limit;
    while !done() && Instant::now() < deadline {
        context.iteration(false);
        std::thread::sleep(Duration::from_millis(1));
    }
    // One more turn so a callback scheduled by the last message runs.
    if done() {
        context.iteration(false);
    }
}

fn pump_for(context: &glib::MainContext, period: Duration) {
    let deadline = Instant::now() + period;
    while Instant::now() < deadline {
        context.iteration(false);
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// A dialog host that records the requests and never answers on its own, so a
/// test can answer them (or fail to).
#[derive(Default)]
struct FakeHost {
    requests: RefCell<Vec<Rc<Request>>>,
    /// The `current_folder` each call carried, as the service parsed it.
    folders: RefCell<Vec<Option<PathBuf>>>,
    fail: Cell<bool>,
}

impl DialogHost for FakeHost {
    fn open(&self, request: Rc<Request>, call: &Call) -> Result<(), String> {
        self.folders
            .borrow_mut()
            .push(call.options.current_folder.clone());
        if self.fail.get() {
            return Err("no display in this test".to_string());
        }
        self.requests.borrow_mut().push(request);
        Ok(())
    }
}

fn options(entries: &[(&str, glib::Variant)]) -> HashMap<String, glib::Variant> {
    entries
        .iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect()
}

fn nul_path(path: &std::path::Path) -> Vec<u8> {
    let mut bytes = path.to_string_lossy().into_owned().into_bytes();
    bytes.push(0);
    bytes
}

const HANDLE: &str = "/org/freedesktop/portal/desktop/request/1_42/t1";

fn open_file(bus: &Bus, app_id: &str, title: &str, extra: HashMap<String, glib::Variant>) -> Reply {
    bus.call(
        service::OBJECT_PATH,
        service::INTERFACE,
        "OpenFile",
        Some(
            (
                glib::variant::ObjectPath::try_from(HANDLE).expect("a valid handle"),
                app_id.to_string(),
                String::new(),
                title.to_string(),
                extra,
            )
                .to_variant(),
        ),
    )
}

fn close(bus: &Bus, handle: &str) -> Reply {
    bus.call(
        handle,
        service::REQUEST_INTERFACE,
        "Close",
        Some(().to_variant()),
    )
}

fn reply_parts(reply: &glib::Variant) -> (u32, HashMap<String, glib::Variant>) {
    reply
        .get::<(u32, HashMap<String, glib::Variant>)>()
        .expect("a (ua{sv}) reply")
}

#[test]
fn current_folder_accepts_with_or_without_a_nul_and_keeps_a_non_utf8_name() {
    // The frontend sends `ay` with a trailing NUL; xdp-gnome writes it with
    // one and some callers do not. Both must land on the same folder.
    assert_eq!(
        service::folder_from_bytes(b"/home/kyle/Desktop\0"),
        Some(PathBuf::from("/home/kyle/Desktop"))
    );
    assert_eq!(
        service::folder_from_bytes(b"/home/kyle/Desktop"),
        Some(PathBuf::from("/home/kyle/Desktop"))
    );

    // An empty value and a relative path are not a starting folder.
    assert_eq!(service::folder_from_bytes(b""), None);
    assert_eq!(service::folder_from_bytes(b"\0"), None);
    assert_eq!(service::folder_from_bytes(b"Desktop\0"), None);

    // A non-UTF-8 path is still a PathBuf, not a lossy String.
    let name = b"/tmp/\xff\xfe\0";
    let expected = PathBuf::from(OsString::from_vec(b"/tmp/\xff\xfe".to_vec()));
    assert_eq!(service::folder_from_bytes(name), Some(expected));
}

#[test]
fn a_missing_or_empty_start_folder_falls_back_to_home() {
    let scratch = Scratch::new("start");
    let home = scratch.path().join("home");
    let folder = scratch.path().join("folder");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&folder).expect("folder");
    let file = folder.join("not-a-folder");
    std::fs::write(&file, b"x").expect("file");

    assert_eq!(service::starting_folder(None, &home), home);
    assert_eq!(
        service::starting_folder(Some(&scratch.path().join("gone")), &home),
        home
    );
    assert_eq!(service::starting_folder(Some(&file), &home), home);
    assert_eq!(service::starting_folder(Some(&folder), &home), folder);
}

#[test]
fn parse_open_options_keeps_the_label_and_names_the_ignored_keys() {
    let parsed = service::parse_open_options(&options(&[
        ("accept_label", "_Open".to_variant()),
        ("modal", true.to_variant()),
        ("multiple", true.to_variant()),
        (
            "filters",
            Vec::<(String, Vec<(String, String)>)>::new().to_variant(),
        ),
        ("something_new", 1i32.to_variant()),
    ]));
    assert_eq!(parsed.accept_label.as_deref(), Some("_Open"));
    assert_eq!(parsed.current_folder, None);
    // Known-but-unimplemented keys are named; an unknown key is silent.
    assert_eq!(parsed.ignored, vec!["modal", "multiple", "filters"]);
}

#[test]
fn an_empty_accept_label_falls_back_to_open_and_a_title_to_the_app_name() {
    let call = Call {
        method: Method::OpenFile,
        app_id: "org.example.App".to_string(),
        title: None,
        options: OpenOptions {
            accept_label: Some(String::new()),
            ..OpenOptions::default()
        },
    };
    assert_eq!(call.accept_label(), "Open");
    assert_eq!(call.title(), "Gimbal Files");
}

#[test]
fn results_carry_one_encoded_uri_and_an_empty_choices_array() {
    let scratch = Scratch::new("results");
    let file = scratch.path().join("héllo wörld.txt");
    std::fs::write(&file, b"x").expect("file");

    let results = service::results_for(&file);
    let uris = results
        .get("uris")
        .expect("uris")
        .get::<Vec<String>>()
        .expect("one uri");
    assert_eq!(uris.len(), 1);
    assert!(uris[0].starts_with("file:///"), "{}", uris[0]);
    let back = gio::File::for_uri(&uris[0])
        .path()
        .expect("a local path from the uri");
    assert_eq!(back, file);
    let choices = results
        .get("choices")
        .expect("choices")
        .get::<Vec<(String, String)>>()
        .expect("an empty choices array");
    assert!(choices.is_empty());
}

#[test]
fn openfile_accepts_one_file_and_answers_success_with_its_uri() {
    let scratch = Scratch::new("accept");
    let folder = scratch.path().join("pictures");
    std::fs::create_dir_all(&folder).expect("folder");
    let file = folder.join("photo one.png");
    std::fs::write(&file, b"x").expect("file");

    let context = glib::MainContext::new();
    let _ = context.with_thread_default(|| {
        let host = Rc::new(FakeHost::default());
        let service = service::Service::new();
        service.set_host(host.clone());
        let bus = Bus::service(&service, Scratch::new("accept-bus"));

        let mut extra = options(&[("accept_label", "Select".to_variant())]);
        extra.insert("current_folder".to_string(), nul_path(&folder).to_variant());
        let reply = open_file(&bus, "org.example.App", "Pick a picture", extra);

        // The call is held while the dialog is up.
        assert!(
            bus.settles_without_reply(&reply),
            "the dialog answered before it was shown"
        );
        let requests = host.requests.borrow().clone();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].app_id(), "org.example.App");
        // Claude's review: the folder must survive the wire, not just parse.
        assert_eq!(host.folders.borrow().as_slice(), &[Some(folder.clone())]);

        requests[0].accept(&file);
        let (response, results) = reply_parts(&bus.wait(&reply).expect("a reply"));
        assert_eq!(response, RESPONSE_SUCCESS);
        let uris = results
            .get("uris")
            .expect("uris")
            .get::<Vec<String>>()
            .expect("one uri");
        assert_eq!(uris.len(), 1);
        assert_eq!(gio::File::for_uri(&uris[0]).path().expect("a path"), file);
        assert!(
            results
                .get("choices")
                .expect("choices")
                .get::<Vec<(String, String)>>()
                .expect("choices")
                .is_empty()
        );
    });
}

#[test]
fn request_close_answers_other_never_neither_success_nor_cancel() {
    let context = glib::MainContext::new();
    let _ = context.with_thread_default(|| {
        let host = Rc::new(FakeHost::default());
        let service = service::Service::new();
        service.set_host(host.clone());
        let bus = Bus::service(&service, Scratch::new("close-bus"));

        let reply = open_file(&bus, "org.example.App", "Open", HashMap::new());
        assert!(bus.settles_without_reply(&reply));

        let closed = close(&bus, HANDLE);
        bus.wait(&closed).expect("Close is answered");
        let (response, results) = reply_parts(&bus.wait(&reply).expect("a reply"));
        assert_eq!(response, RESPONSE_OTHER);
        assert!(results.is_empty());

        // The Request object is gone: a second Close finds nothing.
        let again = close(&bus, HANDLE);
        assert!(
            bus.wait(&again).is_err(),
            "the Request object outlived its answer"
        );
    });
}

#[test]
fn a_second_answer_is_a_no_op_after_accept() {
    let context = glib::MainContext::new();
    let _ = context.with_thread_default(|| {
        let scratch = Scratch::new("twice");
        let file = scratch.path().join("a.txt");
        std::fs::write(&file, b"x").expect("file");

        let host = Rc::new(FakeHost::default());
        let service = service::Service::new();
        service.set_host(host.clone());
        let bus = Bus::service(&service, Scratch::new("twice-bus"));

        let reply = open_file(&bus, "org.example.App", "Open", HashMap::new());
        assert!(bus.settles_without_reply(&reply));
        let request = host.requests.borrow()[0].clone();
        assert!(request.accept(&file));
        // Accept then Cancel (a click and then a destroy) answers once.
        assert!(!request.accept(&file));
        assert!(!request.complete(RESPONSE_CANCELLED, service::empty_results()));

        let (response, _) = reply_parts(&bus.wait(&reply).expect("a reply"));
        assert_eq!(response, RESPONSE_SUCCESS);
    });
}

#[test]
fn savefile_and_savefiles_answer_other_and_export_no_request_object() {
    let context = glib::MainContext::new();
    let _ = context.with_thread_default(|| {
        let host = Rc::new(FakeHost::default());
        let service = service::Service::new();
        service.set_host(host.clone());
        let bus = Bus::service(&service, Scratch::new("save-bus"));

        for method in ["SaveFile", "SaveFiles"] {
            let reply = bus.call(
                service::OBJECT_PATH,
                service::INTERFACE,
                method,
                Some(
                    (
                        glib::variant::ObjectPath::try_from(HANDLE).expect("a valid handle"),
                        "org.example.App".to_string(),
                        String::new(),
                        "Save".to_string(),
                        HashMap::<String, glib::Variant>::new(),
                    )
                        .to_variant(),
                ),
            );
            let (response, results) = reply_parts(&bus.wait(&reply).expect("a reply"));
            assert_eq!(response, RESPONSE_OTHER, "{method}");
            assert!(results.is_empty(), "{method}");

            // No dialog: no Request object, and no dialog host call.
            assert!(bus.wait(&close(&bus, HANDLE)).is_err(), "{method}");
        }
        assert!(host.requests.borrow().is_empty());
    });
}

#[test]
fn a_dialog_that_cannot_open_answers_other_instead_of_hanging() {
    let context = glib::MainContext::new();
    let _ = context.with_thread_default(|| {
        let host = Rc::new(FakeHost::default());
        host.fail.set(true);
        let service = service::Service::new();
        service.set_host(host.clone());
        let bus = Bus::service(&service, Scratch::new("fail-bus"));

        let reply = open_file(&bus, "org.example.App", "Open", HashMap::new());
        let (response, results) = reply_parts(&bus.wait(&reply).expect("a reply"));
        assert_eq!(response, RESPONSE_OTHER);
        assert!(results.is_empty());
    });
}

#[test]
fn a_cancelled_request_answers_cancelled_with_no_results() {
    let context = glib::MainContext::new();
    let _ = context.with_thread_default(|| {
        let host = Rc::new(FakeHost::default());
        let service = service::Service::new();
        service.set_host(host.clone());
        let bus = Bus::service(&service, Scratch::new("cancel-bus"));

        let reply = open_file(&bus, "org.example.App", "Open", HashMap::new());
        assert!(bus.settles_without_reply(&reply));
        host.requests.borrow()[0].complete(RESPONSE_CANCELLED, service::empty_results());
        let (response, results) = reply_parts(&bus.wait(&reply).expect("a reply"));
        assert_eq!(response, RESPONSE_CANCELLED);
        assert!(results.is_empty());
    });
}

#[test]
fn the_picker_allowlist_refuses_every_file_effect_and_the_settings_writes() {
    use fm_core::command::{COMMANDS, Caller, Effect};

    for spec in COMMANDS {
        let accepted = crate::app::picker_accepts(spec.name);
        match spec.effect {
            Effect::Files => assert!(!accepted, "{} must be refused", spec.name),
            Effect::View | Effect::Settings | Effect::System | Effect::Launch => {}
        }
        // Everything that could create or rearrange windows is outside F1.
        if accepted {
            assert!(
                spec.name.starts_with("nav.")
                    || spec.name.starts_with("selection.")
                    || matches!(
                        spec.name,
                        "view.set-sort"
                            | "view.clear-folder-sort"
                            | "view.set-hidden"
                            | "view.toggle-hidden"
                            | "picker.accept"
                            | "picker.cancel"
                    ),
                "{} is in the picker's allowlist but not in its design",
                spec.name
            );
        }
    }

    // The specific commands the brief names, with the outcome they get.
    for name in [
        "file.trash",
        "file.rename",
        "file.paste",
        "file.new-folder",
        "file.cut",
        "file.copy",
        "file.move",
    ] {
        let refusal = crate::app::picker_refusal(name).expect("a refusal");
        assert!(refusal.contains("not available"), "{refusal}");
    }
    for name in [
        "view.toggle-section",
        "view.set-scale",
        "pins.add",
        "tab.new",
        "window.new",
        "app.settings",
        "agent.set-enabled",
        "search.open",
        "file.open",
    ] {
        assert!(
            crate::app::picker_refusal(name).is_some(),
            "{name} must be refused"
        );
    }
    for name in [
        "nav.go",
        "nav.back",
        "nav.forward",
        "nav.up",
        "nav.refresh",
        "nav.focus-path",
        "selection.all",
        "selection.none",
        "view.set-sort",
        "view.clear-folder-sort",
        "view.set-hidden",
        "view.toggle-hidden",
        "picker.accept",
        "picker.cancel",
    ] {
        assert!(crate::app::picker_accepts(name), "{name} must be accepted");
    }

    // The catalogue's caller rules already refuse the CLI and the agent; this
    // restates the UI/Hotkey half for the two dialog answers.
    for name in fm_core::command::PICKER_COMMANDS {
        assert!(fm_core::command::caller_allowed(
            name,
            Effect::View,
            Caller::Ui
        ));
        assert!(fm_core::command::caller_allowed(
            name,
            Effect::View,
            Caller::Hotkey
        ));
    }
}

#[test]
fn only_the_picker_registry_answers_a_window() {
    // A fresh id is not a picker: the allowlist never applies to a main window.
    let id = fm_core::workspace::WindowId::from_raw(1);
    assert!(!picker::is_live(id));
    assert!(picker::get(id).is_none());
    assert!(!picker::forget(id));
}

#[test]
fn current_folder_from_the_wire_text_reaches_the_options() {
    // Claude's review: the live run opened at home although `current_folder`
    // named an existing folder. This parses exactly what `gdbus call` and
    // xdg-desktop-portal send (`ay` inside `v`, with a trailing NUL).
    let text = "{'current_folder': <b'/tmp/gimbal-p2am-wire'>, 'multiple': <true>}";
    let variant = glib::Variant::parse(Some(glib::VariantTy::new("a{sv}").expect("type")), text)
        .expect("a vardict");
    let map = variant
        .get::<HashMap<String, glib::Variant>>()
        .expect("a{sv} as a map");
    let parsed = service::parse_open_options(&map);
    assert_eq!(
        parsed.current_folder,
        Some(PathBuf::from("/tmp/gimbal-p2am-wire"))
    );
}
