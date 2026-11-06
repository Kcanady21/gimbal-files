// SPDX-License-Identifier: GPL-3.0-or-later
//! The `org.freedesktop.FileManager1` service.
//!
//! Browsers, the desktop portal, terminals and IDEs do not open a folder
//! through the MIME default. They call this D-Bus interface instead. Gimbal
//! exports the object and asks for the name only while its Settings switch is
//! on, or while the bus started it with `--gapplication-service`.
//!
//! The object is registered and the name requested in GApplication's
//! `startup`, because a service launch never activates. The first call builds
//! the process and restores the session exactly like a first `open` does.
//! The methods hand their paths to the same `open_arguments` path as launch
//! arguments, so folder/file grouping stays identical everywhere.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use gtk::glib::variant::ToVariant;

use fm_core::filemanager1::{Refused, uris_to_paths};

/// The well-known name and the object path freedesktop.org's interface uses.
pub const BUS_NAME: &str = "org.freedesktop.FileManager1";
pub const OBJECT_PATH: &str = "/org/freedesktop/FileManager1";
pub const INTERFACE: &str = "org.freedesktop.FileManager1";

/// A call that arrives within this many seconds of the previous one is part
/// of the same burst when the bus started Gimbal with the switch off.
const SERVICE_GRACE_SECONDS: u32 = 2;

/// The three reviewed methods. Dolphin's KDE-only `SortOrderForUrl` is
/// deliberately absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Folders,
    Items,
    ItemProperties,
}

impl Method {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "ShowFolders" => Some(Self::Folders),
            "ShowItems" => Some(Self::Items),
            "ShowItemProperties" => Some(Self::ItemProperties),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Folders => "ShowFolders",
            Self::Items => "ShowItems",
            Self::ItemProperties => "ShowItemProperties",
        }
    }
}

/// The pure call-to-arguments mapping. Every method hands the accepted paths
/// to `open_arguments`; `group_arguments` turns a folder into a folder tab and
/// a file into its parent tab with the item selected. `Folders` therefore
/// needs no separate filter: a path that is not a folder is exactly the
/// `Items` case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallPlan {
    pub method: Method,
    pub paths: Vec<PathBuf>,
    pub refused: Vec<Refused>,
}

impl CallPlan {
    pub fn properties_unsupported(&self) -> bool {
        self.method == Method::ItemProperties
    }
}

pub fn plan(method: Method, uris: &[String]) -> CallPlan {
    let (paths, refused) = uris_to_paths(uris);
    CallPlan {
        method,
        paths,
        refused,
    }
}

/// What the Settings row shows. `Waiting` carries the current owner's
/// `/proc/<pid>/comm` name, or `another app` when it cannot be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Owned,
    Waiting(String),
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub enabled: bool,
    pub state: State,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            enabled: false,
            state: State::Off,
        }
    }
}

impl Status {
    pub fn text(&self) -> String {
        text::status(self)
    }
}

/// Every user-facing and log string for this feature lives here.
pub mod text {
    use super::{BUS_NAME, State, Status};

    pub const INTEGRATION: &str = "Integration";
    pub const SHOW_IN_FOLDER: &str = "Open \"Show in folder\" in Gimbal";
    pub const SUBTITLE: &str = "For browsers and other apps that ask the desktop to show a file.";
    pub const ON: &str = "On";
    pub const OFF: &str = "Off";
    pub const ANOTHER_APP: &str = "another app";
    pub const SERVICE_OFF_LOG: &str = "started by the bus with the switch off; served one call";
    pub const PROPERTIES_NOT_SUPPORTED: &str = "properties are not supported; showing the item";
    pub const MALFORMED_CALL: &str = "Expected an array of URIs and a startup id.";
    pub const UNKNOWN_METHOD: &str = "Unknown FileManager1 method.";

    pub fn waiting(owner: &str) -> String {
        format!("Waiting: {owner} is handling it until it closes")
    }

    pub fn status(status: &Status) -> String {
        if !status.enabled {
            return OFF.to_string();
        }
        match &status.state {
            State::Owned => ON.to_string(),
            State::Waiting(owner) => waiting(owner),
            State::Off => OFF.to_string(),
        }
    }

    pub fn state_log(state: &State) -> String {
        match state {
            State::Owned => format!("now owns {BUS_NAME}"),
            State::Waiting(owner) => format!("waiting; {owner} is handling it"),
            State::Off => "off".to_string(),
        }
    }
}

