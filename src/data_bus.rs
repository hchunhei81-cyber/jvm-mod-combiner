use crate::window_manager;
use crate::window_peer::{self, PeerIdentity};

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread;
use std::time::Duration;

const MAX_CLIENTS: usize = 8;
const MAX_MESSAGE: usize = 64 * 1024;
const OUT_QUEUE: usize = 128;

type Sender = mpsc::SyncSender<String>;

#[derive(Clone)]
struct Registration {
    sender: Sender,
    generation: u64,
}

#[derive(Clone)]
struct Authorization {
    pid: u32,
    start_time: window_peer::ProcessStartTime,
    identity: Option<PeerIdentity>,
    generation: u64,
}

struct State {
    authorizations: HashMap<String, Authorization>,
    registrations: HashMap<String, Registration>,
}

struct Server {
    path: PathBuf,
    inode: u64,
    state: Arc<Mutex<State>>,
    stopping: Arc<AtomicBool>,
    clients: Arc<AtomicUsize>,
    accept_thread: Option<thread::JoinHandle<()>>,
}

static SERVER: OnceLock<Server> = OnceLock::new();
static START_LOCK: Mutex<()> = Mutex::new(());
static NEXT_AUTHORIZATION_GENERATION: AtomicU64 = AtomicU64::new(1);

pub fn socket_path() -> io::Result<PathBuf> {
    SERVER
        .get()
        .map(|s| s.path.clone())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "DataBus is not running"))
}

pub fn start() -> io::Result<()> {
    let _start_guard = START_LOCK
        .lock()
        .map_err(|_| io::Error::other("DataBus start lock poisoned"))?;

    if SERVER.get().is_some() {
        return Ok(());
    }

    #[cfg(not(target_os = "macos"))]
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "authenticated DataBus currently requires macOS peer credentials",
        ));
    }

    #[cfg(target_os = "macos")]
    {
        let dir = window_manager::private_runtime_dir()?;
        let path = window_manager::unique_socket_path("bus")?;

        if path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "refusing to replace existing DataBus socket path",
            ));
        }

        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;

        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_socket()
            || metadata.uid() != unsafe { libc::getuid() }
            || metadata.mode() & 0o777 != 0o600
        {
            let _ = fs::remove_file(&path);
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "DataBus socket ownership or permissions invalid",
            ));
        }

        listener.set_nonblocking(true)?;

        let state = Arc::new(Mutex::new(State {
            authorizations: HashMap::new(),
            registrations: HashMap::new(),
        }));
        let stopping = Arc::new(AtomicBool::new(false));
        let clients = Arc::new(AtomicUsize::new(0));

        let thread_state = state.clone();
        let thread_stopping = stopping.clone();
        let thread_clients = clients.clone();

        let accept_thread =
            match thread::Builder::new()
                .name("jvm-data-bus".into())
                .spawn(move || {
                    while !thread_stopping.load(Ordering::Acquire) {
                        match listener.accept() {
                            Ok((stream, _)) => {
                                if thread_clients.fetch_add(1, Ordering::AcqRel) >= MAX_CLIENTS {
                                    thread_clients.fetch_sub(1, Ordering::AcqRel);
                                    drop(stream);
                                    continue;
                                }

                                let state = thread_state.clone();
                                let clients = thread_clients.clone();

                                let spawned = thread::Builder::new()
                                    .name("jvm-data-bus-peer".into())
                                    .spawn(move || {
                                        let _guard = ClientGuard(clients);
                                        handle_client(stream, state);
                                    });

                                if spawned.is_err() {
                                    // Closure is dropped when spawn fails, but ClientGuard
                                    // was not constructed; explicitly roll back the slot.
                                    thread_clients.fetch_sub(1, Ordering::AcqRel);
                                }
                            }
                            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                                thread::sleep(Duration::from_millis(20));
                            }
                            Err(_) => thread::sleep(Duration::from_millis(50)),
                        }
                    }
                }) {
                Ok(handle) => handle,
                Err(error) => {
                    if let Ok(current) = fs::symlink_metadata(&path) {
                        if current.file_type().is_socket() && current.ino() == metadata.ino() {
                            let _ = fs::remove_file(&path);
                        }
                    }
                    return Err(error);
                }
            };

        let server = Server {
            path,
            inode: metadata.ino(),
            state,
            stopping,
            clients,
            accept_thread: Some(accept_thread),
        };

        SERVER.set(server).map_err(|_| {
            io::Error::new(io::ErrorKind::AlreadyExists, "DataBus already initialized")
        })?;

        let _ = dir;
        eprintln!("[DataBus] authenticated UDS server ready");
        Ok(())
    }
}

