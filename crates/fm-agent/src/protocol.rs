// SPDX-License-Identifier: GPL-3.0-or-later
//! The V1 wire protocol: one UTF-8 TOML document per length-prefixed frame.
//!
//! A frame is a four-byte big-endian byte count followed by that many bytes of
//! TOML. The parser is deliberately strict: a version other than 1, an unknown
//! operation or field, a wrong type, an oversized value, invalid UTF-8, a NUL
//! in a string or a duplicate key is refused before the bridge sees anything.
//! `toml_edit` does the syntax work; there is no hand-written TOML parser.

use std::fmt;

use toml_edit::{Document, DocumentMut, Item, Table, Value};

pub const VERSION: i64 = 1;
/// The largest frame, including the four-byte length.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_REQUEST_ID: usize = 128;
pub const MAX_COMMANDS: usize = 128;
pub const MAX_STRING_BYTES: usize = 8192;
pub const MAX_PAGE: usize = 100;
pub const DEFAULT_PAGE: usize = 20;

/// One request from a local client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub request_id: String,
    pub instance: Option<String>,
    pub generation: Option<u64>,
    pub operation: Operation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Commands {
        offset: usize,
        limit: usize,
    },
    State {
        offset: usize,
        limit: usize,
    },
    Run {
        window: Option<String>,
        tab: Option<String>,
        command: String,
        arguments: Vec<(String, String)>,
        wait: bool,
    },
    Job {
        id: String,
    },
}

impl Operation {
    pub fn name(&self) -> &'static str {
        match self {
            Operation::Commands { .. } => "commands",
            Operation::State { .. } => "state",
            Operation::Run { .. } => "run",
            Operation::Job { .. } => "job",
        }
    }
}

/// A reply to one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub request_id: String,
    pub operation: String,
    pub status: Status,
    pub instance: Option<String>,
    pub generation: Option<u64>,
    pub data: Data,
    pub error: Option<ErrorBody>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Accepted,
    Error,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Accepted => "accepted",
            Status::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Data {
    Empty,
    Commands {
        offset: usize,
        total: usize,
        next: Option<usize>,
        commands: Vec<CommandInfo>,
    },
    State {
        offset: usize,
        total: usize,
        next: Option<usize>,
        socket: SocketInfo,
        windows: Vec<WindowInfo>,
        jobs: Vec<JobInfo>,
    },
    Run {
        kind: RunKind,
        job: Option<String>,
        lines: Vec<String>,
    },
    Job(JobInfo),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    Done,
    DryRun,
    Job,
}

