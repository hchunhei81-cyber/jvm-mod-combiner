use crate::window_manager;
use crate::window_peer::{self, PeerIdentity};

use serde_json::{Value, json};

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PROTOCOL_VERSION: u64 = 1;
const MAX_REQUEST_BYTES: usize = 16 * 1024;
const MAX_RESPONSE_BYTES: usize = 16 * 1024;
const MAX_WINDOW_IDS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DryRunRequest {
    pub window_ids: Vec<u32>,
    pub direction: String,
    pub display_id: Option<u32>,
}

pub struct WindowControlServer {
    listener: UnixListener,
    socket_path: PathBuf,
    socket_dev: u64,
    socket_ino: u64,
}

impl WindowControlServer {
    pub fn start() -> io::Result<Self> {
        let dir = window_manager::private_runtime_dir()?;

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();

        let socket_path = dir.join(format!("control-{}-{timestamp}.sock", std::process::id()));

        if socket_path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "Window Control socket path already exists",
            ));
        }

        let listener = UnixListener::bind(&socket_path)?;

        let metadata = match fs::symlink_metadata(&socket_path) {
            Ok(metadata) => metadata,
            Err(error) => {
                drop(listener);
                return Err(error);
            }
        };

        if !metadata.file_type().is_socket() {
            drop(listener);
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Window Control path is not a socket",
            ));
        }

        let socket_dev = metadata.dev();
        let socket_ino = metadata.ino();

        let setup = (|| {
            fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
            listener.set_nonblocking(true)?;
            Ok::<(), io::Error>(())
        })();

        if let Err(error) = setup {
            drop(listener);
            remove_socket_if_owned(&socket_path, socket_dev, socket_ino);
            return Err(error);
        }

        Ok(Self {
            listener,
            socket_path,
            socket_dev,
            socket_ino,
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Non-blocking accept. The Fusion supervisor remains responsible for
    /// deciding when and how many requests to process.
    pub fn try_accept(&self) -> io::Result<Option<UnixStream>> {
        match self.listener.accept() {
            Ok((stream, _)) => Ok(Some(stream)),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Authenticate and process one read-only request under the deadline
    /// established when the supervisor accepted the connection.
    pub fn handle_one_until<F>(
        mut stream: UnixStream,
        deadline: Instant,
        handler: F,
    ) -> io::Result<PeerIdentity>
    where
        F: FnOnce(DryRunRequest, &PeerIdentity, Instant) -> Value,
    {
        remaining_time(deadline)?;
        let identity = window_peer::peer_identity(&stream)?;
        remaining_time(deadline)?;
        stream.set_nonblocking(true)?;

        let request = match read_request(&mut stream, deadline).and_then(parse_request) {
            Ok(request) => request,
            Err(error) => {
                let response = json!({
                    "protocol": PROTOCOL_VERSION,
                    "accepted": false,
                    "transaction_ready": false,
                    "error": error.to_string(),
                });
                let _ = write_response(&mut stream, &response, deadline);
                return Err(error);
            }
        };

        remaining_time(deadline)?;
        let response = handler(request, &identity, deadline);
        remaining_time(deadline)?;
        write_response(&mut stream, &response, deadline)?;

        Ok(identity)
    }
}

impl Drop for WindowControlServer {
    fn drop(&mut self) {
        remove_socket_if_owned(&self.socket_path, self.socket_dev, self.socket_ino);
    }
}

fn remove_socket_if_owned(path: &Path, dev: u64, ino: u64) {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_socket() && metadata.dev() == dev && metadata.ino() == ino {
            let _ = fs::remove_file(path);
        }
    }
}

fn wait_nonblocking_retry(deadline: Instant) -> io::Result<()> {
    let remaining = remaining_time(deadline)?;
    thread::sleep(remaining.min(Duration::from_millis(2)));
    Ok(())
}

fn read_line_nonblocking_until(
    stream: &mut UnixStream,
    deadline: Instant,
    max_bytes: usize,
    eof_message: &'static str,
) -> io::Result<Vec<u8>> {
    stream.set_nonblocking(true)?;

    let mut bytes = Vec::with_capacity(512);
    let mut byte = [0_u8; 1];

    loop {
        remaining_time(deadline)?;

        match stream.read(&mut byte) {
            Ok(0) => {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, eof_message));
            }
            Ok(_) if byte[0] == b'\n' => return Ok(bytes),
            Ok(_) => {
                if bytes.len() >= max_bytes {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "line exceeds maximum size",
                    ));
                }

                bytes.push(byte[0]);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                wait_nonblocking_retry(deadline)?;
            }
            Err(error) => return Err(error),
        }
    }
}