struct ClientGuard(Arc<AtomicUsize>);
impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub fn authorize(role: &str, pid: u32) -> io::Result<()> {
    if !matches!(role, "fabric" | "neoforge" | "forge") || pid == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid DataBus role or PID",
        ));
    }

    // 父进程授权时捕获进程启动时间，不能等首次连接后才绑定 PID。
    let start_time = window_peer::process_start_time(pid)?;

    let server = SERVER
        .get()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "DataBus is not running"))?;

    let mut state = server
        .state
        .lock()
        .map_err(|_| io::Error::other("DataBus authorization lock poisoned"))?;

    if state.authorizations.contains_key(role)
        || state.authorizations.values().any(|entry| entry.pid == pid)
    {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "DataBus role or PID already authorized",
        ));
    }

    let generation = NEXT_AUTHORIZATION_GENERATION.fetch_add(1, Ordering::Relaxed);
    if generation == 0 {
        return Err(io::Error::other(
            "DataBus authorization generation exhausted",
        ));
    }

    state.authorizations.insert(
        role.to_string(),
        Authorization {
            pid,
            start_time,
            identity: None,
            generation,
        },
    );

    eprintln!(
        "[DataBus] authorized role={role} pid={pid} start_time={}.{:06}",
        start_time.seconds, start_time.microseconds
    );
    Ok(())
}

fn revoke_from_state(state: &mut State, role: &str, pid: u32) -> io::Result<()> {
    let Some(auth) = state.authorizations.get(role) else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "DataBus authorization not found",
        ));
    };

    if auth.pid != pid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "DataBus authorization PID mismatch",
        ));
    }

    state.authorizations.remove(role);
    state.registrations.remove(role);
    Ok(())
}

pub fn revoke(role: &str, pid: u32) -> io::Result<()> {
    if !matches!(role, "fabric" | "neoforge" | "forge") || pid == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid DataBus role or PID",
        ));
    }

    let server = SERVER
        .get()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "DataBus is not running"))?;

    let mut state = server
        .state
        .lock()
        .map_err(|_| io::Error::other("DataBus revocation lock poisoned"))?;

    revoke_from_state(&mut state, role, pid)?;
    eprintln!("[DataBus] revoked role={role} pid={pid}");
    Ok(())
}

enum BoundedLine {
    Line(String),
    Eof,
    Idle,
}

fn read_bounded_line(stream: &mut UnixStream) -> io::Result<BoundedLine> {
    let mut bytes = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];

    loop {
        match stream.read(&mut byte) {
            Ok(0) if bytes.is_empty() => return Ok(BoundedLine::Eof),
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated message",
                ));
            }
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) => {
                if bytes.len() >= MAX_MESSAGE {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "message too large",
                    ));
                }
                bytes.push(byte[0]);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                if bytes.is_empty() {
                    return Ok(BoundedLine::Idle);
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        }
    }

    String::from_utf8(bytes)
        .map(BoundedLine::Line)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "message is not UTF-8"))
}

fn send_error(tx: &Sender, code: &str) {
    let payload = serde_json::json!({"error": code}).to_string();
    let _ = tx.try_send(payload);
}

