// SPDX-License-Identifier: GPL-3.0-or-later
//! The `org.freedesktop.impl.portal.FileChooser` backend.
//!
//! xdg-desktop-portal calls this process over D-Bus when an application asks
//! for an Open dialog. The method call carries a Request object path; the
//! answer arrives later, as the return value of that same call, so the
//! invocation is held while the dialog is up. This module owns the wire
//! contract and the response bookkeeping; [`super::picker`] owns the window.
//!
//! Nothing here touches GTK: the service is testable over a peer-to-peer
//! GDBus connection with a fake dialog host.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

/// The name Gimbal claims when the picker runs as a portal backend.
pub const BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.gimbal";

/// Where every portal backend exports its interfaces.
pub const OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";

pub const INTERFACE: &str = "org.freedesktop.impl.portal.FileChooser";
pub const REQUEST_INTERFACE: &str = "org.freedesktop.impl.portal.Request";

/// The user picked something.
pub const RESPONSE_SUCCESS: u32 = 0;
/// The user cancelled (Escape, Close, the titlebar's close button).
pub const RESPONSE_CANCELLED: u32 = 1;
/// The backend could not show the dialog, or the dialog went away without an
/// answer.
pub const RESPONSE_OTHER: u32 = 2;

// The methods Gimbal answers. Copied from xdg-desktop-portal 1.21's
// xdp-gnome/data/org.freedesktop.impl.portal.FileChooser.xml; the Qt
// annotations and the documentation comments are dropped. OpenFile is F1;
// SaveFile and SaveFiles are answered "not available" until F3.
const FILE_CHOOSER_XML: &str = r#"
<node>
  <interface name="org.freedesktop.impl.portal.FileChooser">
    <method name="OpenFile">
      <arg type="o" name="handle" direction="in"/>
      <arg type="s" name="app_id" direction="in"/>
      <arg type="s" name="parent_window" direction="in"/>
      <arg type="s" name="title" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="u" name="response" direction="out"/>
      <arg type="a{sv}" name="results" direction="out"/>
    </method>
    <method name="SaveFile">
      <arg type="o" name="handle" direction="in"/>
      <arg type="s" name="app_id" direction="in"/>
      <arg type="s" name="parent_window" direction="in"/>
      <arg type="s" name="title" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="u" name="response" direction="out"/>
      <arg type="a{sv}" name="results" direction="out"/>
    </method>
    <method name="SaveFiles">
      <arg type="o" name="handle" direction="in"/>
      <arg type="s" name="app_id" direction="in"/>
      <arg type="s" name="parent_window" direction="in"/>
      <arg type="s" name="title" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="u" name="response" direction="out"/>
      <arg type="a{sv}" name="results" direction="out"/>
    </method>
  </interface>
</node>
"#;

// The shared Request object. Portal exports one per method call at the path
// the caller sent; Close aborts the interaction.
const REQUEST_XML: &str = r#"
<node>
  <interface name="org.freedesktop.impl.portal.Request">
    <method name="Close"/>
  </interface>
</node>
"#;

/// The FileChooser methods F1 recognises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    OpenFile,
    SaveFile,
    SaveFiles,
}

impl Method {
    pub fn from_dbus(name: &str) -> Option<Self> {
        match name {
            "OpenFile" => Some(Self::OpenFile),
            "SaveFile" => Some(Self::SaveFile),
            "SaveFiles" => Some(Self::SaveFiles),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenFile => "OpenFile",
            Self::SaveFile => "SaveFile",
            Self::SaveFiles => "SaveFiles",
        }
    }
}

/// The options F1 understands from an OpenFile call. Everything else is
/// accepted and ignored: a chooser that refused an unfamiliar key would break
/// applications that only wanted an ordinary dialog.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OpenOptions {
    /// The accept button's label, with mnemonic underlines allowed.
    pub accept_label: Option<String>,
    /// The suggested starting folder. It is only used if it exists and is a
    /// directory: the frontend's suggestion never becomes a failed dialog.
    pub current_folder: Option<PathBuf>,
    /// Known option keys F1 does not implement. Each is logged once; unknown
    /// keys are ignored silently.
    pub ignored: Vec<&'static str>,
}

/// One incoming call, after its arguments have been split into the parts the
/// picker needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub method: Method,
    pub app_id: String,
    pub title: Option<String>,
    pub options: OpenOptions,
}