const INTROSPECTION_XML: &str = r#"<node>
  <interface name="org.freedesktop.FileManager1">
    <method name="ShowFolders">
      <arg type="as" name="uris" direction="in"/>
      <arg type="s" name="startup_id" direction="in"/>
    </method>
    <method name="ShowItems">
      <arg type="as" name="uris" direction="in"/>
      <arg type="s" name="startup_id" direction="in"/>
    </method>
    <method name="ShowItemProperties">
      <arg type="as" name="uris" direction="in"/>
      <arg type="s" name="startup_id" direction="in"/>
    </method>
  </interface>
</node>"#;

struct Runtime {
    application: adw::Application,
    connection: gio::DBusConnection,
    enabled: Cell<bool>,
    /// The bus started us with the switch off. Serve the call that started us,
    /// then release the name after the grace period.
    service_off: Cell<bool>,
    first_service_call: Cell<bool>,
    state: RefCell<State>,
    owner: RefCell<Option<gio::OwnerId>>,
    registration: RefCell<Option<gio::RegistrationId>>,
    /// Invalidates a pending grace timeout when the switch changes.
    grace_generation: Cell<u64>,
}

thread_local! {
    static RUNTIME: RefCell<Option<Rc<Runtime>>> = const { RefCell::new(None) };
}

/// Read the switch without building the process. `startup` runs before the
/// first `activate`, so `Process::start` has not loaded the config yet.
fn config_enabled() -> bool {
    let Some(path) = fm_core::config::default_config_path() else {
        return false;
    };
    fm_core::config::ConfigFile::load(&path)
        .0
        .config()
        .integration
        .show_in_folder
}

/// Called from `main.rs`'s `startup` handler, on the primary instance only.
///
/// With the switch on, or when `--gapplication-service` started us, export
/// the object and ask for the name before the first call can arrive.
pub fn start(application: &adw::Application) {
    let enabled = config_enabled();
    let service = application
        .flags()
        .contains(gio::ApplicationFlags::IS_SERVICE);

    let Some(connection) = application.dbus_connection() else {
        crate::log!(
            "filemanager1",
            "no session-bus connection; the FileManager1 service is off"
        );
        return;
    };

    let runtime = Rc::new(Runtime {
        application: application.clone(),
        connection,
        enabled: Cell::new(enabled),
        service_off: Cell::new(service && !enabled),
        first_service_call: Cell::new(true),
        state: RefCell::new(State::Off),
        owner: RefCell::new(None),
        registration: RefCell::new(None),
        grace_generation: Cell::new(0),
    });
    RUNTIME.with(|slot| *slot.borrow_mut() = Some(runtime.clone()));

    if enabled || service {
        runtime.install_and_own();
    } else {
        crate::log!("filemanager1", "off");
    }

    if service && !enabled {
        // This is not "served one call" yet: that line belongs after the
        // first real call, and the name is held only until then.
        crate::log!(
            "filemanager1",
            "started by the bus with the switch off; waiting for its call"
        );
    }
}

/// The current Settings row state. A process that never started the service
/// (development builds, no bus) reports Off.
pub fn status() -> Status {
    RUNTIME.with(|slot| match slot.borrow().as_ref() {
        Some(runtime) => Status {
            enabled: runtime.enabled.get(),
            state: runtime.state.borrow().clone(),
        },
        None => Status::default(),
    })
}

/// Apply the Settings switch after the config change was saved.
pub fn apply(enabled: bool) {
    let runtime = RUNTIME.with(|slot| slot.borrow().clone());
    let Some(runtime) = runtime else {
        crate::log!(
            "filemanager1",
            "the switch changed, but this process has no FileManager1 service"
        );
        return;
    };

    // Any pending service-off release is superseded by this decision.
    runtime
        .grace_generation
        .set(runtime.grace_generation.get() + 1);
    runtime.service_off.set(false);
    runtime.enabled.set(enabled);

    if enabled {
        runtime.install_and_own();
        runtime.schedule_owner_refresh();
    } else {
        runtime.release();
    }
    refresh_all();
}

/// Push the current state into every open Settings popover.
pub fn refresh_all() {
    if let Some(process) = crate::process() {
        process.refresh_filemanager1_status_all();
    }
}

