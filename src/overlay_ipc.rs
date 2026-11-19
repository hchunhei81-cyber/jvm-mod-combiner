use serde::Deserialize;
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TOKEN_BYTES: usize = 32;
const TOKEN_HEX_LEN: usize = TOKEN_BYTES * 2;
const IO_TIMEOUT: Duration = Duration::from_secs(4);
const START_TIMEOUT: Duration = Duration::from_secs(8);
const PROBE_COMMAND: &str = "__overlay_health_probe__";

#[derive(Debug, Deserialize)]
struct ProbeResponse {
    version: u32,
    #[serde(rename = "requestID")]
    request_id: Option<String>,
    success: bool,
    error: Option<String>,
}

#[derive(Clone, Copy)]
struct SocketIdentity {
    device: u64,
    inode: u64,
}

pub(crate) struct OverlaySupervisor {
    child: Child,
    socket_path: PathBuf,
    socket_identity: Option<SocketIdentity>,
}

impl OverlaySupervisor {
    pub(crate) fn start() -> io::Result<Self> {
        #[cfg(not(target_os = "macos"))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "native Overlay backend is currently supported only on macOS",
            ));
        }

        #[cfg(target_os = "macos")]
        {
            Self::start_macos()
        }
    }

    #[cfg(target_os = "macos")]
    fn start_macos() -> io::Result<Self> {
        let executable = env::var_os("JVM_COMBINER_OVERLAY_BIN")
            .map(PathBuf::from)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "JVM_COMBINER_OVERLAY_BIN is not configured",
                )
            })?;

        validate_executable(&executable)?;

        let runtime_dir = crate::window_manager::private_runtime_dir()?;
        let socket_path = make_socket_path(&runtime_dir)?;

        if socket_path.as_os_str().as_encoded_bytes().len() >= 104 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Overlay UDS path is too long",
            ));
        }

        let token = generate_token()?;
        let (read_fd, write_fd) = create_token_channel()?;
        let read_raw = read_fd.as_raw_fd();

        let mut command = Command::new(&executable);
        command
            .env("JVM_COMBINER_OVERLAY_TOKEN_FD", read_raw.to_string())
            .env("JVM_COMBINER_OVERLAY_SOCKET", socket_path.as_os_str())
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                drop(read_fd);
                drop(write_fd);
                return Err(error);
            }
        };

        // The child inherited the read endpoint. Only the parent retains
        // the write endpoint from this point onward.
        drop(read_fd);

        let mut token_writer =
            unsafe { File::from_raw_fd(std::os::fd::IntoRawFd::into_raw_fd(write_fd)) };

        let token_write_result = token_writer
            .write_all(token.as_bytes())
            .and_then(|_| token_writer.flush());

        drop(token_writer);

        if let Err(error) = token_write_result {
            terminate_child(&mut child);
            return Err(io::Error::new(
                error.kind(),
                format!("failed to deliver Overlay startup token: {error}"),
            ));
        }

        let mut supervisor = Self {
            child,
            socket_path,
            socket_identity: None,
        };

        if let Err(error) = supervisor.connect_and_authenticate(&token) {
            return Err(error);
        }

        Ok(supervisor)
    }

    #[cfg(target_os = "macos")]
    fn connect_and_authenticate(&mut self, token: &str) -> io::Result<()> {
        let deadline = Instant::now() + START_TIMEOUT;
        let mut last_error = None;

        loop {
            if let Some(status) = self.child.try_wait()? {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("Overlay backend exited during startup: {status}"),
                ));
            }

            if Instant::now() >= deadline {
                let detail = last_error
                    .as_ref()
                    .map(|error: &io::Error| error.to_string())
                    .unwrap_or_else(|| "no connection attempt completed".to_owned());

                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("Overlay backend did not become ready: {detail}"),
                ));
            }

            // Capture the owned socket inode before connecting. This
            // preserves cleanup identity even if peer authentication fails.
            if let Some(identity) = capture_socket_identity(&self.socket_path)? {
                match self.socket_identity {
                    None => self.socket_identity = Some(identity),
                    Some(existing)
                        if existing.device == identity.device
                            && existing.inode == identity.inode => {}
                    Some(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "Overlay socket identity changed during startup",
                        ));
                    }
                }
            }

            match connect_verified(&self.socket_path) {
                Ok((mut stream, identity)) => {
                    match self.socket_identity {
                        None => self.socket_identity = Some(identity),
                        Some(existing)
                            if existing.device == identity.device
                                && existing.inode == identity.inode => {}
                        Some(_) => {
                            return Err(io::Error::new(
                                io::ErrorKind::PermissionDenied,
                                "Overlay socket identity changed during connection",
                            ));
                        }
                    }
                    let request_id = format!(
                        "startup-{}",
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                    );

                    let request = serde_json::json!({
                        "version": 1,
                        "requestID": request_id,
                        "token": token,
                        "command": PROBE_COMMAND
                    });

                    let payload = serde_json::to_vec(&request)
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;

                    let length = u32::try_from(payload.len()).map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "Overlay probe request is too large",
                        )
                    })?;

                    stream.write_all(&length.to_be_bytes())?;
                    stream.write_all(&payload)?;

                    let mut header = [0u8; 4];
                    stream.read_exact(&mut header)?;

                    let response_len = u32::from_be_bytes(header) as usize;
                    if response_len == 0 || response_len > 16 * 1024 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Overlay probe response has an invalid length",
                        ));
                    }

                    let mut response_bytes = vec![0u8; response_len];
                    stream.read_exact(&mut response_bytes)?;

                    let response: ProbeResponse = serde_json::from_slice(&response_bytes)
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;

                    if response.version != 1
                        || response.request_id.as_deref() != Some(request_id.as_str())
                        || response.success
                        || response.error.as_deref() != Some("invalid_command")
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "Overlay authentication probe returned an unexpected response",
                        ));
                    }

                    return Ok(());
                }
                Err(error) => {
                    last_error = Some(error);
                }
            }

            thread::sleep(Duration::from_millis(100));
        }
    }

    pub(crate) fn check_alive(&mut self) -> io::Result<()> {
        match self.child.try_wait()? {
            None => Ok(()),
            Some(status) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                format!("Overlay backend exited unexpectedly: {status}"),
            )),
        }
    }

    pub(crate) fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Returns the supervised Overlay process ID.
    ///
    /// This is primarily useful for process-level diagnostics and
    /// fault-injection tests. It does not grant process control authority
    /// to external clients.
    #[cfg(test)]
    pub fn child_pid(&self) -> u32 {
        self.child.id()
    }

    fn cleanup_owned_socket(&mut self) {
        let Some(identity) = self.socket_identity.take() else {
            return;
        };

        let Ok(metadata) = fs::symlink_metadata(&self.socket_path) else {
            return;
        };

        if metadata.file_type().is_socket()
            && metadata.dev() == identity.device
            && metadata.ino() == identity.inode
        {
            if let Err(error) = fs::remove_file(&self.socket_path) {
                eprintln!("[Overlay] Could not remove owned socket: {error}");
            }
        }
    }
}