impl RunKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RunKind::Done => "done",
            RunKind::DryRun => "dry-run",
            RunKind::Job => "job",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInfo {
    pub name: String,
    pub summary: String,
    pub effect: String,
    pub undo: bool,
    pub allowed: bool,
    pub reason: Option<String>,
    pub arguments: Vec<ArgumentInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentInfo {
    pub name: String,
    pub kind: String,
    pub presence: String,
    pub default: Option<String>,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketInfo {
    pub status: String,
    pub path: Option<String>,
    pub detail: Option<String>,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: String,
    pub focused: bool,
    pub focused_tab: String,
    pub groups_total: usize,
    pub groups_truncated: bool,
    pub groups: Vec<GroupInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupInfo {
    pub id: String,
    pub name: Option<String>,
    pub number: u32,
    pub focused: bool,
    pub split: bool,
    pub tabs_total: usize,
    pub tabs_truncated: bool,
    pub tabs: Vec<TabInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabInfo {
    pub id: String,
    pub folder: String,
    pub folder_hex: String,
    pub focused: bool,
    /// An administrator tab is listed for state, but every run against it is
    /// refused: it is UI-only.
    pub admin: bool,
    pub selection_count: usize,
    pub selection: Vec<NameInfo>,
    pub selection_truncated: bool,
}

/// A display string plus the lossless hexadecimal bytes of the OS name. The
/// display text is absent when the name is not UTF-8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameInfo {
    pub text: Option<String>,
    pub hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobInfo {
    pub id: String,
    pub state: String,
    pub summary: String,
    pub lines: Vec<String>,
    pub errors: Vec<String>,
}

/// A protocol refusal. It never carries the raw frame back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError {
    pub code: &'static str,
    pub message: String,
}

impl ProtocolError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ProtocolError {}

// ---------------------------------------------------------------------------
// Request parsing and encoding
// ---------------------------------------------------------------------------

impl Request {
    /// Encode this request as one TOML document.
    pub fn to_toml(&self) -> String {
        let mut root = Table::new();
        root.insert("version", Item::Value(Value::from(VERSION)));
        root.insert("request_id", Item::Value(Value::from(&self.request_id)));
        root.insert("operation", Item::Value(Value::from(self.operation.name())));
        if let Some(instance) = &self.instance {
            root.insert("instance", Item::Value(Value::from(instance)));
        }
        if let Some(generation) = self.generation {
            root.insert("generation", Item::Value(Value::from(generation as i64)));
        }

        match &self.operation {
            Operation::Commands { offset, limit } | Operation::State { offset, limit } => {
                root.insert("offset", Item::Value(Value::from(*offset as i64)));
                root.insert("limit", Item::Value(Value::from(*limit as i64)));
            }
            Operation::Run {
                window,
                tab,
                command,
                arguments,
                wait,
            } => {
                if let Some(window) = window {
                    root.insert("window", Item::Value(Value::from(window)));
                }
                if let Some(tab) = tab {
                    root.insert("tab", Item::Value(Value::from(tab)));
                }
                root.insert("command", Item::Value(Value::from(command)));
                root.insert("wait", Item::Value(Value::from(*wait)));
                let mut list = toml_edit::Array::new();
                for (key, value) in arguments {
                    let mut table = toml_edit::InlineTable::new();
                    table.insert("key", Value::from(key));
                    table.insert("value", Value::from(value));
                    list.push(Value::InlineTable(table));
                }
                root.insert("args", Item::Value(Value::Array(list)));
            }
            Operation::Job { id } => {
                root.insert("id", Item::Value(Value::from(id)));
            }
        }

        let mut document = DocumentMut::new();
        for (key, item) in root.iter() {
            document[key] = item.clone();
        }
        document.to_string()
    }

    /// Parse and strictly validate one request document.
    pub fn parse(text: &str) -> Result<Self, ProtocolError> {
        let document = Document::parse(text)
            .map_err(|error| ProtocolError::new("malformed", error.message()))?;
        let root = document.as_table();

        let version = integer(root, "version", true)?;
        if version != VERSION {
            return Err(ProtocolError::new(
                "unsupported-version",
                format!("expected version {VERSION}, found {version}"),
            ));
        }

        let request_id = string(root, "request_id", true)?;
        check_request_id(&request_id)?;

        let operation_name = string(root, "operation", true)?;
        let allowed: &[&str] = match operation_name.as_str() {
            "commands" | "state" => &[
                "version",
                "request_id",
                "operation",
                "instance",
                "generation",
                "offset",
                "limit",
            ],
            "run" => &[
                "version",
                "request_id",
                "operation",
                "instance",
                "generation",
                "window",
                "tab",
                "command",
                "args",
                "wait",
            ],
            "job" => &[
                "version",
                "request_id",
                "operation",
                "instance",
                "generation",
                "id",
            ],
            other => {
                return Err(ProtocolError::new(
                    "unknown-operation",
                    format!("unknown operation {other:?}"),
                ));
            }
        };
        reject_unknown(root, allowed)?;

        let instance = optional_string(root, "instance")?;
        let generation = match root.get("generation") {
            Some(item) => {
                let value = item_value(item, "generation")?
                    .as_integer()
                    .ok_or_else(|| {
                        ProtocolError::new("bad-type", "generation must be an integer")
                    })?;
                if value < 0 {
                    return Err(ProtocolError::new(
                        "bad-value",
                        "generation must not be negative",
                    ));
                }
                Some(value as u64)
            }
            None => None,
        };

        // `commands` and `state` are bootstrap operations and may discover the
        // current identity. A write or a job lookup must name the exact enable
        // cycle it belongs to, so a stale client can never write unbound.
        if matches!(operation_name.as_str(), "run" | "job")
            && (instance.is_none() || generation.is_none())
        {
            return Err(ProtocolError::new(
                "missing-field",
                format!("{operation_name} requires both instance and generation"),
            ));
        }

        let operation = match operation_name.as_str() {
            "commands" | "state" => {
                let offset = optional_page(root, "offset")?.unwrap_or(0);
                let limit = optional_page(root, "limit")?
                    .unwrap_or(DEFAULT_PAGE)
                    .min(MAX_PAGE);
                if limit == 0 {
                    return Err(ProtocolError::new("bad-value", "limit must be at least 1"));
                }
                if operation_name == "commands" {
                    Operation::Commands { offset, limit }
                } else {
                    Operation::State { offset, limit }
                }
            }
            "run" => {
                let window = optional_string(root, "window")?;
                let tab = optional_string(root, "tab")?;
                let command = string(root, "command", true)?;
                if command.is_empty() || command.contains('\0') {
                    return Err(ProtocolError::new(
                        "bad-value",
                        "command must be a non-empty command name",
                    ));
                }
                let arguments = parse_arguments(root)?;
                let wait = optional_bool(root, "wait")?.unwrap_or(false);
                Operation::Run {
                    window,
                    tab,
                    command,
                    arguments,
                    wait,
                }
            }
            "job" => {
                let id = string(root, "id", true)?;
                if id.is_empty() || id.contains('\0') {
                    return Err(ProtocolError::new("bad-value", "job id must not be empty"));
                }
                Operation::Job { id }
            }
            _ => unreachable!("the operation was checked above"),
        };

        Ok(Self {
            request_id,
            instance,
            generation,
            operation,
        })
    }
}

fn parse_arguments(root: &Table) -> Result<Vec<(String, String)>, ProtocolError> {
    let Some(item) = root.get("args") else {
        return Ok(Vec::new());
    };
    let array = item_value(item, "args")?
        .as_array()
        .ok_or_else(|| ProtocolError::new("bad-type", "args must be an array"))?;
    if array.len() > MAX_COMMANDS {
        return Err(ProtocolError::new(
            "too-many-arguments",
            format!("at most {MAX_COMMANDS} arguments are allowed"),
        ));
    }

    let mut arguments = Vec::with_capacity(array.len());
    for (index, value) in array.iter().enumerate() {
        let table = value.as_inline_table().ok_or_else(|| {
            ProtocolError::new(
                "bad-type",
                format!("args[{index}] must be an inline table like {{ key = \"paths\", value = \"/tmp\" }}"),
            )
        })?;
        reject_inline_unknown(table, &["key", "value"])?;
        let key = table.get("key").and_then(Value::as_str).ok_or_else(|| {
            ProtocolError::new("bad-type", format!("args[{index}].key must be a string"))
        })?;
        let value = table.get("value").and_then(Value::as_str).ok_or_else(|| {
            ProtocolError::new("bad-type", format!("args[{index}].value must be a string"))
        })?;
        check_string(&format!("args[{index}].key"), key)?;
        check_string(&format!("args[{index}].value"), value)?;
        if key.is_empty() || key.len() > 64 {
            return Err(ProtocolError::new(
                "bad-value",
                format!("args[{index}].key must be 1 to 64 bytes"),
            ));
        }
        arguments.push((key.to_string(), value.to_string()));
    }
    Ok(arguments)
}

fn check_request_id(id: &str) -> Result<(), ProtocolError> {
    if id.is_empty() || id.len() > MAX_REQUEST_ID {
        return Err(ProtocolError::new(
            "bad-request-id",
            format!("request_id must be 1 to {MAX_REQUEST_ID} bytes"),
        ));
    }
    if !id
        .bytes()
        .all(|byte| byte.is_ascii_graphic() || byte == b' ' || byte == b'\t')
    {
        return Err(ProtocolError::new(
            "bad-request-id",
            "request_id must be printable ASCII",
        ));
    }
    Ok(())
}

fn check_string(label: &str, value: &str) -> Result<(), ProtocolError> {
    if value.len() > MAX_STRING_BYTES {
        return Err(ProtocolError::new(
            "too-long",
            format!("{label} exceeds {MAX_STRING_BYTES} bytes"),
        ));
    }
    if value.contains('\0') {
        return Err(ProtocolError::new(
            "nul-byte",
            format!("{label} contains a NUL byte"),
        ));
    }
    Ok(())
}

fn item_value<'a>(item: &'a Item, key: &str) -> Result<&'a Value, ProtocolError> {
    item.as_value()
        .ok_or_else(|| ProtocolError::new("bad-type", format!("{key} must be a value")))
}

fn integer(root: &Table, key: &str, required: bool) -> Result<i64, ProtocolError> {
    match root.get(key) {
        Some(item) => item_value(item, key)?
            .as_integer()
            .ok_or_else(|| ProtocolError::new("bad-type", format!("{key} must be an integer"))),
        None if required => Err(ProtocolError::new(
            "missing-field",
            format!("missing {key}"),
        )),
        None => Ok(0),
    }
}

fn string(root: &Table, key: &str, required: bool) -> Result<String, ProtocolError> {
    match root.get(key) {
        Some(item) => {
            let value = item_value(item, key)?
                .as_str()
                .ok_or_else(|| ProtocolError::new("bad-type", format!("{key} must be a string")))?;
            check_string(key, value)?;
            Ok(value.to_string())
        }
        None if required => Err(ProtocolError::new(
            "missing-field",
            format!("missing {key}"),
        )),
        None => Ok(String::new()),
    }
}

fn optional_string(root: &Table, key: &str) -> Result<Option<String>, ProtocolError> {
    if root.get(key).is_none() {
        return Ok(None);
    }
    string(root, key, false).map(Some)
}

fn optional_bool(root: &Table, key: &str) -> Result<Option<bool>, ProtocolError> {
    match root.get(key) {
        Some(item) => item_value(item, key)?
            .as_bool()
            .map(Some)
            .ok_or_else(|| ProtocolError::new("bad-type", format!("{key} must be a boolean"))),
        None => Ok(None),
    }
}

fn optional_page(root: &Table, key: &str) -> Result<Option<usize>, ProtocolError> {
    match root.get(key) {
        Some(item) => {
            let value = item_value(item, key)?.as_integer().ok_or_else(|| {
                ProtocolError::new("bad-type", format!("{key} must be an integer"))
            })?;
            if !(0..=1_000_000).contains(&value) {
                return Err(ProtocolError::new(
                    "bad-value",
                    format!("{key} must be between 0 and 1000000"),
                ));
            }
            Ok(Some(value as usize))
        }
        None => Ok(None),
    }
}

fn reject_unknown(root: &Table, allowed: &[&str]) -> Result<(), ProtocolError> {
    for (key, _) in root.iter() {
        if !allowed.contains(&key) {
            return Err(ProtocolError::new(
                "unknown-field",
                format!("unknown field {key:?}"),
            ));
        }
    }
    Ok(())
}

fn reject_inline_unknown(
    table: &toml_edit::InlineTable,
    allowed: &[&str],
) -> Result<(), ProtocolError> {
    for (key, _) in table.iter() {
        if !allowed.contains(&key) {
            return Err(ProtocolError::new(
                "unknown-field",
                format!("unknown args field {key:?}"),
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Reply encoding
// ---------------------------------------------------------------------------

impl Reply {
    pub fn error(
        request_id: impl Into<String>,
        operation: impl Into<String>,
        code: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            operation: operation.into(),
            status: Status::Error,
            instance: None,
            generation: None,
            data: Data::Empty,
            error: Some(ErrorBody {
                code,
                message: message.into(),
            }),
        }
    }

    /// Encode this reply as one TOML document.
    pub fn to_toml(&self) -> String {
        let mut root = Table::new();
        root.insert("version", Item::Value(Value::from(VERSION)));
        root.insert("request_id", Item::Value(Value::from(&self.request_id)));
        root.insert("operation", Item::Value(Value::from(&self.operation)));
        root.insert("status", Item::Value(Value::from(self.status.as_str())));
        if let Some(instance) = &self.instance {
            root.insert("instance", Item::Value(Value::from(instance)));
        }
        if let Some(generation) = self.generation {
            root.insert("generation", Item::Value(Value::from(generation as i64)));
        }
        if let Some(error) = &self.error {
            let mut table = Table::new();
            table.insert("code", Item::Value(Value::from(error.code)));
            table.insert("message", Item::Value(Value::from(&error.message)));
            root.insert("error", Item::Table(table));
        }
        if !matches!(self.data, Data::Empty) {
            root.insert("data", Item::Table(data_table(&self.data)));
        }

        let mut document = DocumentMut::new();
        for (key, item) in root.iter() {
            document[key] = item.clone();
        }
        document.to_string()
    }
}

fn data_table(data: &Data) -> Table {
    let mut table = Table::new();
    match data {
        Data::Empty => {}
        Data::Commands {
            offset,
            total,
            next,
            commands,
        } => {
            table.insert("offset", Item::Value(Value::from(*offset as i64)));
            table.insert("total", Item::Value(Value::from(*total as i64)));
            if let Some(next) = next {
                table.insert("next", Item::Value(Value::from(*next as i64)));
            }
            let mut list = toml_edit::ArrayOfTables::new();
            for command in commands {
                list.push(command_table(command));
            }
            table.insert("command", Item::ArrayOfTables(list));
        }
        Data::State {
            offset,
            total,
            next,
            socket,
            windows,
            jobs,
        } => {
            table.insert("offset", Item::Value(Value::from(*offset as i64)));
            table.insert("total", Item::Value(Value::from(*total as i64)));
            if let Some(next) = next {
                table.insert("next", Item::Value(Value::from(*next as i64)));
            }
            let mut socket_table = Table::new();
            socket_table.insert("status", Item::Value(Value::from(&socket.status)));
            if let Some(path) = &socket.path {
                socket_table.insert("path", Item::Value(Value::from(path)));
            }
            if let Some(detail) = &socket.detail {
                socket_table.insert("detail", Item::Value(Value::from(detail)));
            }
            socket_table.insert(
                "generation",
                Item::Value(Value::from(socket.generation as i64)),
            );
            table.insert("socket", Item::Table(socket_table));

            let mut windows_list = toml_edit::ArrayOfTables::new();
            for window in windows {
                windows_list.push(window_table(window));
            }
            table.insert("window", Item::ArrayOfTables(windows_list));

            let mut jobs_list = toml_edit::ArrayOfTables::new();
            for job in jobs {
                jobs_list.push(job_table(job));
            }
            table.insert("job", Item::ArrayOfTables(jobs_list));
        }
        Data::Run { kind, job, lines } => {
            table.insert("kind", Item::Value(Value::from(kind.as_str())));
            if let Some(job) = job {
                table.insert("job", Item::Value(Value::from(job)));
            }
            table.insert("lines", string_array(lines));
        }
        Data::Job(job) => {
            for (key, item) in job_table(job).iter() {
                table.insert(key, item.clone());
            }
        }
    }
    table
}

fn command_table(command: &CommandInfo) -> Table {
    let mut table = Table::new();
    table.insert("name", Item::Value(Value::from(&command.name)));
    table.insert("summary", Item::Value(Value::from(&command.summary)));
    table.insert("effect", Item::Value(Value::from(&command.effect)));
    table.insert("undo", Item::Value(Value::from(command.undo)));
    table.insert("allowed", Item::Value(Value::from(command.allowed)));
    if let Some(reason) = &command.reason {
        table.insert("reason", Item::Value(Value::from(reason)));
    }
    let mut arguments = toml_edit::ArrayOfTables::new();
    for argument in &command.arguments {
        let mut argument_table = Table::new();
        argument_table.insert("name", Item::Value(Value::from(&argument.name)));
        argument_table.insert("kind", Item::Value(Value::from(&argument.kind)));
        argument_table.insert("presence", Item::Value(Value::from(&argument.presence)));
        argument_table.insert("required", Item::Value(Value::from(argument.required)));
        if let Some(default) = &argument.default {
            argument_table.insert("default", Item::Value(Value::from(default)));
        }
        arguments.push(argument_table);
    }
    table.insert("argument", Item::ArrayOfTables(arguments));
    table
}

fn window_table(window: &WindowInfo) -> Table {
    let mut table = Table::new();
    table.insert("id", Item::Value(Value::from(&window.id)));
    table.insert("focused", Item::Value(Value::from(window.focused)));
    table.insert("focused_tab", Item::Value(Value::from(&window.focused_tab)));
    table.insert(
        "groups_total",
        Item::Value(Value::from(window.groups_total as i64)),
    );
    table.insert(
        "groups_truncated",
        Item::Value(Value::from(window.groups_truncated)),
    );
    let mut groups = toml_edit::ArrayOfTables::new();
    for group in &window.groups {
        let mut group_table = Table::new();
        group_table.insert("id", Item::Value(Value::from(&group.id)));
        if let Some(name) = &group.name {
            group_table.insert("name", Item::Value(Value::from(name)));
        }
        group_table.insert("number", Item::Value(Value::from(i64::from(group.number))));
        group_table.insert("focused", Item::Value(Value::from(group.focused)));
        group_table.insert("split", Item::Value(Value::from(group.split)));
        group_table.insert(
            "tabs_total",
            Item::Value(Value::from(group.tabs_total as i64)),
        );
        group_table.insert(
            "tabs_truncated",
            Item::Value(Value::from(group.tabs_truncated)),
        );
        let mut tabs = toml_edit::ArrayOfTables::new();
        for tab in &group.tabs {
            let mut tab_table = Table::new();
            tab_table.insert("id", Item::Value(Value::from(&tab.id)));
            tab_table.insert("folder", Item::Value(Value::from(&tab.folder)));
            tab_table.insert("folder_hex", Item::Value(Value::from(&tab.folder_hex)));
            tab_table.insert("focused", Item::Value(Value::from(tab.focused)));
            tab_table.insert("admin", Item::Value(Value::from(tab.admin)));
            tab_table.insert(
                "selection_count",
                Item::Value(Value::from(tab.selection_count as i64)),
            );
            tab_table.insert(
                "selection_truncated",
                Item::Value(Value::from(tab.selection_truncated)),
            );
            let mut names = toml_edit::ArrayOfTables::new();
            for name in &tab.selection {
                let mut name_table = Table::new();
                if let Some(text) = &name.text {
                    name_table.insert("text", Item::Value(Value::from(text)));
                }
                name_table.insert("hex", Item::Value(Value::from(&name.hex)));
                names.push(name_table);
            }
            tab_table.insert("selection", Item::ArrayOfTables(names));
            tabs.push(tab_table);
        }
        group_table.insert("tab", Item::ArrayOfTables(tabs));
        groups.push(group_table);
    }
    table.insert("group", Item::ArrayOfTables(groups));
    table
}

fn job_table(job: &JobInfo) -> Table {
    let mut table = Table::new();
    table.insert("id", Item::Value(Value::from(&job.id)));
    table.insert("state", Item::Value(Value::from(&job.state)));
    table.insert("summary", Item::Value(Value::from(&job.summary)));
    table.insert("lines", string_array(&job.lines));
    table.insert("errors", string_array(&job.errors));
    table
}

fn string_array(values: &[String]) -> Item {
    let mut array = toml_edit::Array::new();
    for value in values {
        array.push(Value::from(value));
    }
    Item::Value(Value::Array(array))
}

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    /// The peer closed before a length was read.
    Closed,
    /// The peer closed or timed out part way through a frame.
    Partial,
    TooLarge(usize),
    Empty,
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::Closed => f.write_str("the client closed the connection"),
            FrameError::Partial => f.write_str("the frame ended before its declared length"),
            FrameError::TooLarge(length) => {
                write!(
                    f,
                    "a {length}-byte frame exceeds the {MAX_FRAME_BYTES}-byte limit"
                )
            }
            FrameError::Empty => f.write_str("an empty frame is not a TOML document"),
        }
    }
}

impl std::error::Error for FrameError {}

pub fn frame_bytes(payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    if payload.is_empty() {
        return Err(FrameError::Empty);
    }
    if payload.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(payload.len()));
    }
    let mut framed = Vec::with_capacity(4 + payload.len());
    framed.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    framed.extend_from_slice(payload);
    Ok(framed)
}