impl Runtime {
    fn install_and_own(self: &Rc<Self>) {
        if self.registration.borrow().is_some() {
            return;
        }

        let node = match gio::DBusNodeInfo::for_xml(INTROSPECTION_XML) {
            Ok(node) => node,
            Err(error) => {
                crate::log!("filemanager1", "bad introspection XML: {error}");
                return;
            }
        };
        let Some(interface) = node.lookup_interface(INTERFACE) else {
            crate::log!("filemanager1", "the introspection XML has no {INTERFACE}");
            return;
        };

        let weak = Rc::downgrade(self);
        let builder = self
            .connection
            .register_object(OBJECT_PATH, &interface)
            .method_call(
                move |connection,
                      sender,
                      _object_path,
                      _interface,
                      method,
                      parameters,
                      invocation| {
                    match weak.upgrade() {
                        Some(runtime) => {
                            runtime.handle_call(
                                &connection,
                                sender,
                                method,
                                parameters,
                                invocation,
                            );
                        }
                        None => invocation.return_dbus_error(
                            "org.freedesktop.DBus.Error.Failed",
                            "Gimbal Files is shutting down.",
                        ),
                    }
                },
            );
        match builder.build() {
            Ok(registration) => *self.registration.borrow_mut() = Some(registration),
            Err(error) => {
                crate::log!("filemanager1", "could not export {OBJECT_PATH}: {error}");
                return;
            }
        }

        let weak = Rc::downgrade(self);
        let acquired = move |_connection: gio::DBusConnection, _name: &str| {
            if let Some(runtime) = weak.upgrade() {
                runtime.set_state(State::Owned);
            }
        };
        let weak = Rc::downgrade(self);
        let lost = move |_connection: gio::DBusConnection, _name: &str| {
            if let Some(runtime) = weak.upgrade() {
                runtime.schedule_owner_refresh();
            }
        };
        let owner = gio::bus_own_name_on_connection(
            &self.connection,
            BUS_NAME,
            gio::BusNameOwnerFlags::ALLOW_REPLACEMENT | gio::BusNameOwnerFlags::REPLACE,
            acquired,
            lost,
        );
        *self.owner.borrow_mut() = Some(owner);

        // GIO queues the request when the current owner does not allow
        // replacement. Ask the bus who owns the name now so the Settings row
        // can say who is handling it until it closes.
        self.set_state(State::Waiting(text::ANOTHER_APP.to_string()));
        self.schedule_owner_refresh();
    }

    fn release(&self) {
        self.grace_generation.set(self.grace_generation.get() + 1);
        if let Some(owner) = self.owner.borrow_mut().take() {
            gio::bus_unown_name(owner);
        }
        if let Some(registration) = self.registration.borrow_mut().take()
            && let Err(error) = self.connection.unregister_object(registration)
        {
            crate::log!("filemanager1", "could not unexport {OBJECT_PATH}: {error}");
        }
        self.set_state(State::Off);
    }

    fn set_state(&self, state: State) {
        let changed = {
            let mut current = self.state.borrow_mut();
            if *current == state {
                false
            } else {
                *current = state.clone();
                true
            }
        };
        if changed {
            crate::log!("filemanager1", "{}", text::state_log(&state));
            refresh_all();
        }
    }

