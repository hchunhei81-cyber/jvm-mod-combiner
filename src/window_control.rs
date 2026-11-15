use crate::window_manager;
use crate::window_peer::{self, PeerIdentity};

use serde_json::{Value, json};

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PROTOCOL_VERSION: u64 = 1;
const MAX_REQUEST_BYTES: usize = 16 * 1024;
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

    /// Authenticate and process exactly one read-only request.
    pub fn handle_one<F>(&self, mut stream: UnixStream, handler: F) -> io::Result<PeerIdentity>
    where
        F: FnOnce(DryRunRequest, &PeerIdentity, Instant) -> Value,
    {
        stream.set_read_timeout(Some(Duration::from_millis(500)))?;
        stream.set_write_timeout(Some(Duration::from_millis(500)))?;

        let identity = window_peer::peer_identity(&stream)?;

        let request = match read_request(&mut stream).and_then(parse_request) {
            Ok(request) => request,
            Err(error) => {
                let response = json!({
                    "protocol": PROTOCOL_VERSION,
                    "accepted": false,
                    "transaction_ready": false,
                    "error": error.to_string(),
                });
                write_response(&mut stream, &response)?;
                return Err(error);
            }
        };

        let deadline = Instant::now() + Duration::from_secs(5);
        let response = handler(request, &identity, deadline);
        write_response(&mut stream, &response)?;

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

fn read_request(stream: &mut UnixStream) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    let deadline = Instant::now() + Duration::from_millis(500);

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "request exceeded total read deadline",
            ));
        }
        stream.set_read_timeout(Some(remaining))?;

        match stream.read(&mut byte) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "request ended before newline",
                ));
            }
            Ok(_) if byte[0] == b'\n' => return Ok(bytes),
            Ok(_) => {
                if bytes.len() >= MAX_REQUEST_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "request exceeds maximum size",
                    ));
                }
                bytes.push(byte[0]);
            }
            Err(error) => return Err(error),
        }
    }
}

fn parse_request(bytes: Vec<u8>) -> io::Result<DryRunRequest> {
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid JSON request"))?;

    let object = value.as_object().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "request must be a JSON object")
    })?;

    const ALLOWED: &[&str] = &["type", "protocol", "window_ids", "direction", "display_id"];

    if object.keys().any(|key| !ALLOWED.contains(&key.as_str())) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "request contains unknown fields",
        ));
    }

    if object.get("type").and_then(Value::as_str) != Some("dry_run") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "only dry_run requests are supported",
        ));
    }

    if object.get("protocol").and_then(Value::as_u64) != Some(PROTOCOL_VERSION) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported protocol version",
        ));
    }

    let ids = object
        .get("window_ids")
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

    let direction = object
        .get("direction")
        .and_then(Value::as_str)
        .unwrap_or("horizontal");

    if !matches!(direction, "horizontal" | "vertical" | "overlap") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "direction must be horizontal, vertical, or overlap",
        ));
    }

    let display_id = match object.get("display_id") {
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

fn write_response(stream: &mut UnixStream, response: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *stream, response)
        .map_err(|error| io::Error::other(error.to_string()))?;
    stream.write_all(b"\n")?;
    stream.flush()
}

fn remaining_time(deadline: Instant) -> io::Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());

    if remaining.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Control request exceeded its 2-second total deadline",
        ));
    }

    Ok(remaining)
}

/// Client-side socket validation. This verifies filesystem ownership and
/// permissions; it does not claim cryptographic server authentication.
pub fn request_dry_run(socket_path: &Path, request: &DryRunRequest) -> io::Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(2);
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
    stream.set_read_timeout(Some(remaining_time(deadline)?))?;
    stream.set_write_timeout(Some(remaining_time(deadline)?))?;

    let value = json!({
        "type": "dry_run",
        "protocol": PROTOCOL_VERSION,
        "window_ids": request.window_ids,
        "direction": request.direction,
        "display_id": request.display_id,
    });

    let request_bytes =
        serde_json::to_vec(&value).map_err(|error| io::Error::other(error.to_string()))?;
    stream.set_write_timeout(Some(remaining_time(deadline)?))?;
    stream.write_all(&request_bytes)?;
    stream.set_write_timeout(Some(remaining_time(deadline)?))?;
    stream.write_all(b"\n")?;
    stream.set_write_timeout(Some(remaining_time(deadline)?))?;
    stream.flush()?;

    let mut bytes = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];

    loop {
        stream.set_read_timeout(Some(remaining_time(deadline)?))?;
        match stream.read(&mut byte) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Control server closed before sending a response",
                ));
            }
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) => {
                if bytes.len() >= MAX_REQUEST_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Control response exceeds maximum size",
                    ));
                }
                bytes.push(byte[0]);
            }
            Err(error) => return Err(error),
        }
    }

    let response = serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid Control response JSON"))?;
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
            "Control request exceeded its 2-second total deadline"
        );
    }

    #[test]
    fn unexpired_client_deadline_has_remaining_time() {
        let deadline = Instant::now() + Duration::from_secs(1);

        assert!(!remaining_time(deadline).unwrap().is_zero());
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