fn write_nonblocking_until(
    stream: &mut UnixStream,
    bytes: &[u8],
    deadline: Instant,
) -> io::Result<()> {
    stream.set_nonblocking(true)?;

    let mut offset = 0;

    while offset < bytes.len() {
        remaining_time(deadline)?;

        match stream.write(&bytes[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "failed to write Control socket data",
                ));
            }
            Ok(written) => offset += written,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                wait_nonblocking_retry(deadline)?;
            }
            Err(error) => return Err(error),
        }
    }

    remaining_time(deadline)?;
    Ok(())
}

fn read_request(stream: &mut UnixStream, request_deadline: Instant) -> io::Result<Vec<u8>> {
    read_line_nonblocking_until(
        stream,
        request_deadline,
        MAX_REQUEST_BYTES,
        "request ended before newline",
    )
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDryRunRequest {
    #[serde(rename = "type")]
    request_type: Option<Value>,
    protocol: Option<Value>,
    window_ids: Option<Value>,
    #[serde(default, deserialize_with = "deserialize_present_json_value")]
    direction: Option<Value>,
    display_id: Option<Value>,
}

fn deserialize_present_json_value<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = <Value as serde::Deserialize>::deserialize(deserializer)?;
    Ok(Some(value))
}

fn parse_request(bytes: Vec<u8>) -> io::Result<DryRunRequest> {
    let raw: RawDryRunRequest = serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid JSON request"))?;

    if raw.request_type.as_ref().and_then(Value::as_str) != Some("dry_run") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "only dry_run requests are supported",
        ));
    }

    if raw.protocol.as_ref().and_then(Value::as_u64) != Some(PROTOCOL_VERSION) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported protocol version",
        ));
    }

    let ids = raw
        .window_ids
        .as_ref()
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "window_ids must be an array"))?;

    if !(2..=MAX_WINDOW_IDS).contains(&ids.len()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "window_ids must contain between 2 and 16 IDs",
        ));
    }

    let mut window_ids = Vec::with_capacity(ids.len());

    for value in ids {
        let id = value
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
            .filter(|id| *id > 0)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "window IDs must be positive u32 integers",
                )
            })?;

        if window_ids.contains(&id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "duplicate window ID",
            ));
        }

        window_ids.push(id);
    }

    let direction = match raw.direction.as_ref() {
        None => "horizontal",
        Some(Value::String(value)) => value.as_str(),
        Some(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "direction must be a string",
            ));
        }
    };

    if !matches!(direction, "horizontal" | "vertical" | "overlap") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "direction must be horizontal, vertical, or overlap",
        ));
    }

    let display_id = match raw.display_id.as_ref() {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .and_then(|id| u32::try_from(id).ok())
                .filter(|id| *id > 0)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "display_id must be null or a positive u32",
                    )
                })?,
        ),
    };

    Ok(DryRunRequest {
        window_ids,
        direction: direction.to_owned(),
        display_id,
    })
}

struct BoundedVecWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl BoundedVecWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit),
            limit,
            exceeded: false,
        }
    }
}

impl Write for BoundedVecWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "serialized response exceeds maximum size",
            ));
        }

        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

enum BoundedSerializationError {
    TooLarge,
    Serialization(serde_json::Error),
}

fn serialize_response_bounded(response: &Value) -> Result<Vec<u8>, BoundedSerializationError> {
    let mut writer = BoundedVecWriter::new(MAX_RESPONSE_BYTES);

    match serde_json::to_writer(&mut writer, response) {
        Ok(()) => Ok(writer.bytes),
        Err(_) if writer.exceeded => Err(BoundedSerializationError::TooLarge),
        Err(error) => Err(BoundedSerializationError::Serialization(error)),
    }
}

