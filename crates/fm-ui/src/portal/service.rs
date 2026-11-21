// SPDX-License-Identifier: GPL-3.0-or-later
//! The `org.freedesktop.impl.portal.FileChooser` backend.
//!
//! xdg-desktop-portal calls this process over D-Bus when an application asks
//! for an Open or Save dialog. The method call carries a Request object path; the
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

use super::filter::Filter;

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
// SaveFile and SaveFiles are F3.
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

/// The options a FileChooser call can carry. One type serves all three
/// methods: every field is named for what it carries, and the parser only reads
/// the keys the method knows. Everything else is accepted and ignored: a
/// chooser that refused an unfamiliar key would break applications that only
/// wanted an ordinary dialog. A known key whose value is the wrong type is read
/// as absent and named in `malformed`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CallOptions {
    /// The accept button's label, with mnemonic underlines allowed.
    pub accept_label: Option<String>,
    /// The suggested starting folder. It is only used if it exists and is a
    /// directory: the frontend's suggestion never becomes a failed dialog.
    pub current_folder: Option<PathBuf>,
    /// Several files may be picked, as long as every one is a regular file.
    pub multiple: bool,
    /// Folders-only mode: the grid shows folders, Open picks a folder.
    pub directory: bool,
    /// The application's file-type filters, in the order it sent them.
    pub filters: Vec<Filter>,
    /// Where in `filters` the dialog starts: the entry equal to the call's
    /// `current_filter`, compared as a whole `(sa(us))` value, or the first.
    pub current_filter: Option<usize>,
    /// `current_filter` on a call with no `filters`: one fixed filter with no
    /// dropdown, the rule `nautilus-portal.c:553-560` follows.
    pub fixed_filter: Option<Filter>,
    /// The application's extra choices, in the order it sent them.
    pub choices: Vec<Choice>,
    /// SaveFile: the name the entry starts with.
    pub current_name: Option<String>,
    /// SaveFile: an existing file to open the folder of and pre-select.
    pub current_file: Option<PathBuf>,
    /// SaveFiles: the names to save, each a NUL-terminated byte string. Kept as
    /// bytes until [`super::save`] takes the last component and validates it.
    pub files: Vec<Vec<u8>>,
    /// Known option keys this build does not act on. Each is logged once;
    /// unknown keys are ignored silently.
    pub ignored: Vec<&'static str>,
    /// Known option keys whose value could not be read. Also logged once.
    pub malformed: Vec<&'static str>,
}

impl CallOptions {
    /// Whether the footer shows the filter dropdown. OpenFile hides filters in
    /// folders-only mode, SaveFile always shows them when it has any, and
    /// SaveFiles never does.
    pub fn shows_filters_for(&self, method: Method) -> bool {
        match method {
            Method::OpenFile => !self.directory && !self.filters.is_empty(),
            Method::SaveFile => !self.filters.is_empty(),
            Method::SaveFiles => false,
        }
    }

    /// The filter the dialog starts with, or `None` when nothing filters files
    /// out of it.
    pub fn start_filter_for(&self, method: Method) -> Option<&Filter> {
        match method {
            Method::OpenFile if self.directory => None,
            Method::SaveFiles => None,
            Method::OpenFile | Method::SaveFile => {
                if !self.filters.is_empty() {
                    self.filters.get(self.current_filter.unwrap_or(0))
                } else {
                    self.fixed_filter.as_ref()
                }
            }
        }
    }
}

/// One entry in the `choices` option: an id, a label, its options and a
/// default. An empty option list means a checkbox, whose value is `"true"` or
/// `"false"` (`org.freedesktop.impl.portal.FileChooser.xml`, and the same rule
/// `nautilus-portal.c:402-419` applies).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub label: String,
    /// The `(ss)` option pairs: an id and what the app shows for it.
    pub options: Vec<(String, String)>,
    pub default: String,
}

impl Choice {
    /// A choice with no options is a checkbox.
    pub fn is_check(&self) -> bool {
        self.options.is_empty()
    }