fn handle_client(mut stream: UnixStream, state: Arc<Mutex<State>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let identity = match window_peer::peer_identity(&stream) {
        Ok(identity) => identity,
        Err(_) => return,
    };

    // audit token 提供连接对端的内核 PID/version；
    // proc_pidinfo 再确认该 PID 仍是父进程授权时捕获的同一个进程实例。
    let peer_start_time = match window_peer::process_start_time(identity.pid) {
        Ok(start_time) => start_time,
        Err(_) => return,
    };

    let role = {
        let mut state = match state.lock() {
            Ok(state) => state,
            Err(_) => return,
        };

        let role = state
            .authorizations
            .iter()
            .find(|(_, auth)| auth.pid == identity.pid && auth.start_time == peer_start_time)
            .map(|(role, _)| role.clone());

        let Some(role) = role else {
            return;
        };

        let auth = match state.authorizations.get_mut(&role) {
            Some(auth) => auth,
            None => return,
        };

        match &auth.identity {
            Some(previous) if previous != &identity => return,
            Some(_) => {}
            None => auth.identity = Some(identity.clone()),
        }

        (role, auth.generation)
    };

    let (role, generation) = role;

    let (tx, rx) = mpsc::sync_channel::<String>(OUT_QUEUE);
    let writer_stream = match stream.try_clone() {
        Ok(stream) => stream,
        Err(_) => return,
    };

    let writer = thread::Builder::new()
        .name("jvm-data-bus-writer".into())
        .spawn(move || {
            let mut writer_stream = writer_stream;
            while let Ok(message) = rx.recv() {
                if writer_stream.write_all(message.as_bytes()).is_err()
                    || writer_stream.write_all(b"\n").is_err()
                    || writer_stream.flush().is_err()
                {
                    break;
                }
            }
        });

    if writer.is_err() {
        return;
    }

    {
        let mut state = match state.lock() {
            Ok(state) => state,
            Err(_) => return,
        };

        if state.registrations.contains_key(&role) {
            send_error(&tx, "role_already_connected");
            return;
        }

        state.registrations.insert(
            role.clone(),
            Registration {
                sender: tx.clone(),
                generation,
            },
        );
    }

    eprintln!(
        "[DataBus] authenticated peer connected: role={role} pid={}",
        identity.pid
    );

    loop {
        let line = match read_bounded_line(&mut stream) {
            Ok(BoundedLine::Line(line)) => line,
            Ok(BoundedLine::Eof) => break,
            Ok(BoundedLine::Idle) => {
                let authorized = match state.lock() {
                    Ok(state) => state.authorizations.get(&role).is_some_and(|auth| {
                        auth.pid == identity.pid
                            && auth.generation == generation
                            && auth.identity.as_ref() == Some(&identity)
                    }),
                    Err(_) => false,
                };

                if !authorized {
                    break;
                }
                continue;
            }
            Err(_) => break,
        };

        if line.trim().is_empty() {
            continue;
        }

        let mut message: serde_json::Value = match serde_json::from_str::<serde_json::Value>(&line)
        {
            Ok(value) if value.is_object() => value,
            _ => {
                send_error(&tx, "invalid_json");
                continue;
            }
        };

        if message.get("register").is_some() || message.get("from").is_some() {
            send_error(&tx, "client_identity_fields_forbidden");
            continue;
        }

        let kind = message.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let to = message
            .get("to")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let broadcast = message
            .get("broadcast")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let authoritative = matches!(
            kind,
            "player_move" | "monster_spawn" | "monster_update" | "monster_remove"
        );

        if authoritative && role != "fabric" {
            send_error(&tx, "state_authority_violation");
            continue;
        }

        if to.is_empty() && !broadcast {
            send_error(&tx, "missing_to");
            continue;
        }

        message["from"] = serde_json::Value::String(role.clone());
        let payload = match serde_json::to_string(&message) {
            Ok(payload) if payload.len() <= MAX_MESSAGE => payload,
            _ => {
                send_error(&tx, "message_too_large");
                continue;
            }
        };

        // 授权检查、接收方 generation 检查和 try_send 必须处于同一锁作用域。
        // 这样 revoke() 返回成功后，不可能再有旧授权消息被投递。
        // try_send 是非阻塞操作，不会等待接收队列腾出空间。
        let (authorized, target_count, delivered) = {
            let state = match state.lock() {
                Ok(state) => state,
                Err(_) => break,
            };

            let authorized = state.authorizations.get(&role).is_some_and(|auth| {
                auth.pid == identity.pid
                    && auth.generation == generation
                    && auth.identity.as_ref() == Some(&identity)
            });

            if !authorized {
                (false, 0usize, false)
            } else {
                let mut target_count = 0usize;
                let mut delivered = false;

                if broadcast {
                    for (name, registration) in &state.registrations {
                        if name == &role {
                            continue;
                        }

                        let recipient_authorized = state
                            .authorizations
                            .get(name)
                            .is_some_and(|auth| auth.generation == registration.generation);

                        if !recipient_authorized {
                            continue;
                        }

                        target_count += 1;
                        if registration.sender.try_send(payload.clone()).is_ok() {
                            delivered = true;
                        }
                    }
                } else if let Some(registration) = state.registrations.get(&to) {
                    let recipient_authorized = state
                        .authorizations
                        .get(&to)
                        .is_some_and(|auth| auth.generation == registration.generation);

                    if recipient_authorized {
                        target_count = 1;
                        delivered = registration.sender.try_send(payload.clone()).is_ok();
                    }
                }

                (true, target_count, delivered)
            }
        };

        if !authorized {
            send_error(&tx, "authorization_revoked");
            break;
        }

        if target_count == 0 {
            send_error(&tx, "peer_offline");
            continue;
        }

        if delivered {
            let ack = serde_json::json!({
                "routed": true,
                "from": role,
                "to": if broadcast { "*".to_string() } else { to.clone() }
            })
            .to_string();
            let _ = tx.try_send(ack);
        } else {
            send_error(&tx, "peer_backpressure");
        }
    }

    if let Ok(mut state) = state.lock() {
        let owns_registration = state
            .registrations
            .get(&role)
            .is_some_and(|registration| registration.generation == generation);

        if owns_registration {
            state.registrations.remove(&role);
        }
    }

    eprintln!("[DataBus] peer disconnected: role={role}");
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);

        if let Some(handle) = self.accept_thread.take() {
            if handle.thread().id() != thread::current().id() {
                let _ = handle.join();
            }
        }

        if let Ok(metadata) = fs::symlink_metadata(&self.path) {
            if metadata.file_type().is_socket() && metadata.ino() == self.inode {
                let _ = fs::remove_file(&self.path);
            }
        }

        if let Ok(mut state) = self.state.lock() {
            state.registrations.clear();
            state.authorizations.clear();
        }
        let _ = &self.clients;
    }
}

