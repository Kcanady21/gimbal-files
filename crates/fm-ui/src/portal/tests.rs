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
    self, Call, CallOptions, DialogHost, Method, RESPONSE_CANCELLED, RESPONSE_OTHER,
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

/// The Open-specific spelling the Open tests read, exactly as the service's
/// old wrapper did.
fn parse_open_options(options: &HashMap<String, glib::Variant>) -> CallOptions {
    service::parse_call_options(Method::OpenFile, options)
}

fn nul_path(path: &std::path::Path) -> Vec<u8> {
    let mut bytes = path.to_string_lossy().into_owned().into_bytes();
    bytes.push(0);
    bytes
}

/// The answer of a dialog with no filters and no choices: one path.
fn pick_one(path: &std::path::Path) -> service::Pick {
    service::Pick {
        paths: vec![path.to_path_buf()],
        current_filter: None,
        choices: Vec::new(),
    }
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
fn parse_open_options_keeps_the_label_and_names_only_the_key_it_still_ignores() {
    let parsed = parse_open_options(&options(&[
        ("accept_label", "_Open".to_variant()),
        ("modal", true.to_variant()),
        ("multiple", true.to_variant()),
        ("directory", true.to_variant()),
        (
            "filters",
            vec![(
                "Images",
                vec![
                    (0u32, "*.png".to_string()),
                    (1u32, "image/jpeg".to_string()),
                ],
            )]
            .to_variant(),
        ),
        (
            "choices",
            vec![(
                "enc",
                "Encoding",
                vec![("utf8".to_string(), "UTF-8".to_string())],
                "utf8".to_string(),
            )]
            .to_variant(),
        ),
        ("something_new", 1i32.to_variant()),
    ]));
    assert_eq!(parsed.accept_label.as_deref(), Some("_Open"));
    assert_eq!(parsed.current_folder, None);
    assert!(parsed.multiple, "a `b` option is read");
    assert!(parsed.directory);
    assert_eq!(parsed.filters.len(), 1);
    assert_eq!(parsed.filters[0].name, "Images");
    assert_eq!(parsed.choices.len(), 1);
    // Only `modal` is left unimplemented until F4; an unknown key is silent.
    assert_eq!(parsed.ignored, vec!["modal"]);
    assert!(parsed.malformed.is_empty());
}

#[test]
fn the_options_default_to_one_file_a_folder_and_no_filter_or_choice() {
    let parsed = parse_open_options(&HashMap::new());
    assert!(!parsed.multiple);
    assert!(!parsed.directory);
    assert!(parsed.filters.is_empty());
    assert!(parsed.choices.is_empty());
    assert_eq!(parsed.current_filter, None);
    assert_eq!(parsed.start_filter_for(Method::OpenFile), None);
    assert!(!parsed.shows_filters_for(Method::OpenFile));
    assert!(parsed.ignored.is_empty() && parsed.malformed.is_empty());
}

#[test]
fn a_value_of_the_wrong_type_is_read_as_absent_and_named_for_the_log() {
    let parsed = parse_open_options(&options(&[
        ("accept_label", 7u32.to_variant()),
        ("current_folder", "/tmp".to_variant()),
        ("multiple", "yes".to_variant()),
        ("directory", 1u32.to_variant()),
        ("filters", "*.png".to_variant()),
        ("current_filter", "Images".to_variant()),
        ("choices", 0u32.to_variant()),
    ]));
    // The dialog still opens: nothing here is an error.
    assert_eq!(parsed.accept_label, None);
    assert_eq!(parsed.current_folder, None);
    assert!(!parsed.multiple);
    assert!(!parsed.directory);
    assert!(parsed.filters.is_empty());
    assert!(parsed.fixed_filter.is_none());
    assert!(parsed.choices.is_empty());
    assert_eq!(
        parsed.malformed,
        vec![
            "accept_label",
            "current_folder",
            "filters",
            "current_filter",
            "choices",
            "multiple",
            "directory"
        ]
    );
}

#[test]
fn savefile_options_read_current_name_current_file_and_current_folder() {
    let folder = PathBuf::from("/tmp/gimbal-save");
    let file = folder.join("report.odt");
    let parsed = service::parse_call_options(
        Method::SaveFile,
        &options(&[
            ("current_name", "Untitled.odt".to_variant()),
            ("current_file", nul_path(&file).to_variant()),
            ("current_folder", nul_path(&folder).to_variant()),
            ("accept_label", "Save it".to_variant()),
            ("multiple", true.to_variant()),
        ]),
    );
    assert_eq!(parsed.current_name.as_deref(), Some("Untitled.odt"));
    assert_eq!(parsed.current_file, Some(file));
    assert_eq!(parsed.current_folder, Some(folder));
    assert_eq!(parsed.accept_label.as_deref(), Some("Save it"));
    // `multiple` is Open's key: unknown to SaveFile, so silent and absent.
    assert!(!parsed.multiple);
    assert!(parsed.malformed.is_empty());
    assert_eq!(parsed.ignored, Vec::<&'static str>::new());
}

#[test]
fn savefiles_options_read_files_as_nul_terminated_byte_strings() {
    let parsed = service::parse_call_options(
        Method::SaveFiles,
        &options(&[(
            "files",
            vec![b"/tmp/a.txt\0".to_vec(), b"b.png".to_vec()].to_variant(),
        )]),
    );
    assert_eq!(
        parsed.files,
        vec![b"/tmp/a.txt\0".to_vec(), b"b.png".to_vec()]
    );
    // SaveFiles has no filters; `filters` is another method's key.
    assert!(parsed.filters.is_empty());
    assert!(parsed.malformed.is_empty());
}

#[test]
fn a_wrong_typed_save_key_is_absent_and_named_once() {
    let parsed = service::parse_call_options(
        Method::SaveFile,
        &options(&[
            ("current_name", 7u32.to_variant()),
            ("current_file", "/tmp/a".to_variant()),
            ("current_folder", 1u32.to_variant()),
            ("multiple", "yes".to_variant()),
        ]),
    );
    assert_eq!(parsed.current_name, None);
    assert_eq!(parsed.current_file, None);
    assert_eq!(parsed.current_folder, None);
    assert_eq!(
        parsed.malformed,
        vec!["current_folder", "current_name", "current_file"]
    );

    let parsed = service::parse_call_options(
        Method::SaveFiles,
        &options(&[("files", "a.txt".to_variant())]),
    );
    assert!(parsed.files.is_empty());
    assert_eq!(parsed.malformed, vec!["files"]);
}

#[test]
fn openfile_still_reads_only_its_own_keys() {
    let parsed = service::parse_call_options(
        Method::OpenFile,
        &options(&[
            ("current_name", "Untitled.odt".to_variant()),
            ("current_file", b"/tmp/a\0".to_vec().to_variant()),
            ("files", vec![b"/tmp/a\0".to_vec()].to_variant()),
            ("multiple", true.to_variant()),
        ]),
    );
    assert!(parsed.multiple);
    assert_eq!(parsed.current_name, None);
    assert_eq!(parsed.current_file, None);
    assert!(parsed.files.is_empty());
    assert!(parsed.malformed.is_empty(), "unknown keys are silent");
}

#[test]
fn starting_folder_for_savefile_prefers_the_current_files_folder() {
    let scratch = Scratch::new("save-start");
    let home = scratch.path().join("home");
    let folder = scratch.path().join("docs");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&folder).expect("folder");
    let file = folder.join("a.txt");
    let call = Call {
        method: Method::SaveFile,
        app_id: String::new(),
        title: None,
        options: service::parse_call_options(
            Method::SaveFile,
            &options(&[("current_file", nul_path(&file).to_variant())]),
        ),
    };
    assert_eq!(service::starting_folder_for(&call, &home), folder);

    // A file whose folder is gone falls back to `current_folder` or home.
    let call = Call {
        options: service::parse_call_options(
            Method::SaveFile,
            &options(&[
                (
                    "current_file",
                    nul_path(&scratch.path().join("gone/a.txt")).to_variant(),
                ),
                ("current_folder", nul_path(&home).to_variant()),
            ]),
        ),
        ..call
    };
    assert_eq!(service::starting_folder_for(&call, &home), home);
}

#[test]
fn the_extension_follows_the_filter_only_when_the_old_one_claimed_it() {
    use super::filter::{Filter, follow_extension};

    let text = Filter::from_variant(&filter_variant("Text", &[(0, "*.txt"), (1, "text/plain")]))
        .expect("a filter");
    let markdown = Filter::from_variant(&filter_variant(
        "Markdown",
        &[(0, "*.md"), (0, "*.markdown")],
    ))
    .expect("a filter");
    let mime =
        Filter::from_variant(&filter_variant("MIME", &[(1, "text/x-markdown")])).expect("a filter");
    let any = Filter::from_variant(&filter_variant("Any", &[(0, "*")])).expect("a filter");

    assert_eq!(
        follow_extension("Untitled.txt", Some(&text), Some(&markdown)).as_deref(),
        Some("Untitled.md")
    );
    assert_eq!(
        follow_extension("notes", Some(&text), Some(&markdown)),
        None
    );
    assert_eq!(
        follow_extension("report.pdf", Some(&text), Some(&markdown)),
        None,
        "an extension the old filter does not list stays"
    );
    assert_eq!(
        follow_extension("notes.txt", Some(&text), Some(&mime)),
        None,
        "a MIME-only target filter changes nothing"
    );
    assert_eq!(
        follow_extension("notes.txt", Some(&text), Some(&any)),
        None,
        "a bare * changes nothing"
    );
    assert_eq!(
        follow_extension("notes.TXT", Some(&text), Some(&markdown)).as_deref(),
        Some("notes.md"),
        "the old match is ASCII case-insensitive"
    );
    assert_eq!(
        follow_extension("notes.txt", Some(&any), Some(&markdown)),
        None,
        "a bare * old filter claims no extension"
    );
    // The new filter's first *simple* glob is the one used.
    assert_eq!(
        follow_extension(
            "notes.txt",
            Some(&text),
            Some(
                &Filter::from_variant(&filter_variant(
                    "Complex",
                    &[(0, "*.[mM][dD]"), (0, "*.rst")]
                ))
                .expect("a filter")
            )
        )
        .as_deref(),
        Some("notes.rst")
    );
}

fn filter_variant(name: &str, rules: &[(u32, &str)]) -> glib::Variant {
    let rules: Vec<(u32, String)> = rules
        .iter()
        .map(|(kind, pattern)| (*kind, pattern.to_string()))
        .collect();
    (name.to_string(), rules).to_variant()
}

/// One filter as the application's `a(sa(us))` holds it.
fn filter_tuple(name: &str, rules: &[(u32, &str)]) -> (String, Vec<(u32, String)>) {
    (
        name.to_string(),
        rules
            .iter()
            .map(|(kind, pattern)| (*kind, pattern.to_string()))
            .collect(),
    )
}

/// The `filters` option, typed as the portal sends it. A `Vec<Variant>` would
/// say `av` instead, and the service reads that as a malformed value, so the
/// array is built from the tuples themselves.
fn filter_array(filters: &[(String, Vec<(u32, String)>)]) -> glib::Variant {
    filters.to_vec().to_variant()
}

/// One entry of an `a(sa(us))`, as a variant: the bytes the service keeps and
/// answers back.
fn entry_of(filters: &glib::Variant, index: usize) -> glib::Variant {
    filters
        .try_child_value(index)
        .unwrap_or_else(|| panic!("no entry {index}"))
}

/// One `choices` entry: an id, a label, its options and a default.
type ChoiceTuple = (String, String, Vec<(String, String)>, String);

/// The `choices` option, typed as `a(ssa(ss)s)`.
fn choice_array(choices: &[ChoiceTuple]) -> glib::Variant {
    choices.to_vec().to_variant()
}

#[test]
fn filters_are_kept_in_the_order_sent_with_their_rules_and_their_bytes() {
    let filters = filter_array(&[
        filter_tuple("Images", &[(0, "*.png"), (1, "image/jpeg"), (7, "bogus")]),
        filter_tuple("Text", &[(1, "text/plain")]),
    ]);
    let parsed = parse_open_options(&options(&[("filters", filters.clone())]));
    assert_eq!(parsed.filters.len(), 2);
    assert_eq!(parsed.filters[0].name, "Images");
    assert_eq!(parsed.filters[1].name, "Text", "the app's order is kept");
    assert!(parsed.shows_filters_for(Method::OpenFile));
    // An unknown rule kind is dropped as `gtk_file_filter_new_from_gvariant`
    // drops it; the two it knows survive in the order sent.
    assert_eq!(parsed.filters[0].rules.len(), 2);
    assert_eq!(parsed.filters[0].rules[0].pattern, "*.png");
    assert_eq!(parsed.filters[0].rules[1].pattern, "image/jpeg");
    // The answer must be the bytes that arrived, not a re-encoding of them.
    assert_eq!(parsed.filters[0].raw(), &entry_of(&filters, 0));
    // With no `current_filter` the dialog starts on the first entry.
    assert_eq!(parsed.current_filter, None);
    assert_eq!(
        parsed
            .start_filter_for(Method::OpenFile)
            .map(|filter| filter.name.as_str()),
        Some("Images")
    );
}

#[test]
fn current_filter_selects_the_entry_whose_whole_value_matches() {
    let filters = filter_array(&[
        filter_tuple("Images", &[(0, "*.png")]),
        filter_tuple("Text", &[(1, "text/plain")]),
    ]);
    let text = entry_of(&filters, 1);
    let same_name_other_rules = filter_variant("Text", &[(0, "*.md")]);
    let parsed = parse_open_options(&options(&[
        ("filters", filters.clone()),
        ("current_filter", text.clone()),
    ]));
    assert_eq!(parsed.current_filter, Some(1), "the second entry");
    assert_eq!(
        parsed
            .start_filter_for(Method::OpenFile)
            .map(|filter| filter.raw().clone()),
        Some(text.clone())
    );
    assert_eq!(parsed.fixed_filter, None, "a list wins over a fixed filter");

    // Two filters with one name and different rules are different filters, so
    // matching on the whole `(sa(us))` value is what picks the entry.
    let parsed = parse_open_options(&options(&[
        ("filters", filters),
        ("current_filter", same_name_other_rules.clone()),
    ]));
    assert_eq!(
        parsed.current_filter, None,
        "a filter that is not in the list selects nothing"
    );
    assert_eq!(
        parsed
            .start_filter_for(Method::OpenFile)
            .map(|filter| filter.name.as_str()),
        Some("Images"),
        "and the dialog starts on the first"
    );
}

#[test]
fn a_current_filter_with_no_filters_is_one_fixed_filter_and_no_dropdown() {
    let only = filter_variant("PDF", &[(0, "*.pdf")]);
    let parsed = parse_open_options(&options(&[("current_filter", only.clone())]));
    assert!(parsed.filters.is_empty());
    assert!(
        !parsed.shows_filters_for(Method::OpenFile),
        "there is nothing to choose from"
    );
    let fixed = parsed
        .start_filter_for(Method::OpenFile)
        .expect("the fixed filter applies");
    assert_eq!(fixed.name, "PDF");
    assert_eq!(fixed.raw(), &only);
}

#[test]
fn folders_only_mode_hides_the_filter_dropdown_and_the_default_label_says_select() {
    let images = filter_array(&[filter_tuple("Images", &[(0, "*.png")])]);
    let call = |directory: bool| Call {
        method: Method::OpenFile,
        app_id: "org.example.App".to_string(),
        title: None,
        options: parse_open_options(&options(&[
            ("filters", images.clone()),
            ("directory", directory.to_variant()),
        ])),
    };
    let files = call(false);
    assert!(files.options.shows_filters_for(Method::OpenFile));
    assert_eq!(files.accept_label(), "Open");
    let folders = call(true);
    assert!(
        !folders.options.shows_filters_for(Method::OpenFile),
        "a folder picker ignores filters, so it shows no dropdown"
    );
    assert_eq!(
        folders.options.start_filter_for(Method::OpenFile),
        None,
        "and filters do not apply to its rows"
    );
    assert_eq!(
        folders.accept_label(),
        "Select",
        "until the app says otherwise"
    );

    let named = Call {
        options: parse_open_options(&options(&[
            ("directory", true.to_variant()),
            ("accept_label", "Choose".to_variant()),
        ])),
        ..folders
    };
    assert_eq!(named.accept_label(), "Choose", "the app's label still wins");
}

#[test]
fn choices_keep_their_order_and_an_empty_option_list_is_a_checkbox() {
    let enc = (
        "enc".to_string(),
        "Encoding".to_string(),
        vec![
            ("utf8".to_string(), "UTF-8".to_string()),
            ("latin1".to_string(), "Latin-1".to_string()),
        ],
        "latin1".to_string(),
    );
    let ro = (
        "ro".to_string(),
        "Read only".to_string(),
        Vec::<(String, String)>::new(),
        "false".to_string(),
    );
    let parsed = parse_open_options(&options(&[("choices", choice_array(&[ro, enc]))]));
    assert_eq!(parsed.choices.len(), 2);
    assert_eq!(parsed.choices[0].id, "ro", "the order the app sent");
    assert_eq!(parsed.choices[1].id, "enc");
    assert!(parsed.choices[0].is_check(), "no options is a checkbox");
    assert!(!parsed.choices[0].starts_checked());
    assert_eq!(parsed.choices[1].options.len(), 2);
    assert_eq!(
        parsed.choices[1].start_option(),
        1,
        "the default names the option to start on"
    );
}

#[test]
fn a_choice_default_that_is_not_one_of_the_options_starts_on_the_first() {
    let parsed = parse_open_options(&options(&[(
        "choices",
        choice_array(&[(
            "mode".to_string(),
            "Mode".to_string(),
            vec![
                ("a".to_string(), "A".to_string()),
                ("b".to_string(), "B".to_string()),
            ],
            "zzz".to_string(),
        )]),
    )]));
    assert_eq!(parsed.choices[0].start_option(), 0);
    // A checkbox reads "true" and nothing else as on, as Nautilus's
    // `g_strcmp0 (selected, "true") == 0` does.
    let parsed = parse_open_options(&options(&[(
        "choices",
        choice_array(&[(
            "box".to_string(),
            "Box".to_string(),
            Vec::<(String, String)>::new(),
            "true".to_string(),
        )]),
    )]));
    assert!(parsed.choices[0].starts_checked());
}

#[test]
fn a_malformed_choices_array_is_skipped_whole_never_an_error() {
    // `(ssa(ss)s)` is checked as a whole: one entry of the wrong shape makes the
    // array not the advertised type at all, which is the only malformation the
    // wire can carry. `g_variant_lookup` fails the same way for Nautilus.
    let parsed = parse_open_options(&options(&[(
        "choices",
        vec![("enc", "Encoding")].to_variant(),
    )]));
    assert!(parsed.choices.is_empty());
    assert_eq!(parsed.malformed, vec!["choices"]);
}

#[test]
fn a_log_line_names_the_mode_the_count_the_filter_and_the_choices() {
    let pick = service::Pick {
        paths: vec![PathBuf::from("/tmp/a.png"), PathBuf::from("/tmp/b.png")],
        current_filter: Some(filter_variant("Images", &[(0, "*.png")])),
        choices: vec![
            ("enc".to_string(), "utf8".to_string()),
            ("ro".to_string(), "false".to_string()),
        ],
    };
    assert_eq!(
        pick.log_line("files+multiple", Some("Images")),
        "accept: files+multiple, 2 uri(s), filter \"Images\", choices enc=utf8 ro=false"
    );
    let plain = service::Pick {
        paths: vec![PathBuf::from("/tmp/a.png")],
        current_filter: None,
        choices: Vec::new(),
    };
    assert_eq!(
        plain.log_line("folders", None),
        "accept: folders, 1 uri(s), filter none, choices none"
    );
}

#[test]
fn an_empty_accept_label_falls_back_to_open_and_a_title_to_the_app_name() {
    let call = Call {
        method: Method::OpenFile,
        app_id: "org.example.App".to_string(),
        title: None,
        options: CallOptions {
            accept_label: Some(String::new()),
            ..CallOptions::default()
        },
    };
    assert_eq!(call.accept_label(), "Open");
    assert_eq!(call.title(), "Gimbal Files");

    for method in [Method::SaveFile, Method::SaveFiles] {
        let save = Call {
            method,
            app_id: "org.example.App".to_string(),
            title: None,
            options: CallOptions::default(),
        };
        assert_eq!(save.accept_label(), "Save", "{method:?}");
    }
}

#[test]
fn results_carry_one_encoded_uri_and_an_empty_choices_array() {
    let scratch = Scratch::new("results");
    let file = scratch.path().join("héllo wörld.txt");
    std::fs::write(&file, b"x").expect("file");

    let results = service::results_for(&pick_one(&file), Method::OpenFile);
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

        requests[0].accept(pick_one(&file));

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
        assert!(request.accept(pick_one(&file)));

        // Accept then Cancel (a click and then a destroy) answers once.
        assert!(!request.accept(pick_one(&file)));

        assert!(!request.complete(RESPONSE_CANCELLED, service::empty_results()));

        let (response, _) = reply_parts(&bus.wait(&reply).expect("a reply"));
        assert_eq!(response, RESPONSE_SUCCESS);
    });
}