fn write_response(stream: &mut UnixStream, response: &Value, deadline: Instant) -> io::Result<()> {
    remaining_time(deadline)?;

    let mut bytes = match serialize_response_bounded(response) {
        Ok(bytes) => bytes,
        Err(BoundedSerializationError::TooLarge) => {
            let fallback = json!({
                "protocol": PROTOCOL_VERSION,
                "accepted": false,
                "transaction_ready": false,
                "error": "Control response exceeds maximum size"
            });

            match serialize_response_bounded(&fallback) {
                Ok(bytes) => bytes,
                Err(BoundedSerializationError::TooLarge) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Control fallback response exceeds maximum size",
                    ));
                }
                Err(BoundedSerializationError::Serialization(error)) => {
                    return Err(io::Error::other(error.to_string()));
                }
            }
        }
        Err(BoundedSerializationError::Serialization(error)) => {
            return Err(io::Error::other(error.to_string()));
        }
    };

    // MAX_RESPONSE_BYTES applies to the JSON body; LF is the framing byte.
    bytes.push(b'\n');
    write_nonblocking_until(stream, &bytes, deadline)
}

fn remaining_time(deadline: Instant) -> io::Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());

    if remaining.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Control request exceeded its total deadline",
        ));
    }

    Ok(remaining)
}

fn has_exact_keys(object: &serde_json::Map<String, Value>, expected: &[&str]) -> bool {
    object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
}

fn validate_response_bounds(value: &Value) -> bool {
    let Some(bounds) = value.as_object() else {
        return false;
    };

    if !has_exact_keys(bounds, &["x", "y", "width", "height"]) {
        return false;
    }

    let Some(x) = bounds.get("x").and_then(Value::as_f64) else {
        return false;
    };
    let Some(y) = bounds.get("y").and_then(Value::as_f64) else {
        return false;
    };
    let Some(width) = bounds.get("width").and_then(Value::as_f64) else {
        return false;
    };
    let Some(height) = bounds.get("height").and_then(Value::as_f64) else {
        return false;
    };

    x.is_finite()
        && y.is_finite()
        && width.is_finite()
        && height.is_finite()
        && (x + width).is_finite()
        && (y + height).is_finite()
        && width > 0.0
        && height > 0.0
}