    /// Ask the bus for the name owner after GIO has had a chance to request
    /// it. Doing this from an idle also keeps a nested main-context iteration
    /// out of the `RequestName` callback.
    fn schedule_owner_refresh(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(runtime) = weak.upgrade() {
                runtime.refresh_owner_state();
            }
        });
    }

    fn refresh_owner_state(&self) {
        if !self.enabled.get() && !self.service_off.get() {
            return;
        }
        let Some(unique) = self.connection.unique_name() else {
            self.set_state(State::Off);
            return;
        };
        match self.name_owner() {
            Ok(owner) if owner == unique => self.set_state(State::Owned),
            Ok(owner) => {
                let command = self
                    .owner_command(&owner)
                    .unwrap_or_else(|| text::ANOTHER_APP.to_string());
                self.set_state(State::Waiting(command));
            }
            Err(_) => self.set_state(State::Waiting(text::ANOTHER_APP.to_string())),
        }
    }

    fn name_owner(&self) -> Result<String, glib::Error> {
        let parameters = (BUS_NAME,).to_variant();
        let reply = self.connection.call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "GetNameOwner",
            Some(&parameters),
            Some(glib::VariantTy::new("(s)").expect("valid signature")),
            gio::DBusCallFlags::NONE,
            500,
            None::<&gio::Cancellable>,
        )?;
        reply
            .get::<(String,)>()
            .map(|(owner,)| owner)
            .ok_or_else(|| glib::Error::new(gio::IOErrorEnum::Failed, "unexpected reply"))
    }

    fn owner_command(&self, unique_name: &str) -> Option<String> {
        let parameters = (unique_name,).to_variant();
        let reply = self
            .connection
            .call_sync(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "GetConnectionUnixProcessID",
                Some(&parameters),
                Some(glib::VariantTy::new("(u)").expect("valid signature")),
                gio::DBusCallFlags::NONE,
                500,
                None::<&gio::Cancellable>,
            )
            .ok()?;
        let (pid,) = reply.get::<(u32,)>()?;
        let command = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        let command = command.trim();
        (!command.is_empty()).then(|| command.to_string())
    }

    fn handle_call(
        self: &Rc<Self>,
        _connection: &gio::DBusConnection,
        sender: Option<&str>,
        method_name: &str,
        parameters: glib::Variant,
        invocation: gio::DBusMethodInvocation,
    ) {
        let caller = sender.unwrap_or("unknown caller");
        let Some(method) = Method::parse(method_name) else {
            crate::log!(
                "filemanager1",
                "{caller} called {method_name}: unknown method"
            );
            invocation.return_dbus_error(
                "org.freedesktop.DBus.Error.UnknownMethod",
                text::UNKNOWN_METHOD,
            );
            self.after_call_served();
            return;
        };

        let Some((uris, startup_id)) = parameters.get::<(Vec<String>, String)>() else {
            crate::log!(
                "filemanager1",
                "{caller} called {method_name}: malformed signature"
            );
            invocation.return_dbus_error(
                "org.freedesktop.DBus.Error.InvalidArgs",
                text::MALFORMED_CALL,
            );
            self.after_call_served();
            return;
        };

        let plan = plan(method, &uris);
        crate::log!(
            "filemanager1",
            "{}",
            fm_core::filemanager1::call_log_line(
                caller,
                method.as_str(),
                plan.paths.len(),
                &plan.refused
            )
        );
        if plan.properties_unsupported() {
            crate::log!("filemanager1", "{}", text::PROPERTIES_NOT_SUPPORTED);
        }

        // Reply before any GTK work: creating the process and restoring the
        // session is the slow part, and the caller only expects an empty
        // result.
        invocation.return_value(None);

        if !plan.paths.is_empty() {
            let application = self.application.clone();
            let startup = (!startup_id.is_empty()).then_some(startup_id);
            glib::idle_add_local_once(move || {
                let (process, created) = crate::ensure_process(&application);
                process.open_arguments_with_startup(&plan.paths, created, startup.as_deref());
            });
        }

        self.after_call_served();
    }

    fn after_call_served(self: &Rc<Self>) {
        if !self.service_off.get() {
            return;
        }
        if self.first_service_call.replace(false) {
            crate::log!("filemanager1", "{}", text::SERVICE_OFF_LOG);
        }
        self.arm_service_grace();
    }

    fn arm_service_grace(self: &Rc<Self>) {
        let generation = self.grace_generation.get() + 1;
        self.grace_generation.set(generation);
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local_once(SERVICE_GRACE_SECONDS, move || {
            if let Some(runtime) = weak.upgrade()
                && runtime.service_off.get()
                && runtime.grace_generation.get() == generation
            {
                crate::log!(
                    "filemanager1",
                    "releasing the name after the bus-started call; the window keeps Gimbal running"
                );
                runtime.release();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uris(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn every_method_hands_the_same_paths_to_open_arguments() {
        let values = uris(&[
            "file:///tmp/gimbal-p2x/a%20b%20%28c%29",
            "file:///tmp/gimbal-p2x/a%20b%20%28c%29/file.txt",
        ]);
        for method in [Method::Folders, Method::Items, Method::ItemProperties] {
            let plan = plan(method, &values);
            assert_eq!(
                plan.paths,
                vec![
                    PathBuf::from("/tmp/gimbal-p2x/a b (c)"),
                    PathBuf::from("/tmp/gimbal-p2x/a b (c)/file.txt"),
                ],
                "{method:?}"
            );
            assert!(plan.refused.is_empty());
        }
    }

    #[test]
    fn only_show_item_properties_reports_the_missing_window() {
        let properties = plan(Method::ItemProperties, &[]);
        assert!(properties.properties_unsupported());
        assert!(!plan(Method::Items, &[]).properties_unsupported());
        assert!(!plan(Method::Folders, &[]).properties_unsupported());
    }

    #[test]
    fn refused_uris_are_kept_out_of_the_open_arguments() {
        let plan = plan(
            Method::Items,
            &uris(&["sftp://host/tmp/a", "file:///tmp/a"]),
        );
        assert_eq!(plan.paths, vec![PathBuf::from("/tmp/a")]);
        assert_eq!(plan.refused.len(), 1);
    }

    #[test]
    fn the_status_text_is_on_waiting_or_off() {
        assert_eq!(
            Status {
                enabled: true,
                state: State::Owned,
            }
            .text(),
            "On"
        );
        assert_eq!(
            Status {
                enabled: true,
                state: State::Waiting("dolphin".to_string()),
            }
            .text(),
            "Waiting: dolphin is handling it until it closes"
        );
        assert_eq!(Status::default().text(), "Off");
        // A bus-started process with the switch off may own the name for one
        // call; the row still reports the setting as Off.
        assert_eq!(
            Status {
                enabled: false,
                state: State::Owned,
            }
            .text(),
            "Off"
        );
    }

    #[test]
    fn every_reviewed_method_parses_and_kde_only_ones_do_not() {
        assert_eq!(Method::parse("ShowFolders"), Some(Method::Folders));
        assert_eq!(Method::parse("ShowItems"), Some(Method::Items));
        assert_eq!(
            Method::parse("ShowItemProperties"),
            Some(Method::ItemProperties)
        );
        assert_eq!(Method::parse("SortOrderForUrl"), None);
        assert_eq!(Method::parse("showitems"), None);
    }

    struct NoClipboard;

    impl crate::fileops::clipboard::Reader for NoClipboard {
        fn read(
            &self,
            _display: Option<&gtk::gdk::Display>,
            done: Box<dyn FnOnce(Result<crate::fileops::clipboard::ClipboardPayload, String>)>,
        ) {
            done(Err("the Settings switch test has no clipboard".to_string()));
        }
    }

    /// The Settings switch goes Registry -> the real UiHandler -> Process. It
    /// is a System command, so it needs its own route to the config write; it
    /// once fell through to "not implemented by this version".
    #[test]
    fn the_settings_switch_saves_show_in_folder_and_only_the_ui_may_use_it() {
        use fm_core::command::{Caller, Invocation, Outcome, Registry};

        let root = PathBuf::from(format!(
            "/tmp/gimbal-p2x-switch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|time| time.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(root.join("home")).expect("scratch home");
        let config = root.join("config.toml");
        std::fs::write(&config, "[integration]\nshow_in_folder = false\n").expect("config");
        let process =
            crate::app::Process::headless(config.clone(), root.join("home"), Rc::new(NoClipboard));
        let window = process.seed_window(root.join("home"));

        let dispatch = |caller: Caller, enabled: &str| {
            let mut handler = crate::commands::UiHandler {
                caller,
                window,
                process: Rc::downgrade(&process),
            };
            Registry::new().dispatch(
                caller,
                &Invocation::new(
                    "integration.set-show-in-folder",
                    vec![("enabled".to_string(), enabled.to_string())],
                ),
                &mut handler,
            )
        };

        assert_eq!(dispatch(Caller::Ui, "true"), Outcome::Done);
        let saved = std::fs::read_to_string(&config).expect("config");
        assert!(saved.contains("show_in_folder = true"), "{saved}");
        assert!(process.config.borrow().config().integration.show_in_folder);

        assert_eq!(dispatch(Caller::Ui, "false"), Outcome::Done);
        let saved = std::fs::read_to_string(&config).expect("config");
        assert!(saved.contains("show_in_folder = false"), "{saved}");

        assert_ne!(dispatch(Caller::Agent, "true"), Outcome::Done);
        let saved = std::fs::read_to_string(&config).expect("config");
        assert!(saved.contains("show_in_folder = false"), "{saved}");

        let _ = std::fs::remove_dir_all(&root);
    }
}