fn chooser_call(bus: &Bus, method: &str, extra: HashMap<String, glib::Variant>) -> Reply {
    bus.call(
        service::OBJECT_PATH,
        service::INTERFACE,
        method,
        Some(
            (
                glib::variant::ObjectPath::try_from(HANDLE).expect("a valid handle"),
                "org.example.App".to_string(),
                String::new(),
                "Save".to_string(),
                extra,
            )
                .to_variant(),
        ),
    )
}

#[test]
fn savefile_accepts_one_uri_with_no_writable_and_its_filter() {
    let scratch = Scratch::new("save-file");
    let folder = scratch.path().join("docs");
    std::fs::create_dir_all(&folder).expect("folder");
    let file = folder.join("report.odt");
    let filters = filter_array(&[filter_tuple("Text", &[(0, "*.txt")])]);
    let received = entry_of(&filters, 0);

    let context = glib::MainContext::new();
    let _ = context.with_thread_default(|| {
        let host = Rc::new(FakeHost::default());
        let service = service::Service::new();
        service.set_host(host.clone());
        let bus = Bus::service(&service, Scratch::new("save-file-bus"));

        let mut extra = options(&[
            ("filters", filters.clone()),
            ("current_name", "Untitled.odt".to_variant()),
        ]);
        extra.insert("current_folder".to_string(), nul_path(&folder).to_variant());
        let reply = chooser_call(&bus, "SaveFile", extra);
        assert!(bus.settles_without_reply(&reply));

        host.requests.borrow()[0].accept(service::Pick {
            paths: vec![file.clone()],
            current_filter: Some(received.clone()),
            choices: vec![("kind".to_string(), "odt".to_string())],
        });
        let (response, results) = reply_parts(&bus.wait(&reply).expect("a reply"));
        assert_eq!(response, RESPONSE_SUCCESS);
        let uris = results
            .get("uris")
            .expect("uris")
            .get::<Vec<String>>()
            .expect("one uri");
        assert_eq!(uris.len(), 1);
        assert_eq!(gio::File::for_uri(&uris[0]).path().expect("a path"), file);
        assert_eq!(
            results
                .get("current_filter")
                .expect("current_filter")
                .clone(),
            received
        );
        assert_eq!(
            results
                .get("choices")
                .expect("choices")
                .get::<Vec<(String, String)>>()
                .expect("choices"),
            vec![("kind".to_string(), "odt".to_string())]
        );
        assert!(
            !results.contains_key("writable"),
            "SaveFile carries no writable key"
        );
    });
}

