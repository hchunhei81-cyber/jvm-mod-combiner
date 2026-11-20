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

// macOS PROC_PIDTBSDINFO 的公开 ABI。
// 启动时间用于父进程预授权时绑定 PID，避免 PID 被复用后继承旧授权。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessStartTime {
    pub seconds: u64,
    pub microseconds: u64,
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct ProcBsdInfo {
    pbi_flags: u32,
    pbi_status: u32,
    pbi_xstatus: u32,
    pbi_pid: u32,
    pbi_ppid: u32,
    pbi_uid: libc::uid_t,
    pbi_gid: libc::gid_t,
    pbi_ruid: libc::uid_t,
    pbi_rgid: libc::gid_t,
    pbi_svuid: libc::uid_t,
    pbi_svgid: libc::gid_t,
    rfu_1: u32,
    pbi_comm: [u8; 16],
    pbi_name: [u8; 32],
    pbi_nfiles: u32,
    pbi_pgid: u32,
    pbi_pjobc: u32,
    e_tdev: u32,
    e_tpgid: u32,
    pbi_nice: i32,
    pbi_start_tvsec: u64,
    pbi_start_tvusec: u64,
}

#[cfg(target_os = "macos")]
#[link(name = "proc")]
unsafe extern "C" {
    fn proc_pidinfo(
        pid: libc::c_int,
        flavor: libc::c_int,
        arg: u64,
        buffer: *mut std::ffi::c_void,
        buffersize: libc::c_int,
    ) -> libc::c_int;
}

#[cfg(target_os = "macos")]
pub fn process_start_time(pid: u32) -> io::Result<ProcessStartTime> {
    const PROC_PIDTBSDINFO: libc::c_int = 3;

    let raw_pid = libc::c_int::try_from(pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "PID out of range"))?;

    if raw_pid <= 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid PID"));
    }

    let mut info: ProcBsdInfo = unsafe { std::mem::zeroed() };
    let expected_size = std::mem::size_of::<ProcBsdInfo>();

    // SDK proc_bsdinfo 在 macOS LP64 ABI 下应为 136 字节。
    if expected_size != 136 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected proc_bsdinfo ABI size",
        ));
    }

    let result = unsafe {
        proc_pidinfo(
            raw_pid,
            PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut std::ffi::c_void,
            expected_size as libc::c_int,
        )
    };

    if result != expected_size as libc::c_int {
        return Err(if result < 0 {
            io::Error::last_os_error()
        } else {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "proc_pidinfo returned incomplete process information",
            )
        });
    }

    if info.pbi_pid != pid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "proc_pidinfo returned a different PID",
        ));
    }

    if info.pbi_start_tvsec == 0 || info.pbi_start_tvusec >= 1_000_000 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid process start timestamp",
        ));
    }

    Ok(ProcessStartTime {
        seconds: info.pbi_start_tvsec,
        microseconds: info.pbi_start_tvusec,
    })
}

#[cfg(not(target_os = "macos"))]
pub fn process_start_time(_pid: u32) -> io::Result<ProcessStartTime> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "process start-time capture requires macOS",
    ))
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

#[cfg(all(test, target_os = "macos"))]
mod process_start_time_tests {
    use super::*;
    use std::process::{Command, Stdio};

    #[test]
    fn proc_bsdinfo_matches_expected_macos_abi() {
        assert_eq!(std::mem::size_of::<ProcBsdInfo>(), 136);
        assert_eq!(std::mem::offset_of!(ProcBsdInfo, pbi_start_tvsec), 120);
        assert_eq!(std::mem::offset_of!(ProcBsdInfo, pbi_start_tvusec), 128);
    }

    #[test]
    fn captures_current_process_start_time() {
        let start = process_start_time(std::process::id())
            .expect("current process start time should be readable");

        assert!(start.seconds > 0);
        assert!(start.microseconds < 1_000_000);
    }

    #[test]
    fn captures_independent_child_process_start_time() {
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep child should start");

        let result = process_start_time(child.id());

        let _ = child.kill();
        let _ = child.wait();

        let start = result.expect("child process start time should be readable");
        assert!(start.seconds > 0);
        assert!(start.microseconds < 1_000_000);
    }

    #[test]
    fn rejects_zero_pid() {
        let error = process_start_time(0).expect_err("PID zero must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