/// Write one frame. The caller owns the write timeout.
pub fn write_frame(writer: &mut impl std::io::Write, payload: &[u8]) -> std::io::Result<()> {
    let framed = frame_bytes(payload).map_err(|error| std::io::Error::other(error.to_string()))?;
    writer.write_all(&framed)?;
    writer.flush()
}

/// Read one frame. `Ok(None)` means a clean end of stream between frames.
pub fn read_frame(reader: &mut impl std::io::Read) -> Result<Option<Vec<u8>>, ReadError> {
    let mut length = [0_u8; 4];
    let mut read = 0;
    while read < length.len() {
        match reader.read(&mut length[read..]) {
            Ok(0) if read == 0 => return Ok(None),
            Ok(0) => return Err(ReadError::Frame(FrameError::Partial)),
            Ok(count) => read += count,
            Err(error) if is_would_block(&error) => {
                return Err(ReadError::Io(error));
            }
            Err(error) => return Err(ReadError::Io(error)),
        }
    }
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 {
        return Err(ReadError::Frame(FrameError::Empty));
    }
    if length > MAX_FRAME_BYTES {
        return Err(ReadError::Frame(FrameError::TooLarge(length)));
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            ReadError::Frame(FrameError::Partial)
        } else {
            ReadError::Io(error)
        }
    })?;
    Ok(Some(payload))
}