#[cfg(test)]
mod revocation_tests {
    use super::*;

    fn state_with_authorization(role: &str, pid: u32) -> State {
        let mut authorizations = HashMap::new();
        authorizations.insert(
            role.to_string(),
            Authorization {
                pid,
                start_time: window_peer::ProcessStartTime {
                    seconds: 1,
                    microseconds: 0,
                },
                identity: None,
                generation: 7,
            },
        );

        State {
            authorizations,
            registrations: HashMap::new(),
        }
    }

    #[test]
    fn revoke_removes_only_matching_authorization() {
        let mut state = state_with_authorization("fabric", 1234);

        revoke_from_state(&mut state, "fabric", 1234).unwrap();

        assert!(!state.authorizations.contains_key("fabric"));
        assert!(!state.registrations.contains_key("fabric"));
    }

    #[test]
    fn revoke_rejects_pid_mismatch_without_mutation() {
        let mut state = state_with_authorization("fabric", 1234);

        let error = revoke_from_state(&mut state, "fabric", 5678).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(state.authorizations.get("fabric").unwrap().pid, 1234);
    }

    #[test]
    fn repeated_revoke_reports_not_found() {
        let mut state = state_with_authorization("fabric", 1234);

        revoke_from_state(&mut state, "fabric", 1234).unwrap();

        let error = revoke_from_state(&mut state, "fabric", 1234).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn revoke_removes_live_registration() {
        let mut state = state_with_authorization("fabric", 1234);
        let (tx, _rx) = mpsc::sync_channel(1);
        state.registrations.insert(
            "fabric".to_string(),
            Registration {
                sender: tx,
                generation: 7,
            },
        );

        revoke_from_state(&mut state, "fabric", 1234).unwrap();

        assert!(state.registrations.is_empty());
        assert!(state.authorizations.is_empty());
    }
}

#[cfg(all(test, target_os = "macos"))]
mod integration_tests {
    use super::*;
    use std::io::{BufRead, BufReader, BufWriter};
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::mpsc::{self, Receiver};
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    struct ChildClient {
        child: Child,
        input: BufWriter<ChildStdin>,
        output: Receiver<String>,
    }

