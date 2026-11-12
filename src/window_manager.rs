use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const DEFAULT_BIND_ADDR: &str = "127.0.0.1:25566";
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
    registration: ClientRegistration,
}

type Registry = Arc<Mutex<HashMap<u32, RegistryEntry>>>;

pub struct WindowManagerHandle {
    addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    accept_thread: Option<JoinHandle<()>>,
    registry: Registry,
}

impl WindowManagerHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn registrations(&self) -> Vec<ClientRegistration> {
        let Ok(registry) = self.registry.lock() else {
            return Vec::new();
        };

        let mut registrations: Vec<_> = registry
            .values()
            .map(|entry| entry.registration.clone())
            .collect();

        registrations.sort_by_key(|entry| entry.pid);
        registrations
    }
}

impl Drop for WindowManagerHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);

        if let Some(thread) = self.accept_thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn start(bind_addr: SocketAddr) -> io::Result<WindowManagerHandle> {
    let listener = TcpListener::bind(bind_addr)?;
    listener.set_nonblocking(true)?;

    let addr = listener.local_addr()?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let registry: Registry = Arc::new(Mutex::new(HashMap::new()));
    let next_connection_id = Arc::new(AtomicU64::new(1));

    let thread_shutdown = Arc::clone(&shutdown);
    let thread_registry = Arc::clone(&registry);
    let thread_connection_id = Arc::clone(&next_connection_id);

    let accept_thread = thread::Builder::new()
        .name("window-manager-accept".to_owned())
        .spawn(move || {
            let mut clients: Vec<(TcpStream, JoinHandle<()>)> = Vec::new();

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
                    Ok((stream, _peer)) => {
                        let connection_id =
                            thread_connection_id.fetch_add(1, Ordering::Relaxed);

                        let shutdown_stream = match stream.try_clone() {
                            Ok(stream) => stream,
                            Err(error) => {
                                eprintln!(
                                    "[WindowManager] failed to clone client socket connection_id={connection_id}: {error}"
                                );
                                continue;
                            }
                        };

                        let client_registry = Arc::clone(&thread_registry);
                        let client_shutdown = Arc::clone(&thread_shutdown);

                        let spawn_result = thread::Builder::new()
                            .name(format!("window-manager-client-{connection_id}"))
                            .spawn(move || {
                                handle_client(
                                    stream,
                                    connection_id,
                                    client_registry,
                                    client_shutdown,
                                );
                            });

                        match spawn_result {
                            Ok(client_thread) => {
                                clients.push((shutdown_stream, client_thread));
                            }
                            Err(error) => {
                                eprintln!(
                                    "[WindowManager] client thread spawn failed connection_id={connection_id}: {error}"
                                );
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
                let _ = stream.shutdown(Shutdown::Both);
            }

            for (_, client_thread) in clients {
                let _ = client_thread.join();
            }
        })?;

    Ok(WindowManagerHandle {
        addr,
        shutdown,
        accept_thread: Some(accept_thread),
        registry,
    })
}

pub fn default_bind_addr() -> SocketAddr {
    DEFAULT_BIND_ADDR
        .parse()
        .expect("DEFAULT_BIND_ADDR must be a valid socket address")
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
    stream: TcpStream,
    connection_id: u64,
    registry: Registry,
    shutdown: Arc<AtomicBool>,
) {
    if let Err(error) = stream.set_nonblocking(false) {
        eprintln!(
            "[WindowManager] failed to set blocking mode connection_id={connection_id}: {error}"
        );
        return;
    }

    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

    let mut reader = BufReader::new(stream);

    let mut line = String::new();
    let response = match reader.read_line(&mut line) {
        Ok(0) => return,
        Ok(_) => match serde_json::from_str::<Value>(&line) {
            Ok(value) => match validate_registration(&value) {
                Ok(registration) => {
                    let pid = registration.pid;

                    let inserted = registry.lock().map(|mut entries| {
                        entries.insert(
                            pid,
                            RegistryEntry {
                                connection_id,
                                registration,
                            },
                        );
                    });

                    match inserted {
                        Ok(()) => json!({
                            "type": "registered",
                            "accepted": true,
                            "pid": pid
                        }),
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

    let registered_pid = response
        .get("pid")
        .and_then(Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok());

    let response_sent = {
        let stream = reader.get_mut();
        writeln!(stream, "{response}").is_ok()
    };

    if response_sent {
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

    if let (Some(pid), Ok(mut entries)) = (registered_pid, registry.lock()) {
        if let Some(entry) = entries.get(&pid) {
            if entry.connection_id == connection_id {
                entries.remove(&pid);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    fn valid_registration() -> Value {
        json!({
            "type": "register",
            "pid": 12345,
            "loader": "fabric",
            "role": "minecraft_client",
            "protocol": 1
        })
    }

    #[test]
    fn accepts_valid_registration() {
        let registration = validate_registration(&valid_registration()).unwrap();

        assert_eq!(registration.pid, 12345);
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
        let manager = start("127.0.0.1:0".parse().unwrap()).unwrap();

        let mut first = TcpStream::connect(manager.local_addr()).unwrap();
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

        let mut second = TcpStream::connect(manager.local_addr()).unwrap();
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
            .first()
            .map(|entry| entry.loader.as_str())
            != Some("neoforge")
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(manager.registrations().len(), 1);
        assert_eq!(manager.registrations()[0].loader, "neoforge");

        drop(first);

        thread::sleep(Duration::from_millis(100));

        assert_eq!(manager.registrations().len(), 1);
        assert_eq!(manager.registrations()[0].loader, "neoforge");

        drop(second);

        let deadline = Instant::now() + Duration::from_secs(2);
        while !manager.registrations().is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }

        assert!(manager.registrations().is_empty());
    }

    #[test]
    fn shutdown_closes_clients_and_cleans_registry() {
        let manager = start("127.0.0.1:0".parse().unwrap()).unwrap();
        let registry = Arc::clone(&manager.registry);

        let mut client = TcpStream::connect(manager.local_addr()).unwrap();
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
        let mut pending_client = TcpStream::connect(manager.local_addr()).unwrap();
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
    fn tcp_service_rejects_invalid_registration_with_error_response() {
        let manager = start("127.0.0.1:0".parse().unwrap()).unwrap();

        let mut stream = TcpStream::connect(manager.local_addr()).unwrap();
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
        assert!(manager.registrations().is_empty());

        drop(stream);
    }

    #[test]
    fn tcp_service_registers_and_removes_client() {
        let manager = start("127.0.0.1:0".parse().unwrap()).unwrap();
        let mut stream = TcpStream::connect(manager.local_addr()).unwrap();
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
        assert_eq!(response["pid"], 12345);
        assert!(response.get("error").is_none());

        let deadline = Instant::now() + Duration::from_secs(2);
        while manager.registrations().is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(manager.registrations().len(), 1);
        assert_eq!(manager.registrations()[0].pid, 12345);

        drop(stream);

        let deadline = Instant::now() + Duration::from_secs(2);
        while !manager.registrations().is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }

        assert!(manager.registrations().is_empty());
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

        let manager =
            start("127.0.0.1:0".parse().unwrap()).expect("failed to start Rust Window Manager");

        let address = manager.local_addr().to_string();

        for loader in ["fabric", "forge", "neoforge"] {
            let mut child = Command::new("java")
                .arg(format!("-javaagent:{}={loader}", agent_jar.display()))
                .arg("-cp")
                .arg(&harness_dir)
                .arg("WindowBridgeRustHarness")
                .env("JVM_COMBINER_WINDOW_ADDR", &address)
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

            while manager.registrations().iter().any(|entry| entry.pid == pid)
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
                !manager.registrations().iter().any(|entry| entry.pid == pid),
                "Rust registry did not remove disconnected Java PID {pid}"
            );
        }

        drop(manager);

        fs::remove_dir_all(&harness_dir).expect("failed to remove Java harness directory");
    }
}
