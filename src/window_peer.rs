use std::io;
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerIdentity {
    pub pid: u32,
    pub pid_version: i32,
    pub audit_token: [u32; 8],
}

// 与 macOS SDK mach/message.h 中 audit_token_t 的 ABI 一致。
#[repr(C)]
#[derive(Clone, Copy)]
struct AuditToken {
    val: [u32; 8],
}

#[cfg(target_os = "macos")]
#[link(name = "bsm")]
unsafe extern "C" {
    fn audit_token_to_pid(token: AuditToken) -> libc::pid_t;
    fn audit_token_to_pidversion(token: AuditToken) -> libc::c_int;
}

fn credential_error(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

#[cfg(target_os = "macos")]
pub fn peer_identity(stream: &UnixStream) -> io::Result<PeerIdentity> {
    let fd = stream.as_raw_fd();

    // 第一重身份：内核提供的 peer PID。
    let mut peer_pid: libc::pid_t = 0;
    let mut pid_len = size_of::<libc::pid_t>() as libc::socklen_t;

    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            &mut peer_pid as *mut _ as *mut libc::c_void,
            &mut pid_len,
        )
    };

    if result != 0 {
        return Err(io::Error::last_os_error());
    }

    if pid_len as usize != size_of::<libc::pid_t>() || peer_pid <= 0 {
        return Err(credential_error("invalid LOCAL_PEERPID"));
    }

    // 第二重身份：内核提供的完整 audit token。
    let mut token = AuditToken { val: [0; 8] };
    let mut token_len = size_of::<AuditToken>() as libc::socklen_t;

    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERTOKEN,
            &mut token as *mut _ as *mut libc::c_void,
            &mut token_len,
        )
    };

    if result != 0 {
        return Err(io::Error::last_os_error());
    }

    if token_len as usize != size_of::<AuditToken>() {
        return Err(credential_error("invalid LOCAL_PEERTOKEN length"));
    }

    // 使用 Apple libbsm 官方解析函数，不自行拆解 token。
    let token_pid = unsafe { audit_token_to_pid(token) };
    let pid_version = unsafe { audit_token_to_pidversion(token) };

    if token_pid <= 0 || token_pid != peer_pid {
        return Err(credential_error("peer PID and audit token PID mismatch"));
    }

    if pid_version <= 0 {
        return Err(credential_error("invalid audit token PID version"));
    }

    Ok(PeerIdentity {
        pid: peer_pid as u32,
        pid_version,
        audit_token: token.val,
    })
}

#[cfg(not(target_os = "macos"))]
pub fn peer_identity(_stream: &UnixStream) -> io::Result<PeerIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Window Manager peer credentials require macOS",
    ))
}