    impl ChildClient {
        fn spawn(socket: &std::path::Path, authorized: bool) -> Self {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .arg("--exact")
                .arg("data_bus::integration_tests::subprocess_client_entry")
                .arg("--nocapture")
                .env("JVM_COMBINER_DATABUS_CHILD", "1")
                .env("JVM_COMBINER_DATABUS_SOCKET", socket)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());

            if !authorized {
                command.env("JVM_COMBINER_DATABUS_UNAUTHORIZED", "1");
            }

            let mut child = command.spawn().expect("spawn DataBus child");
            let input = BufWriter::new(child.stdin.take().unwrap());
            let stdout = child.stdout.take().unwrap();
            let (tx, rx) = mpsc::channel();

            thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if line.starts_with("CHILD:") && tx.send(line).is_err() {
                        break;
                    }
                }
            });

            Self {
                child,
                input,
                output: rx,
            }
        }

        fn command(&mut self, command: &str) {
            writeln!(self.input, "{command}").unwrap();
            self.input.flush().unwrap();
        }

        fn receive(&self) -> String {
            self.output
                .recv_timeout(Duration::from_secs(10))
                .expect("timed out waiting for child response")
        }

        fn connect(&mut self) {
            self.command("CONNECT");
            assert_eq!(self.receive(), "CHILD:READY");
        }

        fn send_json(&mut self, json: &str) -> String {
            self.command(&format!("SEND {json}"));
            self.receive()
        }

        fn receive_message(&mut self) -> String {
            self.command("RECV");
            self.receive()
        }
    }

    impl Drop for ChildClient {
        fn drop(&mut self) {
            let _ = writeln!(self.input, "EXIT");
            let _ = self.input.flush();

            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                match self.child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    _ => {
                        let _ = self.child.kill();
                        let _ = self.child.wait();
                        break;
                    }
                }
            }
        }
    }

    fn add_authorization(state: &Arc<Mutex<State>>, role: &str, pid: u32) {
        let start_time = window_peer::process_start_time(pid).unwrap();
        let mut state = state.lock().unwrap();

        assert!(!state.authorizations.contains_key(role));
        assert!(!state.authorizations.values().any(|entry| entry.pid == pid));

        state.authorizations.insert(
            role.to_string(),
            Authorization {
                pid,
                start_time,
                identity: None,
                generation: NEXT_AUTHORIZATION_GENERATION.fetch_add(1, Ordering::Relaxed),
            },
        );
    }

    fn wait_registered(state: &Arc<Mutex<State>>, role: &str, child: &mut ChildClient) {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            {
                let state = state.lock().unwrap();
                if state.registrations.contains_key(role) {
                    return;
                }
            }

            match child.child.try_wait() {
                Ok(Some(status)) => {
                    panic!("child exited before registration: role={role}, status={status}");
                }
                Ok(None) => {}
                Err(error) => panic!("child status check failed: {error}"),
            }

            if let Ok(event) = child.output.try_recv() {
                panic!("child event before registration: role={role}, event={event}");
            }

            if Instant::now() >= deadline {
                let state = state.lock().unwrap();
                let registrations = state.registrations.keys().cloned().collect::<Vec<_>>();
                let authorizations = state.authorizations.keys().cloned().collect::<Vec<_>>();
                panic!(
                    "registration timeout: role={role}, registrations={registrations:?},                      authorizations={authorizations:?}"
                );
            }

            thread::sleep(Duration::from_millis(10));
        }
    }

    fn start_test_server() -> (
        PathBuf,
        Arc<Mutex<State>>,
        Arc<AtomicBool>,
        thread::JoinHandle<()>,
    ) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        let path = std::env::temp_dir().join(format!(
            "jvm-databus-test-{}-{nonce}.sock",
            std::process::id()
        ));

        let listener = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        listener.set_nonblocking(true).unwrap();

        let state = Arc::new(Mutex::new(State {
            authorizations: HashMap::new(),
            registrations: HashMap::new(),
        }));
        let stopping = Arc::new(AtomicBool::new(false));

        let thread_state = state.clone();
        let thread_stopping = stopping.clone();

        let accept_thread = thread::spawn(move || {
            while !thread_stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let state = thread_state.clone();
                        thread::spawn(move || handle_client(stream, state));
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });

        (path, state, stopping, accept_thread)
    }

    fn stop_test_server(
        path: &std::path::Path,
        stopping: &AtomicBool,
        accept_thread: thread::JoinHandle<()>,
    ) {
        stopping.store(true, Ordering::Release);
        let _ = accept_thread.join();
        let _ = fs::remove_file(path);
    }

    fn response_json(line: &str) -> serde_json::Value {
        let payload = line.strip_prefix("CHILD:RESP:").unwrap();
        serde_json::from_str(payload).unwrap()
    }

    #[test]
    fn subprocess_client_entry() {
        if std::env::var_os("JVM_COMBINER_DATABUS_CHILD").is_none() {
            return;
        }

        let socket = std::env::var_os("JVM_COMBINER_DATABUS_SOCKET").unwrap();
        let stdin = io::stdin();
        let mut commands = BufReader::new(stdin.lock());
        let mut stream: Option<UnixStream> = None;
        let mut reader: Option<BufReader<UnixStream>> = None;
        let stdout = io::stdout();

        loop {
            let mut command = String::new();
            if commands.read_line(&mut command).unwrap_or(0) == 0 {
                let mut out = stdout.lock();
                let _ = writeln!(out, "CHILD:STDIN_EOF");
                let _ = out.flush();
                break;
            }

            let command = command.trim_end();
            if command == "EXIT" {
                break;
            }

            if command == "CONNECT" {
                let connected = UnixStream::connect(PathBuf::from(socket.clone()));
                match connected {
                    Ok(connection) => {
                        reader = Some(BufReader::new(connection.try_clone().unwrap()));
                        stream = Some(connection);
                        let mut out = stdout.lock();
                        writeln!(out, "CHILD:READY").unwrap();
                        out.flush().unwrap();
                    }
                    Err(error) => {
                        let mut out = stdout.lock();
                        writeln!(out, "CHILD:CONNECT_ERROR:{error}").unwrap();
                        out.flush().unwrap();
                        break;
                    }
                }
                continue;
            }

            if let Some(json) = command.strip_prefix("SEND ") {
                let Some(connection) = stream.as_mut() else {
                    continue;
                };
                if connection.write_all(json.as_bytes()).is_err()
                    || connection.write_all(b"\n").is_err()
                    || connection.flush().is_err()
                {
                    break;
                }
            } else if command != "RECV" {
                continue;
            }

            let Some(reader) = reader.as_mut() else {
                continue;
            };
            let mut response = String::new();
            let result = reader.read_line(&mut response);
            let reached_eof = matches!(&result, Ok(0));

            let mut out = stdout.lock();
            match result {
                Ok(0) => {
                    writeln!(out, "CHILD:EOF").unwrap();
                }
                Ok(_) => {
                    writeln!(out, "CHILD:RESP:{}", response.trim_end()).unwrap();
                }
                Err(error) => {
                    writeln!(out, "CHILD:READ_ERROR:{error}").unwrap();
                }
            }
            out.flush().unwrap();

            if reached_eof {
                break;
            }
        }
    }

    #[test]
    fn authenticated_subprocess_routing_and_revocation() {
        let (path, state, stopping, accept_thread) = start_test_server();

        let mut fabric = ChildClient::spawn(&path, true);
        let mut neoforge = ChildClient::spawn(&path, true);

        add_authorization(&state, "fabric", fabric.child.id());
        add_authorization(&state, "neoforge", neoforge.child.id());

        fabric.connect();
        neoforge.connect();

        wait_registered(&state, "fabric", &mut fabric);
        wait_registered(&state, "neoforge", &mut neoforge);

        // Fabric -> NeoForge: sender identity is assigned by the server.
        let ack = response_json(&fabric.send_json(r#"{"type":"hello","to":"neoforge"}"#));
        assert_eq!(ack["routed"], true);
        assert_eq!(ack["from"], "fabric");
        assert_eq!(ack["to"], "neoforge");

        let routed = response_json(&neoforge.receive_message());
        assert_eq!(routed["type"], "hello");
        assert_eq!(routed["from"], "fabric");

        // NeoForge -> Fabric: ordinary messages are bidirectional.
        let ack = response_json(&neoforge.send_json(r#"{"type":"hello_back","to":"fabric"}"#));
        assert_eq!(ack["routed"], true);
        let routed = response_json(&fabric.receive_message());
        assert_eq!(routed["type"], "hello_back");
        assert_eq!(routed["from"], "neoforge");

        // Clients cannot inject server-owned identity fields.
        let error = response_json(
            &fabric.send_json(r#"{"type":"hello","to":"neoforge","from":"neoforge"}"#),
        );
        assert_eq!(error["error"], "client_identity_fields_forbidden");

        // Only Fabric may publish authoritative game-state messages.
        let error = response_json(&neoforge.send_json(r#"{"type":"player_move","to":"fabric"}"#));
        assert_eq!(error["error"], "state_authority_violation");

        // An unauthorised process must not acquire a role by connecting.
        let mut unauthorised = ChildClient::spawn(&path, false);
        unauthorised.connect();
        unauthorised.command("RECV");
        assert_eq!(unauthorised.receive(), "CHILD:EOF");
        drop(unauthorised);

        // Cross the server's 5-second idle read timeout without losing authorization.
        thread::sleep(Duration::from_millis(6_200));

        {
            let locked = state.lock().unwrap();
            for role in ["fabric", "neoforge"] {
                assert!(
                    locked.authorizations.contains_key(role),
                    "authorization unexpectedly lost after idle timeout: {role}"
                );
                assert!(
                    locked.registrations.contains_key(role),
                    "registration unexpectedly lost after idle timeout: {role}"
                );
            }
        }

        // An idle connection must remain fully usable after the timeout.
        let ack = response_json(&neoforge.send_json(r#"{"type":"idle_survival","to":"fabric"}"#));
        assert_eq!(ack["routed"], true);
        assert_eq!(ack["from"], "neoforge");

        let routed = response_json(&fabric.receive_message());
        assert_eq!(routed["type"], "idle_survival");
        assert_eq!(routed["from"], "neoforge");

        // Revocation during the next idle read cycle must close the old connection.
        {
            let mut locked = state.lock().unwrap();
            revoke_from_state(&mut locked, "fabric", fabric.child.id()).unwrap();
        }

        fabric.command("RECV");
        assert_eq!(fabric.receive(), "CHILD:EOF");

        drop(fabric);
        drop(neoforge);

        stop_test_server(&path, &stopping, accept_thread);
    }
}