fn validate_response_contract(response: Value) -> io::Result<Value> {
    let invalid = |message: &str| io::Error::new(io::ErrorKind::InvalidData, message.to_owned());

    let object = response
        .as_object()
        .ok_or_else(|| invalid("Control response must be a JSON object"))?;

    if object.get("protocol").and_then(Value::as_u64) != Some(PROTOCOL_VERSION as u64) {
        return Err(invalid("Control response protocol version is invalid"));
    }

    let accepted = object
        .get("accepted")
        .and_then(Value::as_bool)
        .ok_or_else(|| invalid("Control response accepted field must be boolean"))?;

    let expected_top_level = if accepted {
        &[
            "protocol",
            "accepted",
            "transaction_ready",
            "requester",
            "display_id",
            "direction",
            "window_count",
            "entries",
            "notice",
        ][..]
    } else {
        &["protocol", "accepted", "transaction_ready", "error"][..]
    };

    if !has_exact_keys(object, expected_top_level) {
        return Err(invalid(
            "Control response contains unknown or missing top-level fields",
        ));
    }

    let transaction_ready = object
        .get("transaction_ready")
        .and_then(Value::as_bool)
        .ok_or_else(|| invalid("Control response transaction_ready field must be boolean"))?;

    if transaction_ready {
        return Err(invalid(
            "Control response cannot authorize window mutations",
        ));
    }

    if accepted {
        let requester = object
            .get("requester")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("Accepted response is missing requester"))?;

        if !has_exact_keys(requester, &["pid", "pid_version"]) {
            return Err(invalid(
                "Accepted response requester contains unknown or missing fields",
            ));
        }

        let requester_pid = requester
            .get("pid")
            .and_then(Value::as_u64)
            .filter(|pid| *pid > 0 && *pid <= u32::MAX as u64);

        let requester_pid_version = requester
            .get("pid_version")
            .and_then(Value::as_i64)
            .filter(|version| *version > 0 && *version <= i32::MAX as i64);

        if requester_pid.is_none() || requester_pid_version.is_none() {
            return Err(invalid("Accepted response requester identity is invalid"));
        }

        if !object
            .get("display_id")
            .and_then(Value::as_u64)
            .is_some_and(|id| id > 0 && id <= u32::MAX as u64)
        {
            return Err(invalid("Accepted response display_id is invalid"));
        }

        match object.get("direction").and_then(Value::as_str) {
            Some("horizontal") | Some("vertical") | Some("overlap") => {}
            _ => return Err(invalid("Accepted response direction is invalid")),
        }

        let count = object
            .get("window_count")
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid("Accepted response window_count is invalid"))?;

        if !(2..=16).contains(&count) {
            return Err(invalid(
                "Accepted response window_count must be between 2 and 16",
            ));
        }

        let entries = object
            .get("entries")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("Accepted response entries must be an array"))?;

        if count != entries.len() as u64 {
            return Err(invalid(
                "Accepted response window_count does not match entries",
            ));
        }

        let mut window_ids = std::collections::HashSet::with_capacity(entries.len());

        for entry in entries {
            let entry = entry
                .as_object()
                .ok_or_else(|| invalid("Control response entry must be an object"))?;

            if !has_exact_keys(
                entry,
                &[
                    "window_id",
                    "pid",
                    "loader",
                    "peer_pid_version",
                    "title",
                    "owner",
                    "accessibility_index",
                    "current_bounds",
                    "target_bounds",
                ],
            ) {
                return Err(invalid(
                    "Control response entry contains unknown or missing fields",
                ));
            }

            let window_id = entry
                .get("window_id")
                .and_then(Value::as_u64)
                .filter(|id| *id > 0 && *id <= u32::MAX as u64)
                .ok_or_else(|| invalid("Control response entry window_id is invalid"))?;

            if !window_ids.insert(window_id) {
                return Err(invalid("Control response contains duplicate window IDs"));
            }

            if !entry
                .get("pid")
                .and_then(Value::as_u64)
                .is_some_and(|pid| pid > 0 && pid <= u32::MAX as u64)
            {
                return Err(invalid("Control response entry PID is invalid"));
            }

            if !entry
                .get("peer_pid_version")
                .and_then(Value::as_i64)
                .is_some_and(|version| version > 0 && version <= i32::MAX as i64)
            {
                return Err(invalid(
                    "Control response entry peer_pid_version is invalid",
                ));
            }

            match entry.get("loader").and_then(Value::as_str) {
                Some("fabric") | Some("forge") | Some("neoforge") => {}
                _ => return Err(invalid("Control response entry loader is invalid")),
            }

            for field in ["title", "owner"] {
                if entry
                    .get(field)
                    .and_then(Value::as_str)
                    .is_none_or(|value| value.trim().is_empty())
                {
                    return Err(invalid("Control response entry title or owner is missing"));
                }
            }

            if entry
                .get("accessibility_index")
                .and_then(Value::as_u64)
                .is_none()
            {
                return Err(invalid(
                    "Control response entry accessibility_index is invalid",
                ));
            }

            if !entry
                .get("current_bounds")
                .is_some_and(validate_response_bounds)
            {
                return Err(invalid("Control response entry current_bounds is invalid"));
            }

            if !entry
                .get("target_bounds")
                .is_some_and(validate_response_bounds)
            {
                return Err(invalid("Control response entry target_bounds is invalid"));
            }
        }

        if object
            .get("notice")
            .and_then(Value::as_str)
            .is_none_or(|notice| notice.trim().is_empty())
        {
            return Err(invalid("Accepted response notice is missing"));
        }
    } else if object
        .get("error")
        .and_then(Value::as_str)
        .is_none_or(|error| error.trim().is_empty())
    {
        return Err(invalid("Rejected response is missing its error"));
    }

    Ok(response)
}

fn validate_response_for_request(response: Value, request: &DryRunRequest) -> io::Result<Value> {
    let response = validate_response_contract(response)?;

    if response.get("accepted").and_then(Value::as_bool) == Some(true) {
        let entries = response
            .get("entries")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Accepted response entries must be an array",
                )
            })?;

        let requested: std::collections::HashSet<u32> =
            request.window_ids.iter().copied().collect();

        let returned: std::collections::HashSet<u32> = entries
            .iter()
            .filter_map(|entry| {
                entry
                    .get("window_id")
                    .and_then(Value::as_u64)
                    .and_then(|id| u32::try_from(id).ok())
            })
            .collect();

        if requested != returned || requested.len() != request.window_ids.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Accepted response window IDs do not match the request",
            ));
        }
    }

    Ok(response)
}

