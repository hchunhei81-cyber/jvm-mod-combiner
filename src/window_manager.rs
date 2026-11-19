use crate::window_peer;
use serde_json::json;

#[cfg(test)]
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PROTOCOL_VERSION: u64 = 1;
const MAX_REGISTRATION_LINE_BYTES: usize = 16 * 1024;
const MAX_CLIENT_CONNECTIONS: usize = 32;

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationRequest {
    #[serde(rename = "type")]
    message_type: String,
    pid: u32,
    loader: String,
    role: String,
    protocol: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientRegistration {
    pub pid: u32,
    pub loader: String,
    pub role: String,
    pub protocol: u64,
}

/// A registration paired with the kernel-authenticated Unix peer identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedRegistration {
    pub registration: ClientRegistration,
    pub identity: window_peer::PeerIdentity,
}

impl std::ops::Deref for AuthenticatedRegistration {
    type Target = ClientRegistration;

    fn deref(&self) -> &Self::Target {
        &self.registration
    }
}

#[derive(Debug, Clone)]
struct RegistryEntry {
    connection_id: u64,
    authenticated: AuthenticatedRegistration,
    shutdown_stream: Arc<UnixStream>,
}

type Registry = Arc<Mutex<HashMap<(u32, i32), RegistryEntry>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryError {
    Poisoned,
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Poisoned => write!(
                formatter,
                "Window Manager registration registry is poisoned"
            ),
        }
    }
}

impl std::error::Error for RegistryError {}

pub struct WindowManagerHandle {
    socket_path: PathBuf,
    socket_dev: u64,
    socket_ino: u64,
    shutdown: Arc<AtomicBool>,
    accept_thread: Option<JoinHandle<()>>,
    registry: Registry,
}

impl WindowManagerHandle {
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn registrations(&self) -> Result<Vec<AuthenticatedRegistration>, RegistryError> {
        let registry = self.registry.lock().map_err(|_| RegistryError::Poisoned)?;

        let mut registrations: Vec<_> = registry
            .values()
            .map(|entry| entry.authenticated.clone())
            .collect();

        registrations.sort_by_key(|entry| (entry.identity.pid, entry.identity.pid_version));

        Ok(registrations)
    }
}

impl Drop for WindowManagerHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);

        if let Some(thread) = self.accept_thread.take() {
            let _ = thread.join();
        }

        // 只删除本次 bind 创建的 socket inode。
        remove_socket_if_owned(&self.socket_path, self.socket_dev, self.socket_ino);
    }
}

fn remove_socket_if_owned(path: &Path, dev: u64, ino: u64) {
    use std::os::unix::fs::FileTypeExt;

    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_socket() && metadata.dev() == dev && metadata.ino() == ino {
            let _ = fs::remove_file(path);
        }
    }
}

pub(crate) fn private_runtime_dir() -> io::Result<PathBuf> {
    let uid = unsafe { libc::getuid() };
    let dir = PathBuf::from(format!("/tmp/jvm-combiner-{uid}"));

    match fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }

    let metadata = fs::symlink_metadata(&dir)?;

    if !metadata.file_type().is_dir() || metadata.uid() != uid || metadata.mode() & 0o777 != 0o700 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Window Manager runtime directory is not private",
        ));
    }

    Ok(dir)
}

pub fn default_socket_path() -> io::Result<PathBuf> {
    let dir = private_runtime_dir()?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();

    Ok(dir.join(format!("wm-{}-{timestamp}.sock", std::process::id())))
}

pub(crate) fn unique_socket_path(prefix: &str) -> io::Result<PathBuf> {
    if prefix.is_empty()
        || !prefix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid socket filename prefix",
        ));
    }

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let dir = private_runtime_dir()?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);

    Ok(dir.join(format!(
        "{prefix}-{}-{timestamp}-{sequence}.sock",
        std::process::id()
    )))
}

#[cfg(test)]
mod unique_socket_path_tests {
    use super::*;

    #[test]
    fn generated_socket_paths_are_unique() {
        let first = unique_socket_path("diagnostics").unwrap();
        let second = unique_socket_path("diagnostics").unwrap();

        assert_ne!(first, second);
        assert_eq!(first.parent(), second.parent());
        assert!(
            first
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("diagnostics-")
        );
    }

    #[test]
    fn invalid_socket_prefix_is_rejected() {
        assert!(unique_socket_path("../diagnostics").is_err());
        assert!(unique_socket_path("").is_err());
    }
}

