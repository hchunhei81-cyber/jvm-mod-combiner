use crate::window_manager;
use crate::window_peer;

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const PROTOCOL_VERSION: u64 = 1;
const MAX_REQUEST_BYTES: usize = 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(2);
const MAX_CHILDREN: usize = 32;
const MAX_WINDOWS: usize = 128;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotRequest {
    protocol: u64,
    #[serde(rename = "type")]
    message_type: String,
}

#[derive(Debug, Clone, Default)]
pub struct DiagnosticSnapshot {
    pub status: DiagnosticStatus,
    pub updated_at: Option<Instant>,
    pub children: Vec<DiagnosticChild>,
    pub windows: Vec<DiagnosticWindow>,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum DiagnosticStatus {
    #[default]
    Discovering,
    Ready,
    Empty,
    Unavailable,
    Failed,
}

impl DiagnosticStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Discovering => "discovering",
            Self::Ready => "ready",
            Self::Empty => "empty",
            Self::Unavailable => "unavailable",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiagnosticChild {
    pub pid: u32,
    pub loader: String,
    pub role: String,
    pub live: bool,
}

#[derive(Debug, Clone)]
pub struct DiagnosticBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone)]
pub struct DiagnosticWindow {
    pub window_id: u32,
    pub pid: u32,
    pub loader: String,
    pub title: String,
    pub bounds: DiagnosticBounds,
}

#[derive(Serialize)]
struct SnapshotResponse<'a> {
    protocol: u64,
    #[serde(rename = "type")]
    message_type: &'static str,
    status: &'static str,
    snapshot_age_ms: Option<u64>,
    children: Vec<ChildResponse>,
    windows: Vec<WindowResponse<'a>>,
}

#[derive(Serialize)]
struct ChildResponse {
    pid: u32,
    loader: String,
    role: String,
    live: bool,
}

#[derive(Serialize)]
struct WindowResponse<'a> {
    window_id: u32,
    pid: u32,
    loader: &'a str,
    title: &'a str,
    bounds: BoundsResponse,
}

#[derive(Serialize)]
struct BoundsResponse {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

pub struct DiagnosticsServer {
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    socket_path: PathBuf,
    socket_dev: u64,
    socket_ino: u64,
}

impl DiagnosticsServer {
    pub fn start(snapshot: Arc<RwLock<DiagnosticSnapshot>>) -> io::Result<Self> {
        let socket_path = window_manager::unique_socket_path("diagnostics")?;
        Self::start_at(socket_path, snapshot)
    }

    fn start_at(
        socket_path: PathBuf,
        snapshot: Arc<RwLock<DiagnosticSnapshot>>,
    ) -> io::Result<Self> {
        // Never unlink an existing path: it may belong to another live server.
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
                "Diagnostics path is not a socket",
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

        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);

        let worker = match thread::Builder::new()
            .name("diagnostics-uds".to_owned())
            .spawn(move || {
                while !worker_shutdown.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if worker_shutdown.load(Ordering::Acquire) {
                                break;
                            }

                            if let Err(error) = handle_client(stream, &snapshot, &worker_shutdown) {
                                eprintln!("[Diagnostics] Client rejected: {error}");
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(POLL_INTERVAL);
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(error) => {
                            eprintln!("[Diagnostics] Accept failed: {error}");
                            thread::sleep(Duration::from_millis(20));
                        }
                    }
                }

                eprintln!("[Diagnostics] Worker stopped");
            }) {
            Ok(worker) => worker,
            Err(error) => {
                remove_socket_if_owned(&socket_path, socket_dev, socket_ino);
                return Err(error);
            }
        };

        Ok(Self {
            shutdown,
            worker: Some(worker),
            socket_path,
            socket_dev,
            socket_ino,
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for DiagnosticsServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);

        if let Some(worker) = self.worker.take() {
            // The listener is nonblocking, and client reads check shutdown.
            let _ = worker.join();
        }

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

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "diagnostics request deadline exceeded",
            )
        })
}