impl Call {
    /// The accept button's label: the caller's, or "Open".
    pub fn accept_label(&self) -> &str {
        self.options
            .accept_label
            .as_deref()
            .filter(|label| !label.is_empty())
            .unwrap_or("Open")
    }

    /// The window title: the caller's, or the app's own name.
    pub fn title(&self) -> &str {
        self.title
            .as_deref()
            .filter(|title| !title.is_empty())
            .unwrap_or("Gimbal Files")
    }
}

/// The known OpenFile option keys. `modal`, `multiple`, `directory`,
/// `filters`, `current_filter` and `choices` are known but not implemented
/// yet, so they are named in the log.
const KNOWN_OPEN_KEYS: &[&str] = &[
    "accept_label",
    "modal",
    "multiple",
    "directory",
    "filters",
    "current_filter",
    "choices",
    "current_folder",
];

/// Parse an OpenFile options vardict. Only `accept_label` and `current_folder`
/// change the dialog; the rest are recorded for the log.
pub fn parse_open_options(options: &HashMap<String, glib::Variant>) -> OpenOptions {
    let accept_label = options
        .get("accept_label")
        .and_then(|value| value.get::<String>())
        .filter(|label| !label.is_empty());
    let current_folder = options
        .get("current_folder")
        .and_then(|value| value.get::<Vec<u8>>())
        .and_then(|bytes| folder_from_bytes(&bytes));
    let ignored = KNOWN_OPEN_KEYS
        .iter()
        .copied()
        .filter(|key| {
            *key != "accept_label" && *key != "current_folder" && options.contains_key(*key)
        })
        .collect();
    OpenOptions {
        accept_label,
        current_folder,
        ignored,
    }
}

/// The `current_folder` option is a `ay` holding a NUL-terminated path. An
/// empty value, a missing final NUL and a non-UTF-8 name are all handled:
/// bytes stop at the first NUL, and the result must be absolute.
pub fn folder_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let bytes = &bytes[..end];
    if bytes.is_empty() {
        return None;
    }
    let path = PathBuf::from(OsString::from_vec(bytes.to_vec()));
    path.is_absolute().then_some(path)
}

/// The starting folder for a dialog: the suggested one when it exists and is a
/// directory, otherwise the user's home.
pub fn starting_folder(suggested: Option<&Path>, home: &Path) -> PathBuf {
    suggested
        .filter(|path| path.is_dir())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.to_path_buf())
}

/// The empty `a{sv}` a cancelled or refused call returns.
pub fn empty_results() -> HashMap<String, glib::Variant> {
    HashMap::new()
}

/// The `a{sv}` a successful call returns: one `file://` URI and an empty
/// `choices` array. Paths are encoded by GIO, so spaces and non-ASCII names
/// come out percent-encoded exactly as the portal expects.
pub fn results_for(path: &Path) -> HashMap<String, glib::Variant> {
    let mut results = HashMap::new();
    results.insert(
        "uris".to_string(),
        vec![gio::File::for_path(path).uri().to_string()].to_variant(),
    );
    results.insert(
        "choices".to_string(),
        Vec::<(String, String)>::new().to_variant(),
    );
    results
}

/// What the picker must do when a call arrives. Kept as a trait so the wire
/// contract can be tested without GTK, and so the service never depends on the
/// picker module.
pub trait DialogHost {
    /// Build and present a dialog for this request. On `Err` the service
    /// answers `RESPONSE_OTHER`; the request is not left hanging. The host
    /// installs the request's completion hook, which closes its window and
    /// releases the application hold.
    fn open(&self, request: Rc<Request>, call: &Call) -> Result<(), String>;
}

/// One method call's Request object and its held invocation.
pub struct Request {
    handle: String,
    app_id: String,
    title: String,
    invocation: RefCell<Option<gio::DBusMethodInvocation>>,
    connection: gio::DBusConnection,
    registration: RefCell<Option<gio::RegistrationId>>,
    finished: Cell<bool>,
    on_done: RefCell<Option<Box<dyn FnOnce()>>>,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request")
            .field("handle", &self.handle)
            .field("app_id", &self.app_id)
            .field("finished", &self.finished.get())
            .finish()
    }
}