impl Drop for OverlaySupervisor {
    fn drop(&mut self) {
        terminate_child(&mut self.child);
        self.cleanup_owned_socket();
    }
}

fn terminate_child(child: &mut Child) {
    match child.try_wait() {
        Ok(Some(_)) => {}
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn make_socket_path(runtime_dir: &Path) -> io::Result<PathBuf> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    let path = runtime_dir.join(format!("overlay-{}-{nonce}.sock", std::process::id()));

    if path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Overlay socket path already exists",
        ));
    }

    Ok(path)
}

fn generate_token() -> io::Result<String> {
    let mut bytes = [0u8; TOKEN_BYTES];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;

    let mut token = String::with_capacity(TOKEN_HEX_LEN);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut token, "{byte:02x}")
            .map_err(|_| io::Error::other("failed to encode startup token"))?;
    }

    Ok(token)
}

#[cfg(target_os = "macos")]
fn create_token_channel() -> io::Result<(OwnedFd, OwnedFd)> {
    use std::os::fd::FromRawFd;

    let mut fds = [-1; 2];

    let result = unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) };

    if result != 0 {
        return Err(io::Error::last_os_error());
    }

    let read_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let write_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };

    let read_flags = unsafe { libc::fcntl(read_fd.as_raw_fd(), libc::F_GETFD) };
    if read_flags < 0
        || unsafe {
            libc::fcntl(
                read_fd.as_raw_fd(),
                libc::F_SETFD,
                read_flags & !libc::FD_CLOEXEC,
            )
        } < 0
    {
        return Err(io::Error::last_os_error());
    }

    let write_flags = unsafe { libc::fcntl(write_fd.as_raw_fd(), libc::F_GETFD) };
    if write_flags < 0
        || unsafe {
            libc::fcntl(
                write_fd.as_raw_fd(),
                libc::F_SETFD,
                write_flags | libc::FD_CLOEXEC,
            )
        } < 0
    {
        return Err(io::Error::last_os_error());
    }

    let no_sigpipe: libc::c_int = 1;
    let option_result = unsafe {
        libc::setsockopt(
            write_fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            &no_sigpipe as *const _ as *const libc::c_void,
            std::mem::size_of_val(&no_sigpipe) as libc::socklen_t,
        )
    };

    if option_result != 0 {
        return Err(io::Error::last_os_error());
    }

    Ok((read_fd, write_fd))
}