fn read_request(
    stream: &mut UnixStream,
    deadline: Instant,
    shutdown: &AtomicBool,
) -> io::Result<Vec<u8>> {
    stream.set_nonblocking(true)?;

    let mut bytes = Vec::with_capacity(128);
    let mut byte = [0_u8; 1];

    loop {
        if shutdown.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "server shutting down",
            ));
        }

        remaining(deadline)?;

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
                        "request too large",
                    ));
                }
                bytes.push(byte[0]);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(remaining(deadline)?.min(POLL_INTERVAL));
            }
            Err(error) => return Err(error),
        }
    }
}

fn handle_client(
    mut stream: UnixStream,
    snapshot: &RwLock<DiagnosticSnapshot>,
    shutdown: &AtomicBool,
) -> io::Result<()> {
    let deadline = Instant::now() + REQUEST_TIMEOUT;

    remaining(deadline)?;
    let _peer = window_peer::peer_identity(&stream)?;
    remaining(deadline)?;

    let bytes = read_request(&mut stream, deadline, shutdown)?;
    let request: SnapshotRequest = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;

    if request.protocol != PROTOCOL_VERSION || request.message_type != "snapshot" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported diagnostics request",
        ));
    }

    remaining(deadline)?;

    let guard = snapshot
        .read()
        .map_err(|_| io::Error::other("diagnostics snapshot lock poisoned"))?;

    let now = Instant::now();
    let snapshot_age_ms = guard.updated_at.map(|updated| {
        now.saturating_duration_since(updated)
            .as_millis()
            .min(u64::MAX as u128) as u64
    });

    let children = guard
        .children
        .iter()
        .take(MAX_CHILDREN)
        .map(|child| ChildResponse {
            pid: child.pid,
            loader: child.loader.clone(),
            role: child.role.clone(),
            live: child.live,
        })
        .collect();

    let windows = guard
        .windows
        .iter()
        .filter(|window| {
            window.bounds.x.is_finite()
                && window.bounds.y.is_finite()
                && window.bounds.width.is_finite()
                && window.bounds.height.is_finite()
                && window.bounds.width > 0.0
                && window.bounds.height > 0.0
        })
        .take(MAX_WINDOWS)
        .map(|window| WindowResponse {
            window_id: window.window_id,
            pid: window.pid,
            loader: &window.loader,
            title: &window.title,
            bounds: BoundsResponse {
                x: window.bounds.x,
                y: window.bounds.y,
                width: window.bounds.width,
                height: window.bounds.height,
            },
        })
        .collect();

    let response = SnapshotResponse {
        protocol: PROTOCOL_VERSION,
        message_type: "snapshot",
        status: guard.status.as_str(),
        snapshot_age_ms,
        children,
        windows,
    };

    let encoded = serde_json::to_vec(&response).map_err(io::Error::other)?;

    drop(guard);

    if encoded.len() > MAX_RESPONSE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "response too large",
        ));
    }

    remaining(deadline)?;
    stream.set_nonblocking(true)?;

    let mut output = encoded;
    output.push(b'\n');

    let mut offset = 0;
    while offset < output.len() {
        if shutdown.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "server shutting down",
            ));
        }

        remaining(deadline)?;

        match stream.write(&output[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "socket write returned zero",
                ));
            }
            Ok(count) => offset += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(remaining(deadline)?.min(POLL_INTERVAL));
            }
            Err(error) => return Err(error),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_rejects_unknown_fields() {
        let request = br#"{"protocol":1,"type":"snapshot","extra":true}"#;
        assert!(serde_json::from_slice::<SnapshotRequest>(request).is_err());
    }

    #[test]
    fn request_rejects_wrong_protocol() {
        let request: SnapshotRequest =
            serde_json::from_slice(br#"{"protocol":2,"type":"snapshot"}"#).unwrap();

        assert_ne!(request.protocol, PROTOCOL_VERSION);
    }

    #[test]
    fn status_names_are_stable() {
        assert_eq!(DiagnosticStatus::Discovering.as_str(), "discovering");
        assert_eq!(DiagnosticStatus::Ready.as_str(), "ready");
        assert_eq!(DiagnosticStatus::Empty.as_str(), "empty");
        assert_eq!(DiagnosticStatus::Unavailable.as_str(), "unavailable");
        assert_eq!(DiagnosticStatus::Failed.as_str(), "failed");
    }

    #[test]
    fn request_size_limit_is_bounded() {
        assert_eq!(MAX_REQUEST_BYTES, 1024);
        assert_eq!(MAX_RESPONSE_BYTES, 64 * 1024);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod uds_integration_tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;

    struct TestRuntimeDir(PathBuf);

    impl TestRuntimeDir {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after UNIX epoch")
                .as_nanos();
            let path = PathBuf::from("/tmp").join(format!("jvmd-{}-{nonce}", std::process::id()));
            std::fs::create_dir(&path)
                .expect("isolated diagnostics test directory should be created");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .expect("test directory permissions should be restricted");
            Self(path)
        }

        fn socket_path(&self) -> PathBuf {
            self.0.join("diagnostics.sock")
        }
    }

    impl Drop for TestRuntimeDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn diagnostics_uds_authenticates_serves_snapshot_and_cleans_up() {
        let snapshot = Arc::new(RwLock::new(DiagnosticSnapshot {
            status: DiagnosticStatus::Ready,
            updated_at: Some(Instant::now()),
            children: vec![DiagnosticChild {
                pid: std::process::id(),
                loader: "fabric".to_owned(),
                role: "minecraft_client".to_owned(),
                live: true,
            }],
            windows: vec![DiagnosticWindow {
                window_id: 123,
                pid: std::process::id(),
                loader: "fabric".to_owned(),
                title: "Integration Test Window".to_owned(),
                bounds: DiagnosticBounds {
                    x: 10.0,
                    y: 20.0,
                    width: 1280.0,
                    height: 720.0,
                },
            }],
        }));

        let test_runtime = TestRuntimeDir::new();
        let socket_path = test_runtime.socket_path();

        let server = DiagnosticsServer::start_at(socket_path.clone(), Arc::clone(&snapshot))
            .expect("diagnostics test server should start");

        let metadata =
            std::fs::symlink_metadata(&socket_path).expect("diagnostics socket should exist");

        assert!(metadata.file_type().is_socket());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);

        let stream =
            UnixStream::connect(&socket_path).expect("client should connect to diagnostics UDS");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("read timeout should be configured");

        let mut client = BufReader::new(stream);
        client
            .get_mut()
            .write_all(b"{\"protocol\":1,\"type\":\"snapshot\"}\n")
            .expect("request should be sent");

        let mut response_line = String::new();
        client
            .read_line(&mut response_line)
            .expect("server should return a response");

        assert!(!response_line.is_empty(), "response must not be empty");

        let response: serde_json::Value =
            serde_json::from_str(&response_line).expect("response must be valid JSON");

        assert_eq!(response["protocol"], PROTOCOL_VERSION);
        assert_eq!(response["type"], "snapshot");
        assert_eq!(response["status"], "ready");

        assert_eq!(response["children"].as_array().unwrap().len(), 1);
        assert_eq!(response["children"][0]["loader"], "fabric");
        assert_eq!(response["children"][0]["role"], "minecraft_client");
        assert_eq!(response["children"][0]["live"], true);

        assert_eq!(response["windows"].as_array().unwrap().len(), 1);
        assert_eq!(response["windows"][0]["window_id"], 123);
        assert_eq!(response["windows"][0]["title"], "Integration Test Window");
        assert_eq!(response["windows"][0]["bounds"]["width"], 1280.0);

        let serialized = response.to_string();
        assert!(!serialized.contains("audit_token"));
        assert!(!serialized.contains("pid_version"));

        drop(client);
        drop(server);

        assert!(
            std::fs::symlink_metadata(&socket_path).is_err(),
            "server Drop should remove its owned socket"
        );
    }
}