impl Request {
    /// Make the caller's Request object, at the exact path it sent. The object
    /// lives as long as the dialog does.
    fn new(
        connection: &gio::DBusConnection,
        handle: &str,
        invocation: &gio::DBusMethodInvocation,
        app_id: &str,
        title: &str,
    ) -> Result<Rc<Self>, String> {
        let request = Rc::new(Self {
            handle: handle.to_string(),
            app_id: app_id.to_string(),
            title: title.to_string(),
            invocation: RefCell::new(Some(invocation.clone())),
            connection: connection.clone(),
            registration: RefCell::new(None),
            finished: Cell::new(false),
            on_done: RefCell::new(None),
        });
        let node = gio::DBusNodeInfo::for_xml(REQUEST_XML)
            .map_err(|error| format!("bad Request introspection: {error}"))?;
        let interface = node
            .lookup_interface(REQUEST_INTERFACE)
            .ok_or_else(|| "the Request node has no interface".to_string())?;
        let weak = Rc::downgrade(&request);
        let id = connection
            .register_object(handle, &interface)
            .method_call(move |_, _, _, _, _, _, invocation| {
                // The portal aborted the interaction. Answer `OTHER`, never
                // `CANCELLED`: this was not the user's choice.
                if let Some(request) = weak.upgrade() {
                    crate::log!("portal", "Close for {}", request.handle);
                    request.complete(RESPONSE_OTHER, empty_results());
                }
                invocation.return_value(None);
            })
            .build()
            .map_err(|error| format!("cannot export request {handle}: {error}"))?;
        *request.registration.borrow_mut() = Some(id);
        Ok(request)
    }

    /// The application hold is released by this, and the dialog's window is
    /// closed. Set before the dialog is presented.
    pub fn set_on_done(&self, done: impl FnOnce() + 'static) {
        *self.on_done.borrow_mut() = Some(Box::new(done));
    }

    /// Answer the method call exactly once. A second call (a Close after a
    /// click, a destroy after Cancel) is a no-op with a log line, so the
    /// frontend never sees two replies.
    pub fn complete(
        self: &Rc<Self>,
        response: u32,
        results: HashMap<String, glib::Variant>,
    ) -> bool {
        if self.finished.replace(true) {
            crate::log!(
                "portal",
                "{} for {} was already answered",
                response_name(response),
                self.handle
            );
            return false;
        }
        if let Some(invocation) = self.invocation.borrow_mut().take() {
            let value = (response, results).to_variant();
            invocation.return_value(Some(&value));
        }
        if let Some(id) = self.registration.borrow_mut().take() {
            let _ = self.connection.unregister_object(id);
        }
        crate::log!(
            "portal",
            "{} for {} ({}): {}",
            response_name(response),
            self.handle,
            if self.app_id.is_empty() {
                "no app id"
            } else {
                &self.app_id
            },
            if self.title.is_empty() {
                "no title"
            } else {
                &self.title
            }
        );
        if let Some(done) = self.on_done.borrow_mut().take() {
            done();
        }
        true
    }

    /// Answer with success and one path.
    #[cfg(test)]
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    /// Answer with success and one path.
    pub fn accept(self: &Rc<Self>, path: &Path) -> bool {
        crate::log!("portal", "selected {}", path.display());
        self.complete(RESPONSE_SUCCESS, results_for(path))
    }
}

/// The response number as the log names it.
pub fn response_name(response: u32) -> &'static str {
    match response {
        RESPONSE_SUCCESS => "success",
        RESPONSE_CANCELLED => "user-cancelled",
        _ => "other",
    }
}

/// The exported FileChooser object and the one dialog host it talks to.
pub struct Service {
    host: RefCell<Option<Rc<dyn DialogHost>>>,
    connection: RefCell<Option<gio::DBusConnection>>,
    registration: RefCell<Option<gio::RegistrationId>>,
    owner: RefCell<Option<gio::OwnerId>>,
}