    /// Where a checkbox starts. Anything but `"true"` is off, as in Nautilus's
    /// `g_strcmp0 (selected, "true") == 0`.
    pub fn starts_checked(&self) -> bool {
        self.default == "true"
    }

    /// The option a dropdown starts on: the default, or the first option when
    /// the default is not one of them.
    pub fn start_option(&self) -> usize {
        self.options
            .iter()
            .position(|(id, _)| *id == self.default)
            .unwrap_or(0)
    }
}

/// One incoming call, after its arguments have been split into the parts the
/// picker needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub method: Method,
    pub app_id: String,
    pub title: Option<String>,
    pub options: CallOptions,
}

impl Call {
    /// The accept button's label: the caller's, or the mode's own. A folder
    /// picker says "Select", a file picker "Open", and both Save methods say
    /// "Save".
    pub fn accept_label(&self) -> &str {
        self.options
            .accept_label
            .as_deref()
            .filter(|label| !label.is_empty())
            .unwrap_or(match self.method {
                Method::SaveFile | Method::SaveFiles => "Save",
                Method::OpenFile if self.options.directory => "Select",
                Method::OpenFile => "Open",
            })
    }

    /// The window title: the caller's, or the app's own name.
    pub fn title(&self) -> &str {
        self.title
            .as_deref()
            .filter(|title| !title.is_empty())
            .unwrap_or("Gimbal Files")
    }
}

/// The option keys each method knows. A key another method knows is still
/// unknown here: the dialog reads only the keys its own call can carry.
/// `modal` is known to every method but stays unimplemented until F4, so it is
/// the one key that reaches `ignored`.
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
const KNOWN_SAVE_FILE_KEYS: &[&str] = &[
    "accept_label",
    "modal",
    "filters",
    "current_filter",
    "choices",
    "current_name",
    "current_folder",
    "current_file",
];
const KNOWN_SAVE_FILES_KEYS: &[&str] = &[
    "accept_label",
    "modal",
    "choices",
    "current_folder",
    "files",
];

fn known_keys(method: Method) -> &'static [&'static str] {
    match method {
        Method::OpenFile => KNOWN_OPEN_KEYS,
        Method::SaveFile => KNOWN_SAVE_FILE_KEYS,
        Method::SaveFiles => KNOWN_SAVE_FILES_KEYS,
    }
}

/// The wire type of the `filters` option.
const FILTERS_TYPE: &str = "a(sa(us))";
/// The wire type of one filter, and of the `current_filter` option.
const FILTER_TYPE: &str = "(sa(us))";
/// The wire type of the `choices` option.
const CHOICES_TYPE: &str = "a(ssa(ss)s)";

/// Reads one option as `$type`. A value of another type is read as absent and
/// named for the log, so a caller that sent `multiple: "yes"` still gets a
/// dialog rather than a failed one.
macro_rules! typed_option {
    ($options:expr, $malformed:expr, $key:literal, $type:ty) => {
        $options
            .get($key)
            .and_then(|value| match value.get::<$type>() {
                Some(parsed) => Some(parsed),
                None => {
                    $malformed.push($key);
                    None
                }
            })
    };
}

/// Reads one option that must be a whole variant of one exact type, for the two
/// keys whose received bytes have to survive: a filter is answered back as it
/// arrived, and comparing types by hand would lose the rules it holds.
fn variant_option<'a>(
    options: &'a HashMap<String, glib::Variant>,
    malformed: &mut Vec<&'static str>,
    key: &'static str,
    expected: &str,
) -> Option<&'a glib::Variant> {
    let value = options.get(key)?;
    if value.type_().as_str() == expected {
        Some(value)
    } else {
        malformed.push(key);
        None
    }
}

/// The children of an array-typed option, each as its own variant. The exact
/// child is what a filter is answered back with, so the array is walked rather
/// than decoded into Rust types that could not be shown to the application
/// unchanged.
fn children(value: &glib::Variant) -> impl Iterator<Item = glib::Variant> {
    (0..value.n_children()).filter_map(|index| value.try_child_value(index))
}