fn validate_client_request(request: &DryRunRequest) -> io::Result<()> {
    if !(2..=MAX_WINDOW_IDS).contains(&request.window_ids.len()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "window_ids must contain between 2 and 16 entries",
        ));
    }

    let mut unique = std::collections::HashSet::with_capacity(request.window_ids.len());

    for id in &request.window_ids {
        if *id == 0 || !unique.insert(*id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "window_ids must be unique positive IDs",
            ));
        }
    }

    if !matches!(
        request.direction.as_str(),
        "horizontal" | "vertical" | "overlap"
    ) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsupported layout direction",
        ));
    }

    if request.display_id == Some(0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "display_id must be positive",
        ));
    }

    Ok(())
}

/// Client-side socket validation. This verifies filesystem ownership and
/// permissions; it does not claim cryptographic server authentication.
pub fn request_dry_run(socket_path: &Path, request: &DryRunRequest) -> io::Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(12);
    validate_client_request(request)?;

    let metadata = fs::symlink_metadata(socket_path)?;
    let uid = unsafe { libc::getuid() };

    if !metadata.file_type().is_socket()
        || metadata.uid() != uid
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Control socket ownership, type, or permissions are invalid",
        ));
    }

    remaining_time(deadline)?;

    // UnixStream::connect has no timeout parameter.
    // Enforce the budget immediately after it returns.
    let mut stream = UnixStream::connect(socket_path)?;
    remaining_time(deadline)?;
    stream.set_nonblocking(true)?;

    let value = json!({
        "type": "dry_run",
        "protocol": PROTOCOL_VERSION,
        "window_ids": request.window_ids,
        "direction": request.direction,
        "display_id": request.display_id,
    });

    let mut request_bytes =
        serde_json::to_vec(&value).map_err(|error| io::Error::other(error.to_string()))?;
    request_bytes.push(b'\n');

    write_nonblocking_until(&mut stream, &request_bytes, deadline)?;

    let bytes = read_line_nonblocking_until(
        &mut stream,
        deadline,
        MAX_RESPONSE_BYTES,
        "Control server closed before sending a response",
    )?;

    let response = serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid Control response JSON"))?;

    let response = validate_response_for_request(response, request)?;
    remaining_time(deadline)?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_client_deadline_returns_timed_out() {
        let deadline = Instant::now() - Duration::from_secs(1);
        let error = remaining_time(deadline).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(
            error.to_string(),
            "Control request exceeded its total deadline"
        );
    }

    fn valid_success_response() -> Value {
        json!({
            "protocol": 1,
            "accepted": true,
            "transaction_ready": false,
            "requester": {
                "pid": 123,
                "pid_version": 7
            },
            "display_id": 1,
            "direction": "horizontal",
            "window_count": 2,
            "entries": [
                {
                    "window_id": 101,
                    "pid": 123,
                    "loader": "fabric",
                    "peer_pid_version": 7,
                    "title": "Minecraft",
                    "owner": "java",
                    "accessibility_index": 0,
                    "current_bounds": {
                        "x": 0.0, "y": 0.0, "width": 800.0, "height": 600.0
                    },
                    "target_bounds": {
                        "x": 0.0, "y": 0.0, "width": 800.0, "height": 600.0
                    }
                },
                {
                    "window_id": 102,
                    "pid": 124,
                    "loader": "neoforge",
                    "peer_pid_version": 8,
                    "title": "Minecraft 2",
                    "owner": "java",
                    "accessibility_index": 1,
                    "current_bounds": {
                        "x": 800.0, "y": 0.0, "width": 800.0, "height": 600.0
                    },
                    "target_bounds": {
                        "x": 800.0, "y": 0.0, "width": 800.0, "height": 600.0
                    }
                }
            ],
            "notice": "Read-only preview."
        })
    }

    #[test]
    fn rejects_unknown_top_level_response_fields() {
        let mut response = valid_success_response();
        response["unexpected"] = json!(true);
        assert!(validate_response_contract(response).is_err());

        let response = json!({
            "protocol": 1,
            "accepted": false,
            "transaction_ready": false,
            "error": "denied",
            "unexpected": true
        });
        assert!(validate_response_contract(response).is_err());
    }

    #[test]
    fn rejects_unknown_nested_response_fields() {
        let mut response = valid_success_response();
        response["requester"]["unexpected"] = json!(true);
        assert!(validate_response_contract(response).is_err());

        let mut response = valid_success_response();
        response["entries"][0]["unexpected"] = json!(true);
        assert!(validate_response_contract(response).is_err());

        let mut response = valid_success_response();
        response["entries"][0]["current_bounds"]["unexpected"] = json!(true);
        assert!(validate_response_contract(response).is_err());
    }

    #[test]
    fn response_window_ids_must_match_request_as_a_set() {
        let request = DryRunRequest {
            window_ids: vec![101, 102],
            direction: "horizontal".to_owned(),
            display_id: None,
        };

        let mut response = valid_success_response();
        response["entries"].as_array_mut().unwrap().swap(0, 1);

        assert!(validate_response_for_request(response, &request).is_ok());

        let request = DryRunRequest {
            window_ids: vec![101, 103],
            direction: "horizontal".to_owned(),
            display_id: None,
        };

        assert!(validate_response_for_request(valid_success_response(), &request).is_err());
    }

    #[test]
    fn accepts_valid_read_only_success_response() {
        assert!(validate_response_contract(valid_success_response()).is_ok());
    }

    #[test]
    fn accepts_valid_rejection_response() {
        let response = json!({
            "protocol": 1,
            "accepted": false,
            "transaction_ready": false,
            "error": "Authorization failed"
        });

        assert!(validate_response_contract(response).is_ok());
    }

    #[test]
    fn rejects_non_object_response() {
        assert!(validate_response_contract(json!([])).is_err());
        assert!(validate_response_contract(json!("accepted")).is_err());
        assert!(validate_response_contract(Value::Null).is_err());
    }

    #[test]
    fn rejects_wrong_response_protocol() {
        let mut response = valid_success_response();
        response["protocol"] = json!(2);

        assert!(validate_response_contract(response).is_err());
    }

    #[test]
    fn rejects_response_with_transaction_authority() {
        let mut response = valid_success_response();
        response["transaction_ready"] = json!(true);

        assert!(validate_response_contract(response).is_err());
    }

    #[test]
    fn rejects_success_response_with_missing_entries() {
        let mut response = valid_success_response();
        response.as_object_mut().unwrap().remove("entries");

        assert!(validate_response_contract(response).is_err());
    }

    #[test]
    fn rejects_success_response_with_inconsistent_entry_count() {
        let mut response = valid_success_response();
        response["window_count"] = json!(3);

        assert!(validate_response_contract(response).is_err());
    }

    #[test]
    fn rejects_success_response_with_null_display_id() {
        let mut response = valid_success_response();
        response["display_id"] = Value::Null;

        assert!(validate_response_contract(response).is_err());
    }

    #[test]
    fn rejects_success_response_outside_window_count_limits() {
        let mut response = valid_success_response();
        response["window_count"] = json!(1);
        response["entries"] = json!([response["entries"][0].clone()]);
        assert!(validate_response_contract(response).is_err());

        let mut response = valid_success_response();
        let template = response["entries"][0].clone();

        let entries: Vec<Value> = (1..=17)
            .map(|id| {
                let mut entry = template.clone();
                entry["window_id"] = json!(100 + id);
                entry
            })
            .collect();

        response["window_count"] = json!(17);
        response["entries"] = json!(entries);

        assert!(validate_response_contract(response).is_err());
    }
    #[test]
    fn rejects_success_response_with_duplicate_window_ids() {
        let mut response = valid_success_response();
        let duplicate = response["entries"][0].clone();
        response["entries"][1] = duplicate;

        assert!(validate_response_contract(response).is_err());
    }

    #[test]
    fn rejects_success_response_with_invalid_entry_identity() {
        let mut response = valid_success_response();
        response["entries"][0]["pid"] = json!(0);

        assert!(validate_response_contract(response).is_err());

        let mut response = valid_success_response();
        response["entries"][0]["loader"] = json!("unknown");

        assert!(validate_response_contract(response).is_err());
    }

    #[test]
    fn rejects_success_response_with_invalid_geometry() {
        let mut response = valid_success_response();
        response["entries"][0]["target_bounds"]["width"] = json!(-1.0);

        assert!(validate_response_contract(response).is_err());

        let mut response = valid_success_response();
        response["entries"][0]["current_bounds"]["x"] = json!(1.0e308);
        response["entries"][0]["current_bounds"]["width"] = json!(1.0e308);

        assert!(validate_response_contract(response).is_err());
    }

    #[test]
    fn rejects_success_response_with_missing_entry_fields() {
        let mut response = valid_success_response();
        response["entries"][0]
            .as_object_mut()
            .unwrap()
            .remove("title");

        assert!(validate_response_contract(response).is_err());
    }

    #[test]
    fn rejects_rejection_without_error() {
        let response = json!({
            "protocol": 1,
            "accepted": false,
            "transaction_ready": false
        });

        assert!(validate_response_contract(response).is_err());
    }

    #[test]
    fn unexpired_client_deadline_has_remaining_time() {
        let deadline = Instant::now() + Duration::from_secs(1);

        assert!(!remaining_time(deadline).unwrap().is_zero());
    }

    #[test]
    fn read_request_rejects_expired_absolute_deadline() {
        let (mut server, _client) = UnixStream::pair().unwrap();
        let deadline = Instant::now() - Duration::from_millis(1);

        let error = read_request(&mut server, deadline).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn write_response_rejects_expired_absolute_deadline() {
        let (mut server, _client) = UnixStream::pair().unwrap();
        let deadline = Instant::now() - Duration::from_millis(1);

        let error = write_response(&mut server, &json!({"accepted": true}), deadline).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn write_response_terminates_with_one_lf_byte() {
        let (mut server, mut client) = UnixStream::pair().unwrap();
        let response = json!({"accepted": true});
        let expected = serde_json::to_vec(&response).unwrap();

        write_response(
            &mut server,
            &response,
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();

        let mut actual = vec![0; expected.len() + 1];
        client.read_exact(&mut actual).unwrap();

        let mut expected_with_lf = expected;
        expected_with_lf.push(b'\n');

        assert_eq!(actual, expected_with_lf);
    }

    #[test]
    fn oversized_response_is_replaced_with_bounded_rejection() {
        let (mut server, mut client) = UnixStream::pair().unwrap();
        let response = json!({
            "protocol": 1,
            "accepted": true,
            "transaction_ready": false,
            "payload": "x".repeat(MAX_RESPONSE_BYTES + 1024)
        });

        write_response(
            &mut server,
            &response,
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
        drop(server);

        let mut line = Vec::new();
        client.read_to_end(&mut line).unwrap();

        assert!(line.len() <= MAX_RESPONSE_BYTES + 1);
        assert_eq!(line.last(), Some(&b'\n'));

        let parsed: Value = serde_json::from_slice(&line[..line.len() - 1]).unwrap();

        assert_eq!(parsed["accepted"], false);
        assert_eq!(parsed["transaction_ready"], false);
        assert_eq!(parsed["error"], "Control response exceeds maximum size");

        assert!(validate_response_contract(parsed).is_ok());
    }

    #[test]
    fn bounded_response_serialization_enforces_exact_size_limit() {
        for size in [MAX_RESPONSE_BYTES - 1, MAX_RESPONSE_BYTES] {
            let response = Value::String("x".repeat(size - 2));
            let serialized = serde_json::to_vec(&response).unwrap();
            assert_eq!(serialized.len(), size);

            match serialize_response_bounded(&response) {
                Ok(bytes) => assert_eq!(bytes.len(), size),
                Err(_) => panic!("response of {size} bytes exceeded the bound"),
            }
        }

        let response = Value::String("x".repeat(MAX_RESPONSE_BYTES - 1));
        assert_eq!(
            serde_json::to_vec(&response).unwrap().len(),
            MAX_RESPONSE_BYTES + 1
        );
        assert!(matches!(
            serialize_response_bounded(&response),
            Err(BoundedSerializationError::TooLarge)
        ));
    }

    fn request(value: Value) -> io::Result<DryRunRequest> {
        parse_request(serde_json::to_vec(&value).unwrap())
    }

    #[test]
    fn accepts_valid_dry_run_request() {
        let parsed = request(json!({
            "type": "dry_run",
            "protocol": 1,
            "window_ids": [101, 202],
            "direction": "horizontal",
            "display_id": null
        }))
        .unwrap();

        assert_eq!(parsed.window_ids, vec![101, 202]);
        assert_eq!(parsed.direction, "horizontal");
        assert_eq!(parsed.display_id, None);
    }

    #[test]
    fn defaults_missing_direction_to_horizontal() {
        let parsed = request(json!({
            "type": "dry_run",
            "protocol": 1,
            "window_ids": [101, 202]
        }))
        .unwrap();

        assert_eq!(parsed.direction, "horizontal");
    }

    #[test]
    fn rejects_unknown_fields() {
        assert!(
            request(json!({
                "type": "dry_run",
                "protocol": 1,
                "window_ids": [101, 202],
                "unexpected": true
            }))
            .is_err()
        );
    }

    fn request_raw(raw: &str) -> io::Result<DryRunRequest> {
        parse_request(raw.as_bytes().to_vec())
    }

    #[test]
    fn rejects_duplicate_json_object_fields() {
        let requests = [
            r#"{"type":"dry_run","type":"dry_run","protocol":1,"window_ids":[101,202]}"#,
            r#"{"type":"dry_run","protocol":1,"protocol":1,"window_ids":[101,202]}"#,
            r#"{"type":"dry_run","protocol":1,"window_ids":[101,202],"window_ids":[303,404]}"#,
            r#"{"type":"dry_run","protocol":1,"window_ids":[101,202],"direction":"horizontal","direction":"vertical"}"#,
            r#"{"type":"dry_run","protocol":1,"window_ids":[101,202],"display_id":null,"display_id":1}"#,
        ];

        for raw in requests {
            let result = request_raw(raw);
            assert!(result.is_err(), "duplicate JSON field was accepted: {raw}");
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        }
    }

    #[test]
    fn rejects_wrong_protocol() {
        assert!(
            request(json!({
                "type": "dry_run",
                "protocol": 2,
                "window_ids": [101, 202]
            }))
            .is_err()
        );
    }

    #[test]
    fn rejects_duplicate_window_ids() {
        assert!(
            request(json!({
                "type": "dry_run",
                "protocol": 1,
                "window_ids": [101, 101]
            }))
            .is_err()
        );
    }

    #[test]
    fn rejects_invalid_direction() {
        assert!(
            request(json!({
                "type": "dry_run",
                "protocol": 1,
                "window_ids": [101, 202],
                "direction": "apply"
            }))
            .is_err()
        );
    }

    #[test]
    fn rejects_non_string_direction() {
        let invalid_directions = [json!(null), json!(123), json!(true), json!([]), json!({})];

        for direction in invalid_directions {
            let result = request(json!({
                "type": "dry_run",
                "protocol": 1,
                "window_ids": [101, 202],
                "direction": direction
            }));

            assert!(
                result.is_err(),
                "non-string direction was accepted: {direction}"
            );
        }
    }

    #[test]
    fn client_request_validation_rejects_invalid_inputs_before_socket_access() {
        let socket_path = std::env::temp_dir().join(format!(
            "missing-control-socket-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let invalid_requests = [
            DryRunRequest {
                window_ids: vec![101],
                direction: "horizontal".to_owned(),
                display_id: None,
            },
            DryRunRequest {
                window_ids: (1..=MAX_WINDOW_IDS as u32 + 1).collect(),
                direction: "horizontal".to_owned(),
                display_id: None,
            },
            DryRunRequest {
                window_ids: vec![101, 101],
                direction: "horizontal".to_owned(),
                display_id: None,
            },
            DryRunRequest {
                window_ids: vec![0, 101],
                direction: "horizontal".to_owned(),
                display_id: None,
            },
            DryRunRequest {
                window_ids: vec![101, 102],
                direction: "diagonal".to_owned(),
                display_id: None,
            },
            DryRunRequest {
                window_ids: vec![101, 102],
                direction: "horizontal".to_owned(),
                display_id: Some(0),
            },
        ];

        for request in invalid_requests {
            let error = request_dry_run(&socket_path, &request).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn rejects_too_many_window_ids() {
        let ids: Vec<u32> = (1..=17).collect();

        assert!(
            request(json!({
                "type": "dry_run",
                "protocol": 1,
                "window_ids": ids
            }))
            .is_err()
        );
    }
}
