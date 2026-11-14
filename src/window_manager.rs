use crate::window_peer;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PROTOCOL_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientRegistration {
    pub pid: u32,
    pub loader: String,
    pub role: String,
    pub protocol: u64,
}

#[derive(Debug, Clone)]
struct RegistryEntry {
    connection_id: u64,
    pid_version: i32,
    registration: ClientRegistration,
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

    pub fn registrations(&self) -> Result<Vec<ClientRegistration>, RegistryError> {
        let registry = self.registry.lock().map_err(|_| RegistryError::Poisoned)?;

        let mut registrations: Vec<_> = registry
            .values()
            .map(|entry| entry.registration.clone())
            .collect();

        registrations.sort_by_key(|entry| entry.pid);
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

fn private_runtime_dir() -> io::Result<PathBuf> {
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
fn validate_registration(value: &Value) -> Result<ClientRegistration, String> {
    if value.get("type").and_then(Value::as_str) != Some("register") {
        return Err("expected type=register".to_owned());
    }

    let pid = value
        .get("pid")
        .and_then(Value::as_u64)
        .filter(|pid| *pid > 0 && *pid <= u32::MAX as u64)
        .ok_or_else(|| "pid must be a positive 32-bit integer".to_owned())? as u32;

    let loader = value
        .get("loader")
        .and_then(Value::as_str)
        .ok_or_else(|| "loader is required".to_owned())?;

    if !matches!(loader, "fabric" | "forge" | "neoforge") {
        return Err("unsupported loader".to_owned());
    }

    let role = value
        .get("role")
        .and_then(Value::as_str)
        .ok_or_else(|| "role is required".to_owned())?;

    if role != "minecraft_client" {
        return Err("unsupported role".to_owned());
    }

    let protocol = value
        .get("protocol")
        .and_then(Value::as_u64)
        .ok_or_else(|| "protocol must be an unsigned integer".to_owned())?;

    if protocol != PROTOCOL_VERSION {
        return Err("unsupported protocol version".to_owned());
    }

    Ok(ClientRegistration {
        pid,
        loader: loader.to_owned(),
        role: role.to_owned(),
        protocol,
    })
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

    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

    let mut reader = BufReader::new(stream);
    let mut line = String::new();

    let read_result = reader.read_line(&mut line);

    match &read_result {
        Ok(0) => eprintln!(
            "[WindowManager][DEBUG] connection_id={connection_id}: EOF before registration"
        ),
        Ok(bytes_read) => eprintln!(
            "[WindowManager][DEBUG] connection_id={connection_id}: read {bytes_read} bytes: {}",
            line.trim_end()
        ),
        Err(error) => eprintln!(
            "[WindowManager][DEBUG] connection_id={connection_id}: registration read failed: {error}"
        ),
    }

    let response = match read_result {
        Ok(0) => return,
        Ok(_) => match serde_json::from_str::<Value>(&line) {
            Ok(value) => match validate_registration(&value) {
                Ok(registration) if registration.pid != identity.pid => {
                    json!({
                        "type": "registered",
                        "accepted": false,
                        "error": "kernel peer PID mismatch"
                    })
                }
                Ok(registration) => {
                    let key = (identity.pid, identity.pid_version);

                    match registry.lock() {
                        Ok(mut entries) => {
                            entries.insert(
                                key,
                                RegistryEntry {
                                    connection_id,
                                    pid_version: identity.pid_version,
                                    registration,
                                },
                            );

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
            },
            Err(_) => json!({
                "type": "registered",
                "accepted": false,
                "error": "invalid JSON"
            }),
        },
        Err(_) => return,
    };

    let registered = response["accepted"].as_bool() == Some(true);
    let key = (identity.pid, identity.pid_version);

    let response_sent = match writeln!(reader.get_mut(), "{response}") {
        Ok(()) => {
            eprintln!(
                "[WindowManager][DEBUG] connection_id={connection_id}: response sent: {response}"
            );
            true
        }
        Err(error) => {
            eprintln!(
                "[WindowManager][DEBUG] connection_id={connection_id}: response write failed: {error}"
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
                            && entry.pid_version == identity.pid_version
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
        start(&default_socket_path().unwrap()).unwrap()
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
        let registration = validate_registration(&valid_registration()).unwrap();

        assert_eq!(registration.pid, std::process::id());
        assert_eq!(registration.loader, "fabric");
        assert_eq!(registration.role, "minecraft_client");
        assert_eq!(registration.protocol, 1);
    }

    #[test]
    fn rejects_unknown_loader() {
        let mut value = valid_registration();
        value["loader"] = json!("unknown");

        assert!(validate_registration(&value).is_err());
    }

    #[test]
    fn rejects_invalid_pid() {
        let mut value = valid_registration();
        value["pid"] = json!(0);

        assert!(validate_registration(&value).is_err());
    }

    #[test]
    fn rejects_wrong_protocol_version() {
        let mut value = valid_registration();
        value["protocol"] = json!(2);

        assert!(validate_registration(&value).is_err());
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

        drop(first);

        thread::sleep(Duration::from_millis(100));

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
        // Its server-side thread will be blocked in read_line().
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