#[cfg(target_os = "macos")]
fn validate_executable(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let uid = unsafe { libc::getuid() };

    if !metadata.file_type().is_file()
        || metadata.uid() != uid
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Overlay executable must be a user-owned regular executable file",
        ));
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn capture_socket_identity(path: &Path) -> io::Result<Option<SocketIdentity>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };

    validate_socket_metadata(&metadata, unsafe { libc::getuid() })?;

    Ok(Some(SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }))
}

#[cfg(target_os = "macos")]
fn connect_verified(path: &Path) -> io::Result<(UnixStream, SocketIdentity)> {
    let before = fs::symlink_metadata(path)?;
    let uid = unsafe { libc::getuid() };

    validate_socket_metadata(&before, uid)?;

    let identity = SocketIdentity {
        device: before.dev(),
        inode: before.ino(),
    };

    let stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;

    let after = fs::symlink_metadata(path)?;
    validate_socket_metadata(&after, uid)?;

    if after.dev() != identity.device || after.ino() != identity.inode {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Overlay socket identity changed during connection",
        ));
    }

    let mut peer_uid: libc::uid_t = 0;
    let mut peer_gid: libc::gid_t = 0;

    let peer_result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut peer_uid, &mut peer_gid) };

    if peer_result != 0 {
        return Err(io::Error::last_os_error());
    }

    if peer_uid != uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Overlay socket peer UID does not match current user",
        ));
    }

    Ok((stream, identity))
}

#[cfg(target_os = "macos")]
fn validate_socket_metadata(metadata: &fs::Metadata, expected_uid: libc::uid_t) -> io::Result<()> {
    if !metadata.file_type().is_socket()
        || metadata.uid() != expected_uid
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Overlay socket type, owner, or permissions are invalid",
        ));
    }

    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod supervisor_runtime_tests {
    use super::OverlaySupervisor;
    use std::fs;
    use std::io::ErrorKind;
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};

    #[test]
    #[ignore = "requires JVM_COMBINER_OVERLAY_BIN pointing to the built macOS overlay"]
    fn supervisor_detects_unexpected_overlay_termination() {
        use std::thread;
        use std::time::{Duration, Instant};

        let mut supervisor =
            OverlaySupervisor::start().expect("Overlay must start and authenticate");

        let pid = supervisor.child_pid();

        assert!(pid > 0, "Overlay PID must be valid");

        println!("[INFO] Overlay child PID: {pid}");

        // Inject an unexpected process termination.
        let result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };

        assert_eq!(result, 0, "SIGKILL fault injection must succeed");

        println!("[FAULT] Overlay process killed");

        // Wait for the operating system to report process termination.
        let deadline = Instant::now() + Duration::from_secs(3);

        loop {
            match supervisor.check_alive() {
                Ok(()) => {
                    assert!(
                        Instant::now() < deadline,
                        "Supervisor failed to detect terminated Overlay"
                    );

                    thread::sleep(Duration::from_millis(20));
                }

                Err(error) => {
                    println!("[PASS] Unexpected termination detected: {error}");
                    break;
                }
            }
        }

        // Drop must still safely clean up the owned socket.
        let socket_path = supervisor.socket_path().to_path_buf();

        drop(supervisor);

        assert!(
            !socket_path.exists(),
            "Owned Overlay socket must be removed after child termination"
        );

        println!("[PASS] Overlay socket cleaned after unexpected termination");
    }

    #[test]
    #[ignore = "requires JVM_COMBINER_OVERLAY_BIN pointing to the built macOS overlay"]
    fn supervisor_start_authenticate_liveness_and_cleanup() {
        let mut supervisor = OverlaySupervisor::start()
            .expect("Overlay startup and authenticated handshake must succeed");

        let socket_path = supervisor.socket_path().to_path_buf();

        let metadata =
            fs::symlink_metadata(&socket_path).expect("authenticated Overlay socket must exist");

        assert!(
            metadata.file_type().is_socket(),
            "path must be a Unix socket"
        );
        assert_eq!(
            metadata.permissions().mode() & 0o777,
            0o600,
            "socket permissions must be 0600"
        );
        assert_eq!(
            metadata.uid(),
            unsafe { libc::getuid() },
            "socket must belong to the current user"
        );

        supervisor
            .check_alive()
            .expect("Overlay child must remain alive after authentication");

        drop(supervisor);

        match fs::symlink_metadata(&socket_path) {
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Ok(_) => panic!("owned Overlay socket was not removed by Drop"),
            Err(error) => panic!("could not verify Overlay socket cleanup: {error}"),
        }
    }
}
