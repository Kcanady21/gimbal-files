// SPDX-License-Identifier: GPL-3.0-or-later
//! The client half of the local endpoint, used by `gimbal-files-cli`.
//!
//! It never starts the app. A missing socket means Gimbal is not running or
//! local automation is off; the CLI prints that and points at Settings.

use std::fmt;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::protocol::{self, Request};
use crate::transport::{self, SOCKET_NAME};

/// How long a CLI request waits for one reply.
pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub enum ClientError {
    /// No endpoint: Gimbal is not running, or the switch is Off.
    Unavailable(String),
    /// The peer took too long. The outcome is unknown, never proof of failure.
    Timeout,
    /// The connection failed or ended after submission.
    Transport(String),
    /// The reply was not a valid V1 reply.
    Protocol(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Unavailable(message) => f.write_str(message),
            ClientError::Timeout => f.write_str(
                "Gimbal did not answer before the timeout; the request's outcome is unknown",
            ),
            ClientError::Transport(message) => write!(f, "the local endpoint failed: {message}"),
            ClientError::Protocol(message) => write!(f, "Gimbal sent a bad reply: {message}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// The path a client would use for a runtime directory.
pub fn socket_path(runtime_dir: &Path) -> std::path::PathBuf {
    transport::socket_path(runtime_dir)
}

/// Send one request and return the reply document text. The request is never
/// replayed automatically.
pub fn request(
    runtime_dir: &Path,
    request: &Request,
    timeout: Duration,
) -> Result<String, ClientError> {
    let path = socket_path(runtime_dir);
    let mut stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Err(ClientError::Unavailable(format!(
                "Gimbal is not running, or local automation is off ({} is absent). \
                 Enable it in Settings -> Allow local automation.",
                path.display()
            )));
        }
        Err(error) => {
            return Err(ClientError::Unavailable(format!(
                "cannot reach Gimbal at {}: {error}. Enable Settings -> Allow local automation.",
                path.display()
            )));
        }
    };

    let step = Duration::from_millis(200);
    stream
        .set_write_timeout(Some(step))
        .map_err(|error| ClientError::Transport(error.to_string()))?;
    stream
        .set_read_timeout(Some(step))
        .map_err(|error| ClientError::Transport(error.to_string()))?;

    let payload = request.to_toml().into_bytes();
    let framed = protocol::frame_bytes(&payload)
        .map_err(|error| ClientError::Protocol(error.to_string()))?;
    let deadline = Instant::now() + timeout;
    let mut written = 0;
    while written < framed.len() {
        if Instant::now() >= deadline {
            return Err(ClientError::Timeout);
        }
        match stream.write(&framed[written..]) {
            Ok(0) => {
                return Err(ClientError::Transport(
                    "Gimbal closed the connection".to_string(),
                ));
            }
            Ok(count) => written += count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(ClientError::Transport(error.to_string())),
        }
    }

    let frame = read_reply_frame(&mut stream, deadline)?;
    let text = String::from_utf8(frame)
        .map_err(|_| ClientError::Protocol("the reply is not UTF-8".to_string()))?;
    validate_reply(&text, &request.request_id)?;
    Ok(text)
}

fn read_reply_frame(stream: &mut UnixStream, deadline: Instant) -> Result<Vec<u8>, ClientError> {
    let mut length = [0_u8; 4];
    let mut read = 0;
    while read < length.len() {
        if Instant::now() >= deadline {
            return Err(ClientError::Timeout);
        }
        match stream.read(&mut length[read..]) {
            Ok(0) if read == 0 => {
                return Err(ClientError::Transport(
                    "Gimbal closed the connection before replying".to_string(),
                ));
            }
            Ok(0) => return Err(ClientError::Transport("a truncated reply".to_string())),
            Ok(count) => read += count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(ClientError::Transport(error.to_string())),
        }
    }
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > protocol::MAX_FRAME_BYTES {
        return Err(ClientError::Protocol(
            "the reply frame length is invalid".to_string(),
        ));
    }
    let mut payload = vec![0_u8; length];
    let mut read = 0;
    while read < payload.len() {
        if Instant::now() >= deadline {
            return Err(ClientError::Timeout);
        }
        match stream.read(&mut payload[read..]) {
            Ok(0) => return Err(ClientError::Transport("a truncated reply".to_string())),
            Ok(count) => read += count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(ClientError::Transport(error.to_string())),
        }
    }
    Ok(payload)
}

/// Check the envelope the CLI relies on without re-encoding the reply.
fn validate_reply(text: &str, request_id: &str) -> Result<(), ClientError> {
    let document = toml_edit::Document::parse(text)
        .map_err(|error| ClientError::Protocol(error.message().to_string()))?;
    let root = document.as_table();
    let version = root
        .get("version")
        .and_then(toml_edit::Item::as_integer)
        .ok_or_else(|| ClientError::Protocol("missing version".to_string()))?;
    if version != protocol::VERSION {
        return Err(ClientError::Protocol(format!(
            "unsupported reply version {version}"
        )));
    }
    let replied = root
        .get("request_id")
        .and_then(toml_edit::Item::as_str)
        .ok_or_else(|| ClientError::Protocol("missing request_id".to_string()))?;
    if replied != request_id {
        return Err(ClientError::Protocol(
            "the reply does not match the request".to_string(),
        ));
    }
    Ok(())
}

/// Parse a reply document for the CLI's own status checks and tests.
pub fn parse_reply(text: &str) -> Result<ReplyEnvelope, ClientError> {
    let document = toml_edit::Document::parse(text)
        .map_err(|error| ClientError::Protocol(error.message().to_string()))?;
    let root = document.as_table();
    let status = root
        .get("status")
        .and_then(toml_edit::Item::as_str)
        .ok_or_else(|| ClientError::Protocol("missing status".to_string()))?;
    let operation = root
        .get("operation")
        .and_then(toml_edit::Item::as_str)
        .unwrap_or("")
        .to_string();
    let request_id = root
        .get("request_id")
        .and_then(toml_edit::Item::as_str)
        .unwrap_or("")
        .to_string();
    let error_code = root
        .get("error")
        .and_then(toml_edit::Item::as_table)
        .and_then(|table| table.get("code"))
        .and_then(toml_edit::Item::as_str)
        .map(str::to_string);
    let job = root
        .get("data")
        .and_then(toml_edit::Item::as_table)
        .and_then(|table| table.get("job"))
        .and_then(toml_edit::Item::as_str)
        .map(str::to_string);
    let state = root
        .get("data")
        .and_then(toml_edit::Item::as_table)
        .and_then(|table| table.get("state"))
        .and_then(toml_edit::Item::as_str)
        .map(str::to_string);
    let instance = root
        .get("instance")
        .and_then(toml_edit::Item::as_str)
        .map(str::to_string);
    let generation = root
        .get("generation")
        .and_then(toml_edit::Item::as_integer)
        .and_then(|value| u64::try_from(value).ok());
    Ok(ReplyEnvelope {
        request_id,
        operation,
        status: status.to_string(),
        error_code,
        job,
        state,
        instance,
        generation,
    })
}

/// The few envelope fields the CLI acts on. The full document is printed
/// verbatim; this is not a second wire decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyEnvelope {
    pub request_id: String,
    pub operation: String,
    pub status: String,
    pub error_code: Option<String>,
    pub job: Option<String>,
    pub state: Option<String>,
    pub instance: Option<String>,
    pub generation: Option<u64>,
}