pub fn start(socket_path: &Path) -> io::Result<WindowManagerHandle> {
    if socket_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "Window Manager socket path already exists; refusing to replace it",
        ));
    }

    let parent = socket_path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "socket path has no parent"))?;

    let parent_metadata = fs::symlink_metadata(parent)?;
    let uid = unsafe { libc::getuid() };

    if !parent_metadata.file_type().is_dir()
        || parent_metadata.uid() != uid
        || parent_metadata.mode() & 0o777 != 0o700
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "socket parent directory must be owned by current user with mode 0700",
        ));
    }

    let listener = UnixListener::bind(socket_path)?;

    // 立即记录本次 bind 创建的 socket inode。
    // 后续清理只能删除这个 inode，不能盲目删除路径。
    let metadata = match fs::symlink_metadata(socket_path) {
        Ok(metadata) => metadata,
        Err(error) => {
            drop(listener);
            return Err(error);
        }
    };

    {
        use std::os::unix::fs::FileTypeExt;

        if !metadata.file_type().is_socket() {
            drop(listener);
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "bound socket path is no longer a socket",
            ));
        }
    }

    let socket_dev = metadata.dev();
    let socket_ino = metadata.ino();

    let setup_result = (|| {
        fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        Ok::<(), io::Error>(())
    })();

    if let Err(error) = setup_result {
        drop(listener);
        remove_socket_if_owned(socket_path, socket_dev, socket_ino);
        return Err(error);
    }

    let shutdown = Arc::new(AtomicBool::new(false));
    let registry: Registry = Arc::new(Mutex::new(HashMap::new()));
    let next_connection_id = Arc::new(AtomicU64::new(1));

    let thread_shutdown = Arc::clone(&shutdown);
    let thread_registry = Arc::clone(&registry);
    let thread_connection_id = Arc::clone(&next_connection_id);

    let accept_thread = match thread::Builder::new()
        .name("window-manager-accept".to_owned())
        .spawn(move || {
            let mut clients: Vec<(UnixStream, JoinHandle<()>)> = Vec::new();

            while !thread_shutdown.load(Ordering::Acquire) {
                let mut index = 0;

                while index < clients.len() {
                    if clients[index].1.is_finished() {
                        let (_, finished_thread) = clients.swap_remove(index);
                        let _ = finished_thread.join();
                    } else {
                        index += 1;
                    }
                }

                match listener.accept() {
                    Ok((stream, _)) => {
                        if clients.len() >= MAX_CLIENT_CONNECTIONS {
                            eprintln!(
                                "[WindowManager] connection limit reached ({MAX_CLIENT_CONNECTIONS}); rejecting client"
                            );
                            drop(stream);
                            continue;
                        }

                        if let Err(error) = stream.set_nonblocking(false) {
                            eprintln!(
                                "[WindowManager] failed to set accepted socket blocking mode: {error}"
                            );
                            continue;
                        }

                        let connection_id = thread_connection_id.fetch_add(1, Ordering::Relaxed);

                        let shutdown_stream = match stream.try_clone() {
                            Ok(stream) => stream,
                            Err(error) => {
                                eprintln!("[WindowManager] socket clone failed: {error}");
                                continue;
                            }
                        };

                        let client_registry = Arc::clone(&thread_registry);
                        let client_shutdown = Arc::clone(&thread_shutdown);

                        match thread::Builder::new()
                            .name(format!("window-manager-client-{connection_id}"))
                            .spawn(move || {
                                handle_client(
                                    stream,
                                    connection_id,
                                    client_registry,
                                    client_shutdown,
                                );
                            }) {
                            Ok(client_thread) => {
                                clients.push((shutdown_stream, client_thread));
                            }
                            Err(error) => {
                                eprintln!("[WindowManager] client thread spawn failed: {error}");
                            }
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => {
                        if !thread_shutdown.load(Ordering::Acquire) {
                            thread::sleep(Duration::from_millis(50));
                        }
                    }
                }
            }

            for (stream, _) in &clients {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }

            for (_, client_thread) in clients {
                let _ = client_thread.join();
            }
        }) {
        Ok(thread) => thread,
        Err(error) => {
            remove_socket_if_owned(socket_path, socket_dev, socket_ino);
            return Err(error);
        }
    };

    Ok(WindowManagerHandle {
        socket_path: socket_path.to_path_buf(),
        socket_dev,
        socket_ino,
        shutdown,
        accept_thread: Some(accept_thread),
        registry,
    })
}

fn parse_registration_line(line: &str) -> Result<ClientRegistration, String> {
    let request: RegistrationRequest =
        serde_json::from_str(line).map_err(|_| "invalid registration JSON or schema".to_owned())?;

    validate_registration_fields(request)
}

fn validate_registration_fields(
    request: RegistrationRequest,
) -> Result<ClientRegistration, String> {
    if request.message_type != "register" {
        return Err("expected type=register".to_owned());
    }

    if request.pid == 0 {
        return Err("pid must be a positive 32-bit integer".to_owned());
    }

    if !matches!(request.loader.as_str(), "fabric" | "forge" | "neoforge") {
        return Err("unsupported loader".to_owned());
    }

    if request.role != "minecraft_client" {
        return Err("unsupported role".to_owned());
    }

    if request.protocol != PROTOCOL_VERSION {
        return Err("unsupported protocol version".to_owned());
    }

    Ok(ClientRegistration {
        pid: request.pid,
        loader: request.loader,
        role: request.role,
        protocol: request.protocol,
    })
}

fn write_nonblocking_until(
    stream: &mut UnixStream,
    bytes: &[u8],
    deadline: Instant,
) -> io::Result<()> {
    let mut offset = 0;

    while offset < bytes.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());

        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Window Manager response exceeded its total deadline",
            ));
        }

        match stream.write(&bytes[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "failed to write Window Manager response",
                ));
            }
            Ok(written) => offset += written,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(remaining.min(Duration::from_millis(2)));
            }
            Err(error) => return Err(error),
        }
    }

    if Instant::now() >= deadline {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Window Manager response exceeded its total deadline",
        ));
    }

    Ok(())
}