#[test]
fn savefiles_answers_every_uri_in_order_with_no_writable_or_filter() {
    let scratch = Scratch::new("save-files");
    let folder = scratch.path().join("out");
    std::fs::create_dir_all(&folder).expect("folder");
    let first = folder.join("a.txt");
    let second = folder.join("b 三.txt");

    let context = glib::MainContext::new();
    let _ = context.with_thread_default(|| {
        let host = Rc::new(FakeHost::default());
        let service = service::Service::new();
        service.set_host(host.clone());
        let bus = Bus::service(&service, Scratch::new("save-files-bus"));

        let mut extra = options(&[(
            "files",
            vec![
                b"/tmp/a.txt\0".to_vec(),
                b"/tmp/b \xE4\xB8\x89.txt\0".to_vec(),
            ]
            .to_variant(),
        )]);
        extra.insert("current_folder".to_string(), nul_path(&folder).to_variant());
        let reply = chooser_call(&bus, "SaveFiles", extra);
        assert!(bus.settles_without_reply(&reply));

        host.requests.borrow()[0].accept(service::Pick {
            paths: vec![first.clone(), second.clone()],
            current_filter: None,
            choices: Vec::new(),
        });
        let (response, results) = reply_parts(&bus.wait(&reply).expect("a reply"));
        assert_eq!(response, RESPONSE_SUCCESS);
        let uris = results
            .get("uris")
            .expect("uris")
            .get::<Vec<String>>()
            .expect("uris");
        assert_eq!(uris.len(), 2);
        assert_eq!(gio::File::for_uri(&uris[0]).path().expect("a path"), first);
        assert_eq!(
            gio::File::for_uri(&uris[1]).path().expect("a path"),
            second,
            "a space and a non-ASCII name survive the encoding"
        );
        assert!(!results.contains_key("writable"));
        assert!(!results.contains_key("current_filter"));
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
fn savefile_and_savefiles_cancel_with_response_one() {
    let context = glib::MainContext::new();
    let _ = context.with_thread_default(|| {
        for method in ["SaveFile", "SaveFiles"] {
            let host = Rc::new(FakeHost::default());
            let service = service::Service::new();
            service.set_host(host.clone());
            let bus = Bus::service(&service, Scratch::new("save-cancel-bus"));

            let reply = chooser_call(&bus, method, HashMap::new());
            assert!(bus.settles_without_reply(&reply), "{method}");
            host.requests.borrow()[0].complete(RESPONSE_CANCELLED, service::empty_results());
            let (response, results) = reply_parts(&bus.wait(&reply).expect("a reply"));
            assert_eq!(response, RESPONSE_CANCELLED, "{method}");
            assert!(results.is_empty(), "{method}");
        }
    });
}

#[test]
fn close_on_a_save_request_answers_other_and_unexports_the_object() {
    let context = glib::MainContext::new();
    let _ = context.with_thread_default(|| {
        for method in ["SaveFile", "SaveFiles"] {
            let host = Rc::new(FakeHost::default());
            let service = service::Service::new();
            service.set_host(host.clone());
            let bus = Bus::service(&service, Scratch::new("save-close-bus"));

            let reply = chooser_call(&bus, method, HashMap::new());
            assert!(bus.settles_without_reply(&reply), "{method}");
            let closed = close(&bus, HANDLE);
            bus.wait(&closed).expect("Close is answered");
            let (response, results) = reply_parts(&bus.wait(&reply).expect("a reply"));
            assert_eq!(response, RESPONSE_OTHER, "{method}");
            assert!(results.is_empty(), "{method}");
            assert!(
                bus.wait(&close(&bus, HANDLE)).is_err(),
                "{method}: the Request object outlived its answer"
            );
        }
    });
}

#[test]
fn a_save_dialog_that_cannot_open_answers_other_instead_of_hanging() {
    let context = glib::MainContext::new();
    let _ = context.with_thread_default(|| {
        let host = Rc::new(FakeHost::default());
        host.fail.set(true);
        let service = service::Service::new();
        service.set_host(host.clone());
        let bus = Bus::service(&service, Scratch::new("save-fail-bus"));

        for method in ["SaveFile", "SaveFiles"] {
            let reply = chooser_call(&bus, method, HashMap::new());
            let (response, results) = reply_parts(&bus.wait(&reply).expect("a reply"));
            assert_eq!(response, RESPONSE_OTHER, "{method}");
            assert!(results.is_empty(), "{method}");
        }
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
fn the_picker_allowlist_allows_new_folder_and_refuses_every_other_file_effect() {
    use fm_core::command::{COMMANDS, Caller, Effect};

    for spec in COMMANDS {
        let accepted = crate::app::picker_accepts(spec.name);
        match spec.effect {
            // New Folder is the one file effect the picker may run. Every other
            // one, including New File, Paste and the trashing commands, is
            // refused before a handler can touch the disk.
            Effect::Files if spec.name == "file.new-folder" => {
                assert!(accepted, "{} is the one file effect Save needs", spec.name)
            }
            Effect::Files => assert!(!accepted, "{} must be refused", spec.name),
            Effect::View | Effect::Settings | Effect::System | Effect::Launch => {}
        }
        // Everything that could create or rearrange windows is outside the
        // chooser, as is every settings write and every file effect but New
        // Folder.
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
                            | "search.open"
                            | "picker.accept"
                            | "picker.cancel"
                            | "file.new-folder"
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
        "file.new-file",
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
        // F2 added this one: `Effect::View`, and its handler reads the model's
        // folder, hands a query to the read-only worker and shows the results
        // page. It writes no config, no session and no file.
        "search.open",
        "selection.all",
        "selection.none",
        "view.set-sort",
        "view.clear-folder-sort",
        "view.set-hidden",
        "view.toggle-hidden",
        "picker.accept",
        "picker.cancel",
        // F3's one file effect: New Folder, never a shortcut.
        "file.new-folder",
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
    let parsed = parse_open_options(&map);
    assert_eq!(
        parsed.current_folder,
        Some(PathBuf::from("/tmp/gimbal-p2am-wire"))
    );
}

// ---------------------------------------------------------------------------
// The filter's matching rules: `portal/filter.rs` is pure apart from GIO's
// content-type table, so both halves are checked here without a window.
// ---------------------------------------------------------------------------

#[test]
fn a_glob_matches_the_way_gtk_filefilter_matches() {
    use super::filter::fnmatch;

    // `*` stands for anything.
    assert!(fnmatch("*.png", "photo.png"));
    assert!(fnmatch("*", "anything at all"));
    assert!(!fnmatch("*.png", "photo.jpg"));
    // `?` is exactly one character.
    assert!(fnmatch("image-?.png", "image-1.png"));
    assert!(!fnmatch("image-?.png", "image-12.png"));
    // Bracket classes, their negation, and ranges.
    assert!(fnmatch("*.[abc]", "x.b"));
    assert!(!fnmatch("*.[abc]", "x.d"));
    assert!(fnmatch("*.[!abc]", "x.d"));
    assert!(!fnmatch("*.[!abc]", "x.a"));
    assert!(fnmatch("*.[^abc]", "x.d"), "^ negates as well as !");
    assert!(fnmatch("x[a-c]z", "xbz"));
    assert!(!fnmatch("x[a-c]z", "xdz"));
    // The bracket pattern an application sends when it wants a
    // case-insensitive suffix, which is how the two rules differ.
    assert!(fnmatch("*.[jJ][pP][gG]", "Photo.JPG"));
    assert!(fnmatch("*.[jJ][pP][gG]", "photo.jpg"));
    // And a plain glob is case-sensitive on Linux: GTK documents it, and this is
    // the difference the live check looks for (`*.jpg` and `SHOUT.PNG`).
    assert!(!fnmatch("*.jpg", "A.JPG"));
    assert!(!fnmatch("*.png", "SHOUT.PNG"));
    // A leading dot is not special, so `*` reaches a hidden name. GTK's file
    // filter passes `no_leading_period = FALSE`.
    assert!(fnmatch("*.txt", ".hidden.txt"));
    assert!(fnmatch("*", ".dotfile"));
    // A space and a non-ASCII name are ordinary characters, matched a character
    // at a time rather than a byte at a time.
    assert!(fnmatch("*.png", "my photo.png"));
    assert!(fnmatch("*ö*", "gröna"));
    assert!(!fnmatch("*ö*", "grona"));
    // A backslash escapes: `FNM_NOESCAPE` is set only on Windows.
    assert!(fnmatch("a\\*b", "a*b"));
    assert!(!fnmatch("a\\*b", "axb"));
    // An empty pattern takes an empty name, and an unterminated class loses.
    assert!(fnmatch("", ""));
    assert!(!fnmatch("", "x"));
    assert!(!fnmatch("[]", "]"), "an unterminated class matches nothing");
}

#[test]
fn a_mime_rule_matches_through_gio_including_a_wildcard() {
    let png = super::filter::Filter::from_variant(&filter_variant("PNG", &[(1, "image/png")]))
        .expect("a filter");
    let any_image =
        super::filter::Filter::from_variant(&filter_variant("Any image", &[(1, "image/*")]))
            .expect("a filter");
    let text = super::filter::Filter::from_variant(&filter_variant("Text", &[(1, "text/plain")]))
        .expect("a filter");

    assert!(png.matches("x", Some("image/png")));
    assert!(!png.matches("x", Some("text/plain")));
    // A wildcard supertype works through `g_content_type_is_a`. Measured on this
    // machine (GLib 2.88), not assumed: `image/png` against `image/*` is true,
    // and so is `text/x-changelog` against `text/*`.
    assert!(any_image.matches("x", Some("image/png")));
    assert!(any_image.matches("x", Some("image/jpeg")));
    assert!(!any_image.matches("x", Some("text/plain")));
    assert!(!text.matches("x", Some("image/png")));

    // From a name alone — what the search worker has, since no file may be
    // opened during a scan — the type is guessed from the extension.
    assert!(png.matches_name("photo.png"));
    assert!(!png.matches_name("notes.txt"));
    // GIO's extension table ignores case, so a MIME rule catches what a `*.png`
    // glob does not. The two rule kinds are not interchangeable.
    assert!(png.matches_name("SHOUT.PNG"));
    // A name with no extension is `application/octet-stream`, which is not an
    // image.
    assert!(!png.matches_name("noext"));
    assert!(any_image.needs_content_type());
    assert!(
        !super::filter::Filter::from_variant(&filter_variant("Globs", &[(0, "*.png")]))
            .expect("a filter")
            .needs_content_type(),
        "a filter of globs alone never asks GIO"
    );
}

#[test]
fn a_filter_with_no_rules_admits_nothing_as_gtk_s_does() {
    let empty =
        super::filter::Filter::from_variant(&filter_variant("Empty", &[])).expect("a filter");
    assert!(!empty.matches_name("photo.png"));
    assert!(!empty.matches("photo.png", Some("image/png")));
    // And a filter never hides a folder: `ViewRules` decides that, so the dialog
    // stays navigable.
    let rules = super::filter::ViewRules {
        folders_only: false,
        filter: Some(empty),
        show_hidden: true,
    };
    assert!(rules.keeps("Pictures", true, None));
    assert!(!rules.keeps("photo.png", false, Some("image/png")));
}

#[test]
fn the_dialog_rules_apply_hidden_mode_and_filter_in_that_order() {
    use super::filter::ViewRules;

    let png = super::filter::Filter::from_variant(&filter_variant("PNG", &[(0, "*.png")]))
        .expect("a filter");
    let rules = ViewRules {
        folders_only: false,
        filter: Some(png),
        show_hidden: false,
    };
    assert!(rules.keeps("photo.png", false, Some("image/png")));
    assert!(!rules.keeps("notes.txt", false, Some("text/plain")));
    assert!(rules.keeps("Pictures", true, None), "a folder always shows");
    assert!(
        !rules.keeps(".photo.png", false, Some("image/png")),
        "Ctrl+H off hides a dotfile even when the filter admits it"
    );
    assert!(
        !rules.keeps(".secret", true, None),
        "and a dotfile folder follows the switch too"
    );

    // Folders-only mode ignores the filter and keeps folders only.
    let folders = ViewRules {
        folders_only: true,
        filter: None,
        show_hidden: false,
    };
    assert!(folders.keeps("Pictures", true, None));
    assert!(
        !folders.keeps(".secret", true, None),
        "Ctrl+H off hides a dotfile folder too"
    );
    assert!(!folders.keeps("photo.png", false, Some("image/png")));

    // With no filter and hidden files on, everything shows: the main app's rules.
    let open = ViewRules {
        folders_only: false,
        filter: None,
        show_hidden: true,
    };
    assert!(open.keeps("notes.txt", false, None));
    assert!(open.keeps(".hidden", false, None));
}

/// The dialog's rules in the form the search worker takes, run over a real tree
/// with `NativeSearchSource`: the filter is tested where it runs, before the
/// score is computed, not next to a stub.
#[test]
fn the_search_filter_keeps_only_what_the_dialog_would_show() {
    use super::filter::SearchFilter;
    use fm_core::folder_size::SystemClock;
    use fm_core::search::{NativeSearchSource, SearchRequest, SearchResultFilter, SearchSource};
    use std::ffi::OsStr;
    use std::sync::Arc;

    let scratch = Scratch::new("search-filter");
    let root = scratch.path().to_path_buf();
    std::fs::write(root.join("a.png"), b"x").expect("file");
    std::fs::write(root.join("a.txt"), b"x").expect("file");
    std::fs::write(root.join(".a.png"), b"x").expect("file");
    std::fs::create_dir_all(root.join("data")).expect("directory");

    let run = |rules: super::filter::ViewRules, max_results: usize| -> (Vec<String>, u64) {
        let filter: Option<Arc<dyn SearchResultFilter>> = Some(Arc::new(SearchFilter(rules)));
        let mut request = SearchRequest::new(root.clone(), "a");
        request.filter = filter;
        request.limits.max_results = max_results;
        let mut rows: Vec<std::path::PathBuf> = Vec::new();
        let mut matches = 0;
        let outcome = fm_core::search::search(
            &request,
            &NativeSearchSource as &dyn SearchSource,
            &SystemClock,
            &|| false,
            &mut |progress| matches = progress.matches,
            &mut |changes| {
                for change in changes {
                    match change {
                        fm_core::search::SearchChange::Insert { index, row } => {
                            rows.insert(index.min(rows.len()), row.path)
                        }
                        fm_core::search::SearchChange::Remove { index, .. } => {
                            if index < rows.len() {
                                rows.remove(index);
                            }
                        }
                    }
                }
                true
            },
        );
        assert!(
            matches!(outcome, fm_core::search::SearchOutcome::Finished(_)),
            "a scratch tree is searchable: {outcome:?}"
        );
        let names = rows
            .iter()
            .map(|path| {
                path.file_name()
                    .map(OsStr::to_string_lossy)
                    .unwrap_or_default()
                    .into_owned()
            })
            .collect();
        (names, matches)
    };

    // The plain file dialog: the type filter and Ctrl+H both apply, folders stay.
    let png = super::filter::Filter::from_variant(&filter_variant("PNG", &[(0, "*.png")]))
        .expect("a filter");
    let (names, matches) = run(
        super::filter::ViewRules {
            folders_only: false,
            filter: Some(png.clone()),
            show_hidden: false,
        },
        100,
    );
    assert_eq!(
        names,
        vec!["a.png", "data"],
        "a.png and the folder; `a.txt` and the dotfile are gone"
    );
    assert_eq!(matches, 2, "an excluded row is not counted as a match");

    // Ctrl+H on lets the dotfile in, as it does in the grid.
    let (names, _) = run(
        super::filter::ViewRules {
            folders_only: false,
            filter: Some(png),
            show_hidden: true,
        },
        100,
    );
    assert!(
        names.iter().any(|name| name == ".a.png"),
        "a hidden name the switch admits is searchable: {names:?}"
    );

    // Folders-only mode: one folder, and nothing else.
    let (names, matches) = run(
        super::filter::ViewRules {
            folders_only: true,
            filter: None,
            show_hidden: false,
        },
        100,
    );
    assert_eq!(names, vec!["data"]);
    assert_eq!(matches, 1);

    // The limit is spent on kept rows: with two `.png` matches kept and a limit
    // of one, one row is retained and one is reported truncated — the `.txt`
    // file never took part.
    let png = super::filter::Filter::from_variant(&filter_variant("PNG", &[(0, "*.png")]))
        .expect("a filter");
    let (names, matches) = run(
        super::filter::ViewRules {
            folders_only: false,
            filter: Some(png),
            show_hidden: true,
        },
        1,
    );
    assert_eq!(names.len(), 1, "one row, not one row of every kind");
    assert_eq!(matches, 3, "two pngs and the folder all match `a`");
}

#[test]
fn a_success_result_reports_every_uri_the_choices_and_writable_true() {
    let scratch = Scratch::new("full-result");
    let folder = scratch.path().join("photos");
    std::fs::create_dir_all(&folder).expect("folder");
    let first = folder.join("one.png");
    let second = folder.join("two 三.png");
    std::fs::write(&first, b"x").expect("file");
    std::fs::write(&second, b"x").expect("file");
    let filters = filter_array(&[filter_tuple("PNG", &[(0, "*.png"), (1, "image/png")])]);
    let received = entry_of(&filters, 0);

    let context = glib::MainContext::new();
    let _ = context.with_thread_default(|| {
        let host = Rc::new(FakeHost::default());
        let service = service::Service::new();
        service.set_host(host.clone());
        let bus = Bus::service(&service, Scratch::new("full-result-bus"));

        let mut extra = options(&[
            ("multiple", true.to_variant()),
            ("filters", filters.clone()),
            (
                "choices",
                choice_array(&[
                    (
                        "enc".to_string(),
                        "Encoding".to_string(),
                        vec![("utf8".to_string(), "UTF-8".to_string())],
                        "utf8".to_string(),
                    ),
                    (
                        "ro".to_string(),
                        "Read only".to_string(),
                        Vec::<(String, String)>::new(),
                        "false".to_string(),
                    ),
                ]),
            ),
        ]);
        extra.insert("current_folder".to_string(), nul_path(&folder).to_variant());
        let reply = open_file(&bus, "org.example.App", "Pick images", extra);
        assert!(bus.settles_without_reply(&reply));

        host.requests.borrow()[0].accept(service::Pick {
            paths: vec![first.clone(), second.clone()],
            current_filter: Some(received.clone()),
            choices: vec![
                ("enc".to_string(), "utf8".to_string()),
                ("ro".to_string(), "false".to_string()),
            ],
        });
        let (response, results) = reply_parts(&bus.wait(&reply).expect("a reply"));
        assert_eq!(response, RESPONSE_SUCCESS);

        let uris = results
            .get("uris")
            .expect("uris")
            .get::<Vec<String>>()
            .expect("a uri array");
        assert_eq!(
            uris.len(),
            2,
            "both picks, in the order the grid showed them"
        );
        assert_eq!(gio::File::for_uri(&uris[0]).path().expect("a path"), first);
        assert_eq!(
            gio::File::for_uri(&uris[1]).path().expect("a path"),
            second,
            "a space and a non-ASCII name survive the encoding"
        );

        let choices = results
            .get("choices")
            .expect("choices")
            .get::<Vec<(String, String)>>()
            .expect("a(ss) array");
        assert_eq!(
            choices,
            vec![
                ("enc".to_string(), "utf8".to_string()),
                ("ro".to_string(), "false".to_string())
            ],
            "every choice the app sent, in the order it sent them"
        );

        assert_eq!(
            results
                .get("current_filter")
                .expect("current_filter")
                .clone(),
            received,
            "the filter as it was received, byte for byte"
        );
        assert!(
            results
                .get("writable")
                .expect("writable")
                .get::<bool>()
                .expect("a boolean"),
            "the picker always answers writable: true"
        );
    });
}

/// P2at's live checks, driven in-process against the real picker: the real
/// `Process::portal`, the real `PickerHost` and service on a private bus, and
/// the picker's own entry, buttons and alerts reached through the same methods
/// a click or Enter reaches. No desktop input is injected; nothing is created
/// outside the scratch tree except by New Folder inside it.
#[test]
fn real_save_dialog_answers_names_paths_and_collisions() {
    let scratch = Scratch::new("save-widget-env");
    let config = scratch.path().join("config");
    let state = scratch.path().join("state");
    std::fs::create_dir_all(&config).expect("scratch config");
    std::fs::create_dir_all(&state).expect("scratch state");
    let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "portal::tests::real_save_dialog_answers_names_paths_and_collisions_inner",
            "--ignored",
            "--nocapture",
        ])
        .env("XDG_CONFIG_HOME", &config)
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("the owned widget regression process");
    assert!(
        output.status.success(),
        "the real save dialog regression failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "the inner regression did not run"
    );
}

#[test]
#[ignore = "runs in its own process through the subprocess widget regression"]
fn real_save_dialog_answers_names_paths_and_collisions_inner() {
    gtk::init().expect("the widget regression needs a GTK display");
    adw::init().expect("libadwaita init");
    let context = glib::MainContext::default();
    let settle = || pump_for(&context, Duration::from_millis(150));

    let scratch = Scratch::new("save-widget");
    let work = scratch.path().join("work");
    let sub = work.join("sub");
    let other = work.join("other");
    std::fs::create_dir_all(&sub).expect("sub");
    std::fs::create_dir_all(&other).expect("other");
    std::fs::write(work.join("existing.txt"), b"keep me").expect("existing");
    std::fs::write(other.join("picked.txt"), b"x").expect("picked");
    let multi = scratch.path().join("multi");
    std::fs::create_dir_all(&multi).expect("multi");
    std::fs::write(multi.join("b.txt"), b"b").expect("b");

    let application = adw::Application::builder()
        .application_id("io.github.kcanady21.GimbalFiles.PortalSaveTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    application
        .register(None::<&gio::Cancellable>)
        .expect("register the owned test application");
    let process = crate::app::Process::portal(&application);
    let host = picker::PickerHost::new(&process, &application);
    let service = service::Service::new();
    service.set_host(host);
    let bus = Bus::service(&service, Scratch::new("save-widget-bus"));
    let uri = |path: &std::path::Path| gio::File::for_path(path).uri().to_string();
    let uris = |results: &HashMap<String, glib::Variant>| {
        results
            .get("uris")
            .and_then(|value| value.get::<Vec<String>>())
            .expect("uris")
    };
    let one_picker = || {
        pump_until(&context, Duration::from_secs(5), || {
            picker::testing::live().len() == 1
        });
        let live = picker::testing::live();
        assert_eq!(live.len(), 1, "exactly one live picker");
        live[0].clone()
    };
    let folder_bytes = |path: &std::path::Path| nul_path(path).to_variant();

    // 1. current_name: focused, the stem selected; Enter answers, writes nothing.
    let mut extra = options(&[("current_name", "Untitled.odt".to_variant())]);
    extra.insert("current_folder".to_string(), folder_bytes(&work));
    let reply = chooser_call(&bus, "SaveFile", extra);
    let picker = one_picker();
    settle();
    assert_eq!(picker.name_text().as_deref(), Some("Untitled.odt"));
    assert_eq!(
        picker.name_selection(),
        Some((0, 8)),
        "the stem is selected"
    );
    assert!(picker.name_focused(), "the Name entry has focus");
    assert!(picker.has_new_folder_button());
    picker.set_name("report.odt");
    picker.press_enter_in_name();
    let (response, results) = reply_parts(&bus.wait(&reply).expect("a SaveFile reply"));
    assert_eq!(response, RESPONSE_SUCCESS);
    assert_eq!(uris(&results), vec![uri(&work.join("report.odt"))]);
    assert!(!results.contains_key("writable"));
    assert!(!work.join("report.odt").exists(), "Save writes nothing");
    drop(picker);
    settle();

    // 2. current_file: its folder and name; Save asks; Cancel returns, Replace answers.
    let mut extra = HashMap::new();
    extra.insert(
        "current_file".to_string(),
        folder_bytes(&work.join("existing.txt")),
    );
    let reply = chooser_call(&bus, "SaveFile", extra);
    let picker = one_picker();
    settle();
    assert_eq!(picker.shown_folder().as_deref(), Some(work.as_path()));
    assert_eq!(picker.name_text().as_deref(), Some("existing.txt"));
    picker.press_enter_in_name();
    settle();
    let body = picker.alert_body().expect("the overwrite alert");
    assert!(body.contains("“existing.txt” already exists"), "{body}");
    picker.respond_to_alert("cancel");
    assert!(
        bus.settles_without_reply(&reply),
        "Cancel returns to the dialog"
    );
    assert!(
        picker.name_focused(),
        "Cancel puts the caret back in the name"
    );
    picker.press_enter_in_name();
    settle();
    picker.respond_to_alert("replace");
    let (response, results) = reply_parts(&bus.wait(&reply).expect("a replace reply"));
    assert_eq!(response, RESPONSE_SUCCESS);
    assert_eq!(uris(&results), vec![uri(&work.join("existing.txt"))]);
    assert_eq!(
        std::fs::read(work.join("existing.txt")).expect("existing"),
        b"keep me",
        "the backend never writes the file"
    );
    drop(picker);
    settle();

    // 3–6. Filters, folders, typed paths, refusals, selection and New Folder.
    let filters = filter_array(&[
        filter_tuple("Text", &[(0, "*.txt")]),
        filter_tuple("Markdown", &[(0, "*.md"), (0, "*.markdown")]),
    ]);
    let mut extra = options(&[
        ("filters", filters),
        ("current_name", "notes.txt".to_variant()),
    ]);
    extra.insert("current_folder".to_string(), folder_bytes(&work));
    let reply = chooser_call(&bus, "SaveFile", extra);
    let picker = one_picker();
    settle();
    picker.select_filter(1);
    settle();
    assert_eq!(
        picker.name_text().as_deref(),
        Some("notes.md"),
        "the extension follows"
    );
    picker.set_name("notes.rst");
    picker.select_filter(0);
    settle();
    assert_eq!(
        picker.name_text().as_deref(),
        Some("notes.rst"),
        "a typed extension stays"
    );

    picker.set_name("sub");
    picker.press_enter_in_name();
    pump_until(&context, Duration::from_secs(3), || {
        picker.shown_folder().as_deref() == Some(sub.as_path())
    });
    assert_eq!(
        picker.shown_folder().as_deref(),
        Some(sub.as_path()),
        "a folder name is entered"
    );
    assert!(
        bus.settles_without_reply(&reply),
        "entering a folder is not an answer"
    );

    picker.set_name(&other.join("x.txt").to_string_lossy());
    picker.press_enter_in_name();
    pump_until(&context, Duration::from_secs(3), || {
        picker.shown_folder().as_deref() == Some(other.as_path())
    });
    assert_eq!(picker.shown_folder().as_deref(), Some(other.as_path()));
    assert_eq!(
        picker.name_text().as_deref(),
        Some("x.txt"),
        "a typed path keeps its last part"
    );

    // No wait for the new folder's listing: a Save refusal holds the status
    // line, so the listing finishing now must not cover it.
    picker.set_name("a/b");
    picker.press_enter_in_name();
    settle();
    assert!(
        picker
            .texts()
            .iter()
            .any(|text| text == "A name can't contain /"),
        "{:?}",
        picker.texts()
    );

    picker.click_file("picked.txt");
    assert_eq!(
        picker.name_text().as_deref(),
        Some("picked.txt"),
        "a clicked file fills the name"
    );

    picker.new_folder_here("exports");
    let exports = other.join("exports");
    pump_until(&context, Duration::from_secs(4), || {
        picker.shown_folder().as_deref() == Some(exports.as_path())
    });
    assert!(exports.is_dir(), "New Folder created the folder");
    assert_eq!(
        picker.shown_folder().as_deref(),
        Some(exports.as_path()),
        "and opened it"
    );
    picker.set_name("final.txt");
    picker.press_enter_in_name();
    let (response, results) = reply_parts(&bus.wait(&reply).expect("a reply in exports"));
    assert_eq!(response, RESPONSE_SUCCESS);
    assert_eq!(uris(&results), vec![uri(&exports.join("final.txt"))]);
    assert!(results.contains_key("current_filter"));
    let entries: Vec<_> = std::fs::read_dir(&exports).expect("exports").collect();
    assert!(entries.is_empty(), "only the folder was created");
    drop(picker);
    settle();

    // 5. A folder you can't write to: Save is refused; Close answers other.
    let mut extra = options(&[("current_name", "x.txt".to_variant())]);
    extra.insert(
        "current_folder".to_string(),
        folder_bytes(std::path::Path::new("/usr")),
    );
    let reply = chooser_call(&bus, "SaveFile", extra);
    let picker = one_picker();
    settle();
    assert!(!picker.save_enabled(), "Save is disabled in /usr");
    assert!(
        picker
            .texts()
            .iter()
            .any(|text| text == "You can't save in this folder"),
        "{:?}",
        picker.texts()
    );
    let closed = close(&bus, HANDLE);
    let (response, _) = reply_parts(&bus.wait(&reply).expect("a Close reply"));
    assert_eq!(response, RESPONSE_OTHER);
    let _ = bus.wait(&closed);
    drop(picker);
    settle();
    assert!(
        picker::testing::live().is_empty(),
        "Close took the window down"
    );

    // 7. SaveFiles: the summary, then Keep Both and Replace.
    for (choice, expected) in [
        ("keep-both", ["a.txt", "b (2).txt", "a (2).txt"]),
        ("replace", ["a.txt", "b.txt", "a (2).txt"]),
    ] {
        let files: Vec<Vec<u8>> = ["a.txt", "b.txt", "a.txt"]
            .iter()
            .map(|name| {
                let mut bytes = name.as_bytes().to_vec();
                bytes.push(0);
                bytes
            })
            .collect();
        let mut extra = HashMap::new();
        extra.insert("current_folder".to_string(), folder_bytes(&multi));
        extra.insert("files".to_string(), files.to_variant());
        let reply = chooser_call(&bus, "SaveFiles", extra);
        let picker = one_picker();
        settle();
        assert!(
            picker
                .texts()
                .iter()
                .any(|text| text.starts_with("Saving 3 files: a.txt, b.txt, a.txt")),
            "{:?}",
            picker.texts()
        );
        assert_eq!(picker.name_text(), None, "SaveFiles has no Name entry");
        let _ = picker.accept();
        settle();
        let body = picker.alert_body().expect("the collision alert");
        assert!(body.contains("b.txt"), "{body}");
        picker.respond_to_alert(choice);
        let (response, results) = reply_parts(&bus.wait(&reply).expect("a SaveFiles reply"));
        assert_eq!(response, RESPONSE_SUCCESS);
        let expected: Vec<String> = expected.iter().map(|name| uri(&multi.join(name))).collect();
        assert_eq!(uris(&results), expected, "{choice}");
        drop(picker);
        settle();
    }
    assert_eq!(
        std::fs::read_dir(&multi).expect("multi").count(),
        1,
        "SaveFiles wrote nothing"
    );
}