impl Service {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            host: RefCell::new(None),
            connection: RefCell::new(None),
            registration: RefCell::new(None),
            owner: RefCell::new(None),
        })
    }

    pub fn set_host(&self, host: Rc<dyn DialogHost>) {
        *self.host.borrow_mut() = Some(host);
    }

    /// Export the FileChooser interface on `connection`. Called once, from the
    /// application's startup, so a call can never arrive before the object
    /// exists.
    pub fn install(self: &Rc<Self>, connection: &gio::DBusConnection) -> bool {
        if self.registration.borrow().is_some() {
            return true;
        }
        let node = match gio::DBusNodeInfo::for_xml(FILE_CHOOSER_XML) {
            Ok(node) => node,
            Err(error) => {
                crate::log!("portal", "bad FileChooser introspection: {error}");
                return false;
            }
        };
        let Some(interface) = node.lookup_interface(INTERFACE) else {
            crate::log!("portal", "the introspection XML has no {INTERFACE}");
            return false;
        };
        let this = self.clone();
        match connection
            .register_object(OBJECT_PATH, &interface)
            .method_call(
                move |connection, sender, _path, _interface, method, parameters, invocation| {
                    this.handle_call(&connection, sender, method, &parameters, invocation)
                },
            )
            .build()
        {
            Ok(id) => {
                *self.registration.borrow_mut() = Some(id);
                *self.connection.borrow_mut() = Some(connection.clone());
                crate::log!("portal", "exported {INTERFACE} at {OBJECT_PATH}");
                true
            }
            Err(error) => {
                crate::log!("portal", "cannot export {INTERFACE}: {error}");
                false
            }
        }
    }

    /// Claim the backend's bus name. Failure is logged, not fatal: the picker
    /// still has to run so its window can be inspected.
    pub fn own_name(self: &Rc<Self>, connection: &gio::DBusConnection) {
        if self.owner.borrow().is_some() {
            return;
        }
        let owner = gio::bus_own_name_on_connection(
            connection,
            BUS_NAME,
            gio::BusNameOwnerFlags::NONE,
            |_, _| crate::log!("portal", "owning {BUS_NAME}"),
            |_, _| crate::log!("portal", "lost {BUS_NAME}"),
        );
        *self.owner.borrow_mut() = Some(owner);
    }

    /// One incoming FileChooser method call.
    fn handle_call(
        self: &Rc<Self>,
        connection: &gio::DBusConnection,
        sender: Option<&str>,
        method: &str,
        parameters: &glib::Variant,
        invocation: gio::DBusMethodInvocation,
    ) {
        let Some(method) = Method::from_dbus(method) else {
            invocation.return_dbus_error(
                "org.freedesktop.DBus.Error.UnknownMethod",
                "Gimbal has no such FileChooser method",
            );
            return;
        };
        // The handle is an object path (`o`), not a string: it must be read as
        // one or GVariant rejects the whole call.
        type OpenCall = (
            glib::variant::ObjectPath,
            String,
            String,
            String,
            HashMap<String, glib::Variant>,
        );
        let parsed = parameters.get::<OpenCall>();
        let Some((handle, app_id, parent_window, title, options)) = parsed else {
            crate::log!(
                "portal",
                "{} with arguments Gimbal does not understand",
                method.as_str()
            );
            invocation.return_dbus_error(
                "org.freedesktop.DBus.Error.InvalidArgs",
                "unexpected FileChooser arguments",
            );
            return;
        };
        crate::log!(
            "portal",
            "{} from app {:?} sender {:?} handle {} title {:?} parent {:?}",
            method.as_str(),
            app_id,
            sender.unwrap_or("unknown"),
            handle,
            title,
            parent_window
        );

        if method != Method::OpenFile {
            // F3 implements saving. F1 answers a plain refusal, so the
            // frontend falls back rather than waiting forever.
            crate::log!(
                "portal",
                "{} is not implemented yet; answering other",
                method.as_str()
            );
            invocation.return_value(Some(&(RESPONSE_OTHER, empty_results()).to_variant()));
            return;
        }

        let options = parse_open_options(&options);
        if !options.ignored.is_empty() {
            crate::log!(
                "portal",
                "OpenFile ignores option(s) {}",
                options.ignored.join(", ")
            );
        }
        let handle = handle.as_str().to_string();
        let call = Call {
            method,
            app_id: app_id.clone(),
            title: Some(title).filter(|title| !title.is_empty()),
            options,
        };
        let title = call.title().to_string();

        let request = match Request::new(connection, &handle, &invocation, &app_id, &title) {
            Ok(request) => request,
            Err(error) => {
                crate::log!("portal", "{error}");
                invocation.return_value(Some(&(RESPONSE_OTHER, empty_results()).to_variant()));
                return;
            }
        };

        let Some(host) = self.host.borrow().clone() else {
            crate::log!("portal", "no dialog host is installed");
            request.complete(RESPONSE_OTHER, empty_results());
            return;
        };
        if let Err(error) = host.open(request.clone(), &call) {
            crate::log!("portal", "cannot show the dialog: {error}");
            request.complete(RESPONSE_OTHER, empty_results());
        }
    }
}