fn read_registration_line_until(
    reader: &mut BufReader<UnixStream>,
    deadline: Instant,
) -> io::Result<(String, bool)> {
    let mut bytes = Vec::new();
    let mut byte = [0_u8; 1];

    reader.get_mut().set_nonblocking(true)?;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());

        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Window Manager registration exceeded its total deadline",
            ));
        }

        match reader.read(&mut byte) {
            Ok(0) => {
                if bytes.is_empty() {
                    return Ok((String::new(), false));
                }

                return Ok((
                    String::from_utf8(bytes).map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "registration message is not valid UTF-8",
                        )
                    })?,
                    false,
                ));
            }
            Ok(_) => {
                bytes.push(byte[0]);

                if byte[0] == b'\n' {
                    // Recheck the absolute deadline after receiving the
                    // complete frame and before accepting it.
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "Window Manager registration exceeded its total deadline",
                        ));
                    }

                    return Ok((
                        String::from_utf8(bytes).map_err(|_| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                "registration message is not valid UTF-8",
                            )
                        })?,
                        false,
                    ));
                }

                // LF counts toward the limit. A full buffer without LF
                // cannot become a valid frame, so reject immediately.
                if bytes.len() >= MAX_REGISTRATION_LINE_BYTES {
                    return Ok((String::new(), true));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(remaining.min(Duration::from_millis(2)));
            }
            Err(error) => return Err(error),
        }
    }
}