#[derive(Debug)]
pub enum ReadError {
    Frame(FrameError),
    Io(std::io::Error),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::Frame(error) => error.fmt(f),
            ReadError::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ReadError {}

fn is_would_block(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// Parse one frame's payload as a request, refusing invalid UTF-8 first.
pub fn parse_frame(payload: &[u8]) -> Result<Request, ProtocolError> {
    let text = std::str::from_utf8(payload).map_err(|_| {
        ProtocolError::new("invalid-utf8", "the frame is not a UTF-8 TOML document")
    })?;
    Request::parse(text)
}

/// Encode one reply as a framed payload.
pub fn encode_reply(reply: &Reply) -> Result<Vec<u8>, ProtocolError> {
    let text = reply.to_toml();
    if text.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::new(
            "too-large",
            "the reply exceeds one frame",
        ));
    }
    Ok(text.into_bytes())
}

/// Little helper for tests and the bridge: a command info value.
pub fn name_info(bytes: &[u8]) -> NameInfo {
    NameInfo {
        text: std::str::from_utf8(bytes).ok().map(str::to_string),
        hex: bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_id() -> String {
        "req-1".to_string()
    }

    #[test]
    fn requests_round_trip_every_operation() {
        let cases = vec![
            Operation::Commands {
                offset: 0,
                limit: 5,
            },
            Operation::State {
                offset: 10,
                limit: 2,
            },
            Operation::Run {
                window: Some("i1/w3".to_string()),
                tab: Some("i1/t9".to_string()),
                command: "file.copy".to_string(),
                arguments: vec![
                    ("paths".to_string(), "/tmp/a".to_string()),
                    ("paths".to_string(), "/tmp/b c".to_string()),
                    ("to".to_string(), "/tmp/dest".to_string()),
                ],
                wait: true,
            },
            Operation::Job {
                id: "i1/j4".to_string(),
            },
        ];
        for operation in cases {
            let request = Request {
                request_id: request_id(),
                instance: Some("i1".to_string()),
                generation: Some(3),
                operation,
            };
            let text = request.to_toml();
            assert_eq!(Request::parse(&text).expect("parses"), request);
        }
    }

    #[test]
    fn arguments_keep_order_and_repeated_keys() {
        let text = Request {
            request_id: request_id(),
            instance: Some("i1".to_string()),
            generation: Some(1),
            operation: Operation::Run {
                window: None,
                tab: None,
                command: "file.trash".to_string(),
                arguments: vec![
                    ("paths".to_string(), "a".to_string()),
                    ("paths".to_string(), "b".to_string()),
                ],
                wait: false,
            },
        }
        .to_toml();
        let parsed = Request::parse(&text).expect("parses");
        let Operation::Run { arguments, .. } = parsed.operation else {
            panic!("run");
        };
        assert_eq!(arguments.len(), 2);
        assert_eq!(arguments[1].1, "b");
    }

    #[test]
    fn wrong_versions_unknown_fields_operations_and_types_are_refused() {
        let base = "version = 1\nrequest_id = \"r\"\noperation = \"state\"\n";
        assert_eq!(
            Request::parse(&base.replace("version = 1", "version = 2"))
                .unwrap_err()
                .code,
            "unsupported-version"
        );
        assert_eq!(
            Request::parse(&format!("{base}extra = true\n"))
                .unwrap_err()
                .code,
            "unknown-field"
        );
        assert_eq!(
            Request::parse("version = 1\nrequest_id = \"r\"\noperation = \"eval\"\n")
                .unwrap_err()
                .code,
            "unknown-operation"
        );
        assert_eq!(
            Request::parse("version = \"1\"\nrequest_id = \"r\"\noperation = \"state\"\n")
                .unwrap_err()
                .code,
            "bad-type"
        );
        assert_eq!(
            Request::parse("version = 1\nrequest_id = \"r\"\noperation = \"state\"\noffset = -1\n")
                .unwrap_err()
                .code,
            "bad-value"
        );
    }

    #[test]
    fn run_and_job_require_an_explicit_instance_and_generation() {
        for text in [
            "version = 1\nrequest_id = \"r\"\noperation = \"run\"\ncommand = \"window.new\"\n",
            "version = 1\nrequest_id = \"r\"\noperation = \"job\"\nid = \"i1/j1\"\n",
            "version = 1\nrequest_id = \"r\"\noperation = \"run\"\ninstance = \"i1\"\ncommand = \"window.new\"\n",
            "version = 1\nrequest_id = \"r\"\noperation = \"job\"\ngeneration = 1\nid = \"i1/j1\"\n",
        ] {
            assert_eq!(
                Request::parse(text).unwrap_err().code,
                "missing-field",
                "{text}"
            );
        }
        // Bootstrap operations may omit them.
        for text in [
            "version = 1\nrequest_id = \"r\"\noperation = \"commands\"\n",
            "version = 1\nrequest_id = \"r\"\noperation = \"state\"\n",
        ] {
            assert!(Request::parse(text).is_ok(), "{text}");
        }
    }

    #[test]
    fn duplicate_keys_nul_paths_and_bad_request_ids_are_refused() {
        assert!(
            Request::parse(
                "version = 1\nrequest_id = \"r\"\nrequest_id = \"s\"\noperation = \"state\"\n"
            )
            .is_err()
        );
        let text = "version = 1\nrequest_id = \"r\"\ninstance = \"i1\"\ngeneration = 1\noperation = \"run\"\ncommand = \"file.trash\"\nargs = [{key = \"paths\", value = \"/tmp/a\\u0000b\"}]\n";
        assert_eq!(Request::parse(text).unwrap_err().code, "nul-byte");
        assert_eq!(
            Request::parse("version = 1\nrequest_id = \"\"\noperation = \"state\"\n")
                .unwrap_err()
                .code,
            "bad-request-id"
        );
    }

    #[test]
    fn invalid_utf8_and_oversized_frames_are_refused() {
        assert_eq!(parse_frame(&[0xff, 0xfe]).unwrap_err().code, "invalid-utf8");
        let oversized = vec![0_u8; MAX_FRAME_BYTES + 1];
        assert!(matches!(
            frame_bytes(&oversized),
            Err(FrameError::TooLarge(_))
        ));
        assert_eq!(frame_bytes(b"").unwrap_err(), FrameError::Empty);
    }

    #[test]
    fn frames_carry_a_big_endian_length_and_round_trip() {
        let framed = frame_bytes(b"version = 1").expect("frames");
        assert_eq!(&framed[..4], &(11_u32).to_be_bytes());
        let mut cursor = std::io::Cursor::new(framed);
        assert_eq!(
            read_frame(&mut cursor).expect("reads").as_deref(),
            Some(&b"version = 1"[..])
        );
        assert_eq!(read_frame(&mut cursor).expect("end"), None);
    }

    #[test]
    fn partial_frames_are_refused() {
        let mut cursor = std::io::Cursor::new(vec![0_u8, 0, 0, 5, b'a']);
        assert!(matches!(
            read_frame(&mut cursor),
            Err(ReadError::Frame(FrameError::Partial))
        ));
        let mut cursor = std::io::Cursor::new(vec![0_u8, 0]);
        assert!(matches!(
            read_frame(&mut cursor),
            Err(ReadError::Frame(FrameError::Partial))
        ));
    }

    #[test]
    fn a_reply_encodes_typed_data_and_an_error() {
        let reply = Reply {
            request_id: request_id(),
            operation: "commands".to_string(),
            status: Status::Ok,
            instance: Some("i1".to_string()),
            generation: Some(2),
            data: Data::Commands {
                offset: 0,
                total: 1,
                next: None,
                commands: vec![CommandInfo {
                    name: "file.copy".to_string(),
                    summary: "Copy files into a folder.".to_string(),
                    effect: "files".to_string(),
                    undo: true,
                    allowed: true,
                    reason: None,
                    arguments: vec![ArgumentInfo {
                        name: "paths".to_string(),
                        kind: "paths".to_string(),
                        presence: "from selection".to_string(),
                        default: None,
                        required: true,
                    }],
                }],
            },
            error: None,
        };
        let text = reply.to_toml();
        assert!(text.contains("[[data.command]]"));
        assert!(text.contains("[[data.command.argument]]"));
        let state = Reply {
            request_id: request_id(),
            operation: "state".to_string(),
            status: Status::Ok,
            instance: Some("i1".to_string()),
            generation: Some(2),
            data: Data::State {
                offset: 0,
                total: 1,
                next: None,
                socket: SocketInfo {
                    status: "on".to_string(),
                    path: None,
                    detail: None,
                    generation: 2,
                },
                windows: vec![WindowInfo {
                    id: "i1/w1".to_string(),
                    focused: true,
                    focused_tab: "i1/t1".to_string(),
                    groups_total: 1,
                    groups_truncated: false,
                    groups: vec![GroupInfo {
                        id: "i1/g1".to_string(),
                        name: None,
                        number: 1,
                        focused: true,
                        split: false,
                        tabs_total: 1,
                        tabs_truncated: false,
                        tabs: vec![TabInfo {
                            id: "i1/t1".to_string(),
                            folder: "/etc".to_string(),
                            folder_hex: "2f657463".to_string(),
                            focused: true,
                            admin: true,
                            selection_count: 0,
                            selection: Vec::new(),
                            selection_truncated: false,
                        }],
                    }],
                }],
                jobs: Vec::new(),
            },
            error: None,
        };
        let text = state.to_toml();
        assert!(text.contains("admin = true"), "{text}");

        let error = Reply::error("r", "run", "disabled", "off");
        assert!(error.to_toml().contains("[error]"));
    }

    #[test]
    fn non_utf8_names_keep_their_bytes_in_hex() {
        let info = name_info(b"caf\xe9");
        assert_eq!(info.text, None);
        assert_eq!(info.hex, "636166e9");
    }
}