/// Parse one call's options vardict. Every key the method knows either changes
/// the dialog or is named in `ignored` / `malformed` for the log; a key for
/// another method is unknown and ignored silently. Nothing here is an error.
pub fn parse_call_options(method: Method, options: &HashMap<String, glib::Variant>) -> CallOptions {
    let mut malformed: Vec<&'static str> = Vec::new();
    let accept_label =
        typed_option!(options, malformed, "accept_label", String).filter(|label| !label.is_empty());
    let current_folder = typed_option!(options, malformed, "current_folder", Vec<u8>)
        .and_then(|bytes| folder_from_bytes(&bytes));

    // SaveFiles has no filters at all, so those two keys are unknown to it and
    // a wrong-typed one must stay silent.
    let (filters, requested) = if method == Method::SaveFiles {
        (Vec::new(), None)
    } else {
        let filters = match variant_option(options, &mut malformed, "filters", FILTERS_TYPE) {
            Some(value) => children(value)
                .filter_map(|entry| Filter::from_variant(&entry))
                .collect(),
            None => Vec::new(),
        };
        let requested = variant_option(options, &mut malformed, "current_filter", FILTER_TYPE)
            .and_then(Filter::from_variant);
        (filters, requested)
    };
    // The whole `(sa(us))` decides which entry is selected, as Nautilus does
    // (`nautilus-portal.c:539`). Two filters with the same name but different
    // rules are different filters.
    let current_filter = requested
        .as_ref()
        .and_then(|want| filters.iter().position(|have| have == want));
    // A `current_filter` with no `filters` is one fixed filter, with no
    // dropdown to change it (`nautilus-portal.c:553-560`).
    let fixed_filter = if filters.is_empty() { requested } else { None };

    let choices = match variant_option(options, &mut malformed, "choices", CHOICES_TYPE) {
        Some(value) => children(value)
            .filter_map(|entry| entry.get::<(String, String, Vec<(String, String)>, String)>())
            .map(|(id, label, options, default)| Choice {
                id,
                label,
                options,
                default,
            })
            .collect(),
        None => Vec::new(),
    };

    let (multiple, directory, current_name, current_file, files) = match method {
        Method::OpenFile => (
            typed_option!(options, malformed, "multiple", bool).unwrap_or_default(),
            typed_option!(options, malformed, "directory", bool).unwrap_or_default(),
            None,
            None,
            Vec::new(),
        ),
        Method::SaveFile => (
            false,
            false,
            typed_option!(options, malformed, "current_name", String),
            typed_option!(options, malformed, "current_file", Vec<u8>)
                .and_then(|bytes| folder_from_bytes(&bytes)),
            Vec::new(),
        ),
        Method::SaveFiles => (
            false,
            false,
            None,
            None,
            typed_option!(options, malformed, "files", Vec<Vec<u8>>).unwrap_or_default(),
        ),
    };

    let ignored = known_keys(method)
        .iter()
        .copied()
        .filter(|key| *key == "modal" && options.contains_key(*key))
        .collect();
    CallOptions {
        accept_label,
        current_folder,
        multiple,
        directory,
        filters,
        current_filter,
        fixed_filter,
        choices,
        current_name,
        current_file,
        files,
        ignored,
        malformed,
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

/// The starting folder for a whole call. SaveFile opens the folder of
/// `current_file` when it has one and it still exists, so the file can be
/// selected; anything else follows [`starting_folder`].
pub fn starting_folder_for(call: &Call, home: &Path) -> PathBuf {
    if call.method == Method::SaveFile
        && let Some(parent) = call
            .options
            .current_file
            .as_deref()
            .and_then(Path::parent)
            .filter(|parent| parent.is_dir())
    {
        return parent.to_path_buf();
    }
    starting_folder(call.options.current_folder.as_deref(), home)
}

/// The empty `a{sv}` a cancelled or refused call returns.
pub fn empty_results() -> HashMap<String, glib::Variant> {
    HashMap::new()
}

/// What a successful OpenFile answers with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pick {
    /// The chosen paths, in the order they are reported in.
    pub paths: Vec<PathBuf>,
    /// The chosen filter's exact `(sa(us))` value as it was received, whenever
    /// the call had filters. `None` leaves the key out, as `nautilus-portal.c`
    /// does when it has no current filter to report (`:270-273`).
    pub current_filter: Option<glib::Variant>,
    /// Every choice the application sent, in the order it sent them, each with
    /// the value the dialog holds: `"true"`, `"false"`, or the option id.
    pub choices: Vec<(String, String)>,
}

impl Pick {
    /// The one line an accept leaves in the log: the mode, how many URIs, the
    /// filter's name and the choices as `id=value`. Paths are allowed here;
    /// the contents of a file never are.
    pub fn log_line(&self, mode: &str, filter_name: Option<&str>) -> String {
        let choices = if self.choices.is_empty() {
            "none".to_string()
        } else {
            self.choices
                .iter()
                .map(|(id, value)| format!("{id}={value}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        format!(
            "accept: {mode}, {} uri(s), filter {}, choices {choices}",
            self.paths.len(),
            match filter_name {
                Some(name) => format!("{name:?}"),
                None => "none".to_string(),
            }
        )
    }
}

/// The `a{sv}` a successful call returns: one or more `file://` URIs, the
/// choices, the chosen filter when the method reports one, and — for OpenFile
/// only — `writable`. Paths are encoded by GIO, so spaces and non-ASCII names
/// come out percent-encoded exactly as the portal expects.
///
/// `writable` is always `true`: Kyle opens files in order to edit them, and a
/// `false` here would make the document portal hand a sandboxed application a
/// read-only copy of whatever it picked. Nautilus answers the same way unless
/// its optional "Open Read-Only" box is ticked (`nautilus-portal.c:186-206`,
/// `242-276`); Gimbal has no such box. The Save methods carry no `writable`
/// key at all (`nautilus-portal.c:236-262`).
pub fn results_for(pick: &Pick, method: Method) -> HashMap<String, glib::Variant> {
    let mut results = HashMap::new();
    let uris = pick
        .paths
        .iter()
        .map(|path| gio::File::for_path(path).uri().to_string())
        .collect::<Vec<_>>();
    results.insert("uris".to_string(), uris.to_variant());
    results.insert("choices".to_string(), pick.choices.clone().to_variant());
    // SaveFiles carries no filters, so it has nothing to report back.
    if method != Method::SaveFiles
        && let Some(filter) = &pick.current_filter
    {
        results.insert("current_filter".to_string(), filter.clone());
    }
    if method == Method::OpenFile {
        results.insert("writable".to_string(), true.to_variant());
    }
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
    method: Method,
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
            .field("method", &self.method)
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
        method: Method,
        app_id: &str,
        title: &str,
    ) -> Result<Rc<Self>, String> {
        let request = Rc::new(Self {
            handle: handle.to_string(),
            app_id: app_id.to_string(),
            title: title.to_string(),
            method,
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

    /// The application that made this call, for tests that check the routing.
    #[cfg(test)]
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    /// Answer with success and the dialog's pick.
    pub fn accept(self: &Rc<Self>, pick: Pick) -> bool {
        for path in &pick.paths {
            crate::log!("portal", "selected {}", path.display());
        }
        self.complete(RESPONSE_SUCCESS, results_for(&pick, self.method))
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

        let options = parse_call_options(method, &options);
        if !options.ignored.is_empty() {
            crate::log!(
                "portal",
                "{} ignores option(s) {}",
                method.as_str(),
                options.ignored.join(", ")
            );
        }
        if !options.malformed.is_empty() {
            // A value of the wrong type is read as absent, never refused.
            crate::log!(
                "portal",
                "{} ignores malformed option(s) {}",
                method.as_str(),
                options.malformed.join(", ")
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

        let request = match Request::new(connection, &handle, &invocation, method, &app_id, &title)
        {
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