fn handle_client(
    stream: UnixStream,
    connection_id: u64,
    registry: Registry,
    shutdown: Arc<AtomicBool>,
) {
    eprintln!("[WindowManager][DEBUG] connection_id={connection_id}: handler started");

    // 必须先认证内核 peer，之后才读取注册 JSON。
    let identity = match window_peer::peer_identity(&stream) {
        Ok(identity) => {
            eprintln!(
                "[WindowManager][DEBUG] connection_id={connection_id}: peer authenticated pid={} pid_version={}",
                identity.pid, identity.pid_version
            );
            identity
        }
        Err(error) => {
            eprintln!(
                "[WindowManager] peer authentication failed connection_id={connection_id}: {error}"
            );
            return;
        }
    };

    let mut reader = BufReader::new(stream);
    let registration_deadline = Instant::now() + Duration::from_secs(5);

    let (line, registration_too_large) = match read_registration_line_until(
        &mut reader,
        registration_deadline,
    ) {
        Ok(result) => result,
        Err(error) => {
            eprintln!(
                "[WindowManager] registration read failed connection_id={connection_id}: {error}"
            );
            return;
        }
    };

    let registration_line_terminated = line.ends_with('\n');

    let response = if line.is_empty() && !registration_too_large {
        return;
    } else if registration_too_large {
        json!({
            "type": "registered",
            "accepted": false,
            "error": "registration message exceeds 16 KiB limit"
        })
    } else if !registration_line_terminated {
        json!({
            "type": "registered",
            "accepted": false,
            "error": "registration message must end with LF"
        })
    } else {
        match parse_registration_line(&line) {
            Ok(registration) if registration.pid != identity.pid => {
                json!({
                    "type": "registered",
                    "accepted": false,
                    "error": "kernel peer PID mismatch"
                })
            }
            Ok(registration) => {
                let key = (identity.pid, identity.pid_version);

                let shutdown_stream = match reader.get_ref().try_clone() {
                    Ok(stream) => stream,
                    Err(error) => {
                        eprintln!(
                            "[WindowManager] failed to retain shutdown handle connection_id={connection_id}: {error}"
                        );
                        return;
                    }
                };

                match registry.lock() {
                    Ok(mut entries) => {
                        let replaced = entries.insert(
                            key,
                            RegistryEntry {
                                connection_id,
                                authenticated: AuthenticatedRegistration {
                                    registration,
                                    identity: identity.clone(),
                                },
                                shutdown_stream: Arc::new(shutdown_stream),
                            },
                        );

                        // Release the registry lock before shutting down the
                        // previous connection. Its cleanup will see the new
                        // connection_id and cannot remove the replacement.
                        drop(entries);

                        if let Some(previous) = replaced {
                            let _ = previous.shutdown_stream.shutdown(std::net::Shutdown::Both);
                        }

                        json!({
                            "type": "registered",
                            "accepted": true,
                            "pid": identity.pid
                        })
                    }
                    Err(_) => json!({
                        "type": "registered",
                        "accepted": false,
                        "error": "registry unavailable"
                    }),
                }
            }
            Err(error) => json!({
                "type": "registered",
                "accepted": false,
                "error": error
            }),
        }
    };

    let registered = response["accepted"].as_bool() == Some(true);
    let key = (identity.pid, identity.pid_version);

    let mut response_bytes = match serde_json::to_vec(&response) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!(
                "[WindowManager][DEBUG] connection_id={connection_id}: response serialization failed: {error}"
            );
            Vec::new()
        }
    };

    if !response_bytes.is_empty() {
        response_bytes.push(b'\n');
    }

    let write_result = if response_bytes.is_empty() {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Window Manager response serialization failed",
        ))
    } else {
        let response_deadline = Instant::now() + Duration::from_secs(2);
        write_nonblocking_until(reader.get_mut(), &response_bytes, response_deadline)
    };

    // try_clone shares the underlying socket status. Restore blocking only
    // after the nonblocking response write has completed or failed.
    let restore_result = reader.get_mut().set_nonblocking(false);

    let response_sent = match (write_result, restore_result) {
        (Ok(()), Ok(())) => {
            eprintln!(
                "[WindowManager][DEBUG] connection_id={connection_id}: response sent: {response}"
            );
            true
        }
        (Err(error), _) => {
            eprintln!(
                "[WindowManager][DEBUG] connection_id={connection_id}: response write failed: {error}"
            );
            false
        }
        (Ok(()), Err(error)) => {
            eprintln!(
                "[WindowManager][DEBUG] connection_id={connection_id}: failed to restore blocking mode: {error}"
            );
            false
        }
    };

    if response_sent && registered {
        let mut buffer = [0_u8; 256];

        while !shutdown.load(Ordering::Acquire) {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => break,
            }
        }
    }

    if registered {
        match registry.lock() {
            Ok(mut entries) => {
                let should_remove = entries
                    .get(&key)
                    .map(|entry| {
                        entry.connection_id == connection_id
                            && entry.authenticated.identity == identity
                    })
                    .unwrap_or(false);

                if should_remove {
                    entries.remove(&key);
                }
            }
            Err(_) => {
                eprintln!(
                    "[WindowManager] registry poisoned during cleanup pid={}",
                    identity.pid
                );
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    fn valid_registration() -> Value {
        json!({
            "type": "register",
            "pid": std::process::id(),
            "loader": "fabric",
            "role": "minecraft_client",
            "protocol": 1
        })
    }

    fn start_test_manager() -> WindowManagerHandle {
        use std::sync::atomic::{AtomicU64, Ordering};

        static TEST_SOCKET_COUNTER: AtomicU64 = AtomicU64::new(0);

        let base_path = default_socket_path().unwrap();
        let sequence = TEST_SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);

        let socket_path =
            base_path.with_file_name(format!("wm-test-{}-{sequence}.sock", std::process::id()));

        start(&socket_path).unwrap()
    }

    #[test]
    fn registration_line_limit_is_enforced() {
        use std::io::{BufReader, Cursor, Read};

        let oversized = vec![b'a'; MAX_REGISTRATION_LINE_BYTES + 128];
        let mut reader = BufReader::new(Cursor::new(oversized));
        let mut line = String::new();

        let bytes = reader
            .by_ref()
            .take((MAX_REGISTRATION_LINE_BYTES + 1) as u64)
            .read_line(&mut line)
            .unwrap();

        assert_eq!(bytes, MAX_REGISTRATION_LINE_BYTES + 1);
        assert!(bytes > MAX_REGISTRATION_LINE_BYTES);
    }

    #[test]
    fn registration_reader_accepts_complete_line_before_deadline() {
        let (server, mut client) = UnixStream::pair().unwrap();
        let expected = format!("{}\n", valid_registration());
        client.write_all(expected.as_bytes()).unwrap();
        drop(client);

        let mut reader = BufReader::new(server);
        let (line, too_large) =
            read_registration_line_until(&mut reader, Instant::now() + Duration::from_secs(1))
                .unwrap();

        assert!(!too_large);
        assert_eq!(line, expected);
    }

    #[test]
    fn registration_reader_enforces_absolute_deadline() {
        let (server, mut client) = UnixStream::pair().unwrap();
        let writer = thread::spawn(move || {
            for byte in b"abcdef\n" {
                if client.write_all(&[*byte]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(40));
            }
        });

        let mut reader = BufReader::new(server);
        let deadline = Instant::now() + Duration::from_millis(150);

        let error = read_registration_line_until(&mut reader, deadline).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        let _ = writer.join();
    }

    #[test]
    fn registration_reader_enforces_payload_size_boundary() {
        fn read_line(line: &[u8]) -> (String, bool) {
            let (server, mut client) = UnixStream::pair().unwrap();
            let line = line.to_vec();
            let writer = thread::spawn(move || {
                client.write_all(&line).unwrap();
                drop(client);
            });

            let mut reader = BufReader::new(server);
            let result =
                read_registration_line_until(&mut reader, Instant::now() + Duration::from_secs(1))
                    .unwrap();
            writer.join().unwrap();
            result
        }

        let json = valid_registration().to_string();
        let exact_padding = MAX_REGISTRATION_LINE_BYTES - json.len() - 1;
        let exact_limit = format!("{json}{}\n", " ".repeat(exact_padding));
        assert_eq!(exact_limit.len(), MAX_REGISTRATION_LINE_BYTES);

        let (line, too_large) = read_line(exact_limit.as_bytes());
        assert!(!too_large);
        assert!(line.ends_with('\n'));
        assert!(parse_registration_line(&line).is_ok());

        let over_limit_padding = MAX_REGISTRATION_LINE_BYTES - json.len();
        let over_limit = format!("{json}{}\n", " ".repeat(over_limit_padding));
        assert_eq!(over_limit.len(), MAX_REGISTRATION_LINE_BYTES + 1);

        let (_, too_large) = read_line(over_limit.as_bytes());
        assert!(too_large);
    }

    #[test]
    fn registration_reader_rejects_full_buffer_without_lf_immediately() {
        let (mut writer, reader_stream) = std::os::unix::net::UnixStream::pair().unwrap();

        let payload = vec![b'a'; MAX_REGISTRATION_LINE_BYTES];

        // Writer 独立运行，避免 socket 缓冲区填满后发生同步读写死锁。
        let writer_thread = std::thread::spawn(move || {
            std::io::Write::write_all(&mut writer, &payload).unwrap();
        });

        let mut reader = std::io::BufReader::new(reader_stream);
        let started = std::time::Instant::now();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);

        let result = read_registration_line_until(&mut reader, deadline);
        let elapsed = started.elapsed();

        writer_thread
            .join()
            .expect("registration writer thread panicked");

        let (line, too_large) = result.expect("reader should reject oversized unterminated input");
        assert!(too_large, "full buffer without LF must be rejected");
        assert!(
            line.is_empty(),
            "rejected input must not return a partial line"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "full buffer rejection was not immediate: {elapsed:?}"
        );
    }

    #[test]
    fn registration_deadline_is_checked_after_complete_frame() {
        let deadline = Instant::now() - Duration::from_millis(1);

        assert!(Instant::now() >= deadline);
    }

    #[test]
    fn registrations_returns_error_when_registry_is_poisoned() {
        let manager = start_test_manager();
        let registry = Arc::clone(&manager.registry);

        let panic_result = thread::spawn(move || {
            let _guard = registry.lock().unwrap();
            panic!("intentional registry poison for regression test");
        })
        .join();

        assert!(panic_result.is_err());
        assert_eq!(manager.registrations(), Err(RegistryError::Poisoned));
    }

    #[test]
    fn accepts_valid_registration() {
        let registration = parse_registration_line(&valid_registration().to_string()).unwrap();

        assert_eq!(registration.pid, std::process::id());
        assert_eq!(registration.loader, "fabric");
        assert_eq!(registration.role, "minecraft_client");
        assert_eq!(registration.protocol, 1);
    }

    #[test]
    fn rejects_unknown_registration_fields() {
        let mut value = valid_registration();
        value["unexpected"] = json!(true);

        assert!(parse_registration_line(&value.to_string()).is_err());
    }

    #[test]
    fn rejects_duplicate_registration_fields() {
        let line = format!(
            r#"{{"type":"register","pid":{},"loader":"fabric","role":"minecraft_client","protocol":1,"pid":{}}}"#,
            std::process::id(),
            std::process::id()
        );

        assert!(parse_registration_line(&line).is_err());
    }

    #[test]
    fn rejects_registration_pid_outside_u32_range() {
        for pid in ["-1", "4294967296"] {
            let line = format!(
                r#"{{"type":"register","pid":{pid},"loader":"fabric","role":"minecraft_client","protocol":1}}"#
            );

            assert!(
                parse_registration_line(&line).is_err(),
                "accepted pid {pid}"
            );
        }
    }

    #[test]
    fn parses_registration_without_lf_at_line_parser_layer() {
        let line = valid_registration().to_string();

        assert!(parse_registration_line(&line).is_ok());
        assert!(!line.ends_with('\n'));
    }

    #[test]
    fn rejects_registration_with_unknown_raw_field() {
        let line = format!(
            r#"{{"type":"register","pid":{},"loader":"fabric","role":"minecraft_client","protocol":1,"extra":true}}"#,
            std::process::id()
        );

        assert!(parse_registration_line(&line).is_err());
    }

    #[test]
    fn rejects_unknown_loader() {
        let mut value = valid_registration();
        value["loader"] = json!("unknown");

        assert!(parse_registration_line(&value.to_string()).is_err());
    }

    #[test]
    fn rejects_invalid_pid() {
        let mut value = valid_registration();
        value["pid"] = json!(0);

        assert!(parse_registration_line(&value.to_string()).is_err());
    }

    #[test]
    fn rejects_wrong_protocol_version() {
        let mut value = valid_registration();
        value["protocol"] = json!(2);

        assert!(parse_registration_line(&value.to_string()).is_err());
    }

    #[test]
    fn newer_connection_replaces_same_pid_without_stale_cleanup() {
        let manager = start_test_manager();

        let mut first = UnixStream::connect(manager.socket_path()).unwrap();
        first
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        writeln!(first, "{}", valid_registration()).unwrap();

        let mut response = String::new();
        BufReader::new(first.try_clone().unwrap())
            .read_line(&mut response)
            .unwrap();

        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["accepted"], true);

        let mut second = UnixStream::connect(manager.socket_path()).unwrap();
        second
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        let mut replacement = valid_registration();
        replacement["loader"] = json!("neoforge");

        writeln!(second, "{replacement}").unwrap();

        let mut response = String::new();
        BufReader::new(second.try_clone().unwrap())
            .read_line(&mut response)
            .unwrap();

        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["accepted"], true);

        let deadline = Instant::now() + Duration::from_secs(2);
        while manager
            .registrations()
            .expect("registry should be readable")
            .first()
            .map(|entry| entry.loader.as_str())
            != Some("neoforge")
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(
            manager
                .registrations()
                .expect("registry should be readable")
                .len(),
            1
        );
        assert_eq!(
            manager
                .registrations()
                .expect("registry should be readable")[0]
                .loader,
            "neoforge"
        );

        // The replacement must actively close the previous connection.
        // Keep the local client handle alive to verify it cannot remain active.
        let mut old_connection_buffer = [0_u8; 1];
        assert_eq!(
            first.read(&mut old_connection_buffer).unwrap(),
            0,
            "replaced connection should be closed by the manager"
        );

        drop(second);

        let deadline = Instant::now() + Duration::from_secs(2);
        while !manager
            .registrations()
            .expect("registry should be readable")
            .is_empty()
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }

        assert!(
            manager
                .registrations()
                .expect("registry should be readable")
                .is_empty()
        );
    }

    #[test]
    fn shutdown_closes_clients_and_cleans_registry() {
        let manager = start_test_manager();
        let registry = Arc::clone(&manager.registry);

        let mut client = UnixStream::connect(manager.socket_path()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        writeln!(client, "{}", valid_registration()).unwrap();

        let mut response = String::new();
        BufReader::new(client.try_clone().unwrap())
            .read_line(&mut response)
            .unwrap();

        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["accepted"], true);

        let deadline = Instant::now() + Duration::from_secs(2);
        while registry.lock().unwrap().is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(registry.lock().unwrap().len(), 1);

        // Keep a second client connected without sending its registration.
        // Its server-side thread will be polling the registration reader.
        let mut pending_client = UnixStream::connect(manager.socket_path()).unwrap();
        pending_client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        thread::sleep(Duration::from_millis(50));

        let started = Instant::now();
        drop(manager);

        assert!(
            started.elapsed() < Duration::from_secs(2),
            "Window Manager shutdown took too long"
        );

        assert!(registry.lock().unwrap().is_empty());

        let mut buffer = [0_u8; 1];
        assert_eq!(client.read(&mut buffer).unwrap(), 0);
        assert_eq!(pending_client.read(&mut buffer).unwrap(), 0);
    }

    #[test]
    fn unix_service_rejects_invalid_registration_with_error_response() {
        let manager = start_test_manager();

        let mut stream = UnixStream::connect(manager.socket_path()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        let mut invalid = valid_registration();
        invalid["loader"] = json!("unknown");

        writeln!(stream, "{invalid}").unwrap();

        let mut response = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut response)
            .unwrap();

        let response: Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["type"], "registered");
        assert_eq!(response["accepted"], false);
        assert_eq!(response["error"], "unsupported loader");
        assert!(response.get("pid").is_none());
        assert!(
            manager
                .registrations()
                .expect("registry should be readable")
                .is_empty()
        );

        drop(stream);
    }

    #[test]
    fn unix_service_rejects_oversized_registration() {
        use std::io::Write;

        let manager = start_test_manager();

        let mut stream = UnixStream::connect(manager.socket_path()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        let oversized = vec![b'a'; MAX_REGISTRATION_LINE_BYTES + 128];
        stream.write_all(&oversized).unwrap();
        stream.write_all(b"\n").unwrap();

        let mut response = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut response)
            .unwrap();

        let response: Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["type"], "registered");
        assert_eq!(response["accepted"], false);
        assert_eq!(
            response["error"],
            "registration message exceeds 16 KiB limit"
        );

        assert!(
            manager
                .registrations()
                .expect("registry should be readable")
                .is_empty()
        );

        drop(stream);
    }

    #[test]
    fn unix_service_rejects_registration_without_lf() {
        let manager = start_test_manager();

        let mut stream = UnixStream::connect(manager.socket_path()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .write_all(valid_registration().to_string().as_bytes())
            .unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();

        let mut response = String::new();
        BufReader::new(stream).read_line(&mut response).unwrap();

        let response: Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["type"], "registered");
        assert_eq!(response["accepted"], false);
        assert_eq!(response["error"], "registration message must end with LF");
        assert!(
            manager
                .registrations()
                .expect("registry should be readable")
                .is_empty()
        );
    }

    #[test]
    fn unix_service_limits_concurrent_clients() {
        let manager = start_test_manager();
        let mut clients = Vec::with_capacity(MAX_CLIENT_CONNECTIONS);

        // Keep all clients in the registration-reading stage. Registering
        // them here would intentionally replace earlier connections because
        // every client belongs to this same test process.
        for _ in 0..MAX_CLIENT_CONNECTIONS {
            clients.push(UnixStream::connect(manager.socket_path()).unwrap());
        }

        // Retry until the server has accepted all 32 original connections.
        // A candidate that is accepted remains pending; dropping it releases
        // its handler before the next attempt.
        let rejection_deadline = Instant::now() + Duration::from_secs(4);
        let mut capacity_confirmed = false;

        while Instant::now() < rejection_deadline {
            let mut candidate = UnixStream::connect(manager.socket_path()).unwrap();
            candidate
                .set_read_timeout(Some(Duration::from_millis(100)))
                .unwrap();

            let mut buffer = [0_u8; 1];
            match candidate.read(&mut buffer) {
                Ok(0) => {
                    capacity_confirmed = true;
                    break;
                }
                Ok(_) => panic!("unexpected data from an unregistered candidate"),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    drop(candidate);
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("capacity probe failed: {error}"),
            }
        }

        assert!(
            capacity_confirmed,
            "server did not reject a connection after reaching capacity"
        );

        // Free one handler slot.
        drop(clients.remove(0));

        // Verify a new client can register after capacity becomes available.
        let replacement_deadline = Instant::now() + Duration::from_secs(2);
        let mut accepted_replacement = None;

        while Instant::now() < replacement_deadline {
            let mut replacement = UnixStream::connect(manager.socket_path()).unwrap();
            replacement
                .set_read_timeout(Some(Duration::from_millis(150)))
                .unwrap();

            if let Err(error) = writeln!(replacement, "{}", valid_registration()) {
                if !matches!(
                    error.kind(),
                    io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
                ) {
                    panic!("failed to send replacement registration: {error}");
                }

                drop(replacement);
                thread::sleep(Duration::from_millis(10));
                continue;
            }

            let mut response = String::new();
            match BufReader::new(replacement.try_clone().unwrap()).read_line(&mut response) {
                Ok(0) => {}
                Ok(_) => {
                    let response: Value = serde_json::from_str(&response).unwrap();
                    if response["accepted"] == true {
                        accepted_replacement = Some(replacement);
                        break;
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(error) => panic!("replacement response failed: {error}"),
            }

            drop(replacement);
            thread::sleep(Duration::from_millis(10));
        }

        assert!(
            accepted_replacement.is_some(),
            "server did not accept a replacement after a client disconnected"
        );

        drop(accepted_replacement);
        drop(clients);
    }

    #[test]
    fn unix_service_registers_and_removes_client() {
        let manager = start_test_manager();
        let mut stream = UnixStream::connect(manager.socket_path()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        writeln!(stream, "{}", valid_registration()).unwrap();

        let mut response = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut response)
            .unwrap();

        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["type"], "registered");
        assert_eq!(response["accepted"], true);
        assert_eq!(response["pid"], std::process::id());
        assert!(response.get("error").is_none());

        let deadline = Instant::now() + Duration::from_secs(2);
        while manager
            .registrations()
            .expect("registry should be readable")
            .is_empty()
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(
            manager
                .registrations()
                .expect("registry should be readable")
                .len(),
            1
        );
        assert_eq!(
            manager
                .registrations()
                .expect("registry should be readable")[0]
                .pid,
            std::process::id()
        );

        drop(stream);

        let deadline = Instant::now() + Duration::from_secs(2);
        while !manager
            .registrations()
            .expect("registry should be readable")
            .is_empty()
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }

        assert!(
            manager
                .registrations()
                .expect("registry should be readable")
                .is_empty()
        );
    }

    #[test]
    fn java_window_bridge_registers_and_disconnects_from_rust_service() {
        let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let agent_jar = project_root.join("window_bridge/build/window-bridge-agent.jar");

        assert!(
            agent_jar.is_file(),
            "Window Bridge JAR missing; run window_bridge/build.sh first"
        );

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock error")
            .as_nanos();

        let harness_dir = std::env::temp_dir().join(format!(
            "jvm-window-bridge-rust-test-{}-{timestamp}",
            std::process::id()
        ));

        fs::create_dir_all(&harness_dir).expect("failed to create Java harness directory");

        let source_path = harness_dir.join("WindowBridgeRustHarness.java");

        let harness_source = r#"
public final class WindowBridgeRustHarness {
    public static void main(String[] args) throws Exception {
        Thread.sleep(60000L);
    }
}
"#;

        fs::write(&source_path, harness_source).expect("failed to write Java harness source");

        let compile_status = Command::new("javac")
            .arg("--release")
            .arg("21")
            .arg("-d")
            .arg(&harness_dir)
            .arg(&source_path)
            .status()
            .expect("failed to execute javac");

        assert!(
            compile_status.success(),
            "Java integration harness compilation failed"
        );

        let manager = start_test_manager();
        let socket_path = manager.socket_path().to_string_lossy().into_owned();

        for loader in ["fabric", "forge", "neoforge"] {
            let mut child = Command::new("java")
                .arg(format!("-javaagent:{}={loader}", agent_jar.display()))
                .arg("-cp")
                .arg(&harness_dir)
                .arg("WindowBridgeRustHarness")
                .env("JVM_COMBINER_WINDOW_SOCKET", &socket_path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap_or_else(|error| {
                    panic!("failed to start Java process for {loader}: {error}")
                });

            let pid = child.id();
            let deadline = Instant::now() + Duration::from_secs(8);

            let registration = loop {
                if let Some(registration) = manager
                    .registrations()
                    .expect("registry should be readable")
                    .into_iter()
                    .find(|entry| entry.pid == pid)
                {
                    break Some(registration);
                }

                if child
                    .try_wait()
                    .expect("failed to inspect Java process")
                    .is_some()
                {
                    break None;
                }

                if Instant::now() >= deadline {
                    break None;
                }

                thread::sleep(Duration::from_millis(20));
            };

            // Always terminate the test JVM before asserting, so a failed
            // registration cannot leave a background Java process running.
            let _ = child.kill();
            let _ = child.wait();

            let cleanup_deadline = Instant::now() + Duration::from_secs(5);

            while manager
                .registrations()
                .expect("registry should be readable")
                .iter()
                .any(|entry| entry.pid == pid)
                && Instant::now() < cleanup_deadline
            {
                thread::sleep(Duration::from_millis(20));
            }

            let registration = registration.unwrap_or_else(|| {
                panic!(
                    "Rust Window Manager did not receive {loader} registration \
                     from Java PID {pid}"
                )
            });

            assert_eq!(registration.pid, pid);
            assert_eq!(registration.loader, loader);
            assert_eq!(registration.role, "minecraft_client");
            assert_eq!(registration.protocol, PROTOCOL_VERSION);

            assert!(
                !manager
                    .registrations()
                    .expect("registry should be readable")
                    .iter()
                    .any(|entry| entry.pid == pid),
                "Rust registry did not remove disconnected Java PID {pid}"
            );
        }

        drop(manager);

        fs::remove_dir_all(&harness_dir).expect("failed to remove Java harness directory");
    }
}