/// The socket file name, re-exported so help can name it.
pub const FILE_NAME: &str = SOCKET_NAME;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_socket_is_reported_as_not_running() {
        let directory = format!(
            "/tmp/gimbal-agent-client-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|time| time.as_nanos())
                .unwrap_or(0)
        );
        // No folder and no socket: the error names both possibilities.
        let request = Request {
            request_id: "r1".to_string(),
            instance: None,
            generation: None,
            operation: protocol::Operation::State {
                offset: 0,
                limit: 5,
            },
        };
        let error = request_with(&directory, &request).expect_err("missing");
        assert!(matches!(error, ClientError::Unavailable(_)));
        assert!(error.to_string().contains("Settings"));
    }

    fn request_with(directory: &str, outgoing: &Request) -> Result<String, ClientError> {
        request(Path::new(directory), outgoing, Duration::from_millis(200))
    }

    #[test]
    fn reply_validation_matches_the_request_id_and_version() {
        let text = "version = 1\nrequest_id = \"r1\"\noperation = \"state\"\nstatus = \"ok\"\n";
        assert!(validate_reply(text, "r1").is_ok());
        assert!(validate_reply(text, "r2").is_err());
        assert!(validate_reply("version = 2\nrequest_id = \"r1\"\n", "r1").is_err());
    }
}
