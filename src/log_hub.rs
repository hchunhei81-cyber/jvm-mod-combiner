use crate::{window_manager, window_peer};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CAPACITY: usize = 2048;
const MAX_MESSAGE_BYTES: usize = 2048;
const MAX_REQUEST_BYTES: usize = 1024;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_BATCH: usize = 16;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
const POLL_INTERVAL: Duration = Duration::from_millis(2);

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub id: u64,
    pub timestamp_ms: u64,
    pub source: String,
    pub stream: String,
    pub level: String,
    pub message: String,
}

#[derive(Default)]
struct LogState {
    next_id: u64,
    dropped: u64,
    entries: VecDeque<LogEntry>,
}

#[derive(Default)]
pub struct LogHub {
    state: Mutex<LogState>,
}

impl LogHub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn push(&self, source: &str, stream: &str, level: &str, message: &str) {
        let source = match source {
            "fusion" | "fabric" | "neoforge" => source,
            _ => "fusion",
        };
        let stream = if stream == "stderr" {
            "stderr"
        } else {
            "stdout"
        };
        let level = match level {
            "ERROR" | "WARN" | "INFO" | "DEBUG" | "TRACE" => level,
            _ => "UNKNOWN",
        };

        let message = truncate_utf8(message, MAX_MESSAGE_BYTES);
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64;

        let Ok(mut state) = self.state.lock() else {
            return;
        };

        state.next_id = state.next_id.saturating_add(1);
        let entry = LogEntry {
            id: state.next_id,
            timestamp_ms,
            source: source.to_owned(),
            stream: stream.to_owned(),
            level: level.to_owned(),
            message,
        };

        if state.entries.len() == CAPACITY {
            state.entries.pop_front();
            state.dropped = state.dropped.saturating_add(1);
        }
        state.entries.push_back(entry);
    }

    fn batch(&self, after_id: u64, limit: usize) -> io::Result<LogBatch> {
        let state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("log hub lock poisoned"))?;

        let latest_id = state.next_id;
        let first_available_id = state
            .entries
            .front()
            .map(|entry| entry.id)
            .unwrap_or_else(|| latest_id.saturating_add(1));

        let cursor_reset = after_id > latest_id;
        let effective_after_id = if cursor_reset { 0 } else { after_id };

        let missing_count = if effective_after_id > 0
            && effective_after_id < first_available_id.saturating_sub(1)
        {
            first_available_id
                .saturating_sub(effective_after_id)
                .saturating_sub(1)
        } else {
            0
        };

        let gap_detected = missing_count > 0;

        let entries = if effective_after_id == 0 {
            // Initial connection and cursor reset: replay retained history
            // from the oldest available entry, preserving forward pagination.
            state.entries.iter().take(limit).cloned().collect()
        } else {
            state
                .entries
                .iter()
                .filter(|entry| entry.id > effective_after_id)
                .take(limit)
                .cloned()
                .collect()
        };

        Ok(LogBatch {
            entries,
            latest_id,
            first_available_id,
            dropped: state.dropped,
            gap_detected,
            missing_count,
            cursor_reset,
        })
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }

    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn detect_level(line: &str, stream: &str) -> &'static str {
    let upper = line.to_ascii_uppercase();

    // Minecraft Log4j format: [Render thread/ERROR]: message
    // Also accept conventional standalone severity prefixes.
    for (needle, level) in [
        ("/ERROR]", "ERROR"),
        ("[ERROR]", "ERROR"),
        (" ERROR", "ERROR"),
        ("/FATAL]", "ERROR"),
        ("[FATAL]", "ERROR"),
        (" FATAL", "ERROR"),
        ("/WARNING]", "WARN"),
        ("/WARN]", "WARN"),
        ("[WARNING]", "WARN"),
        ("[WARN]", "WARN"),
        (" WARNING", "WARN"),
        (" WARN", "WARN"),
        ("/INFO]", "INFO"),
        ("[INFO]", "INFO"),
        (" INFO", "INFO"),
        ("/DEBUG]", "DEBUG"),
        ("[DEBUG]", "DEBUG"),
        (" DEBUG", "DEBUG"),
        ("/TRACE]", "TRACE"),
        ("[TRACE]", "TRACE"),
        (" TRACE", "TRACE"),
    ] {
        if upper.contains(needle) {
            return level;
        }
    }

    if stream == "stderr" {
        "WARN"
    } else {
        "UNKNOWN"
    }
}

// Bounded line reader: never accumulates more than MAX_MESSAGE_BYTES,
// even if a child writes an arbitrarily large line without a newline.
pub fn pump_reader<R: Read>(
    reader: R,
    hub: Arc<LogHub>,
    source: &'static str,
    stream: &'static str,
) {
    let mut reader = io::BufReader::new(reader);
    let mut line = Vec::with_capacity(512);
    let mut truncated = false;

    loop {
        let (consumed, newline, eof) = {
            let available = match reader.fill_buf() {
                Ok(bytes) => bytes,
                Err(error) => {
                    hub.push(
                        source,
                        stream,
                        "ERROR",
                        &format!("log pipe read failed: {error}"),
                    );
                    return;
                }
            };

            if available.is_empty() {
                (0, false, true)
            } else {
                let count = available
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map(|index| index + 1)
                    .unwrap_or(available.len());

                for byte in &available[..count] {
                    if *byte == b'\n' {
                        break;
                    }
                    if line.len() < MAX_MESSAGE_BYTES {
                        line.push(*byte);
                    } else {
                        truncated = true;
                    }
                }
                (count, available[count - 1] == b'\n', false)
            }
        };

        if eof {
            if !line.is_empty() || truncated {
                emit_line(&mut line, &mut truncated, &hub, source, stream);
            }
            return;
        }

        reader.consume(consumed);

        if newline {
            emit_line(&mut line, &mut truncated, &hub, source, stream);
        }
    }
}

fn emit_line(
    bytes: &mut Vec<u8>,
    truncated: &mut bool,
    hub: &LogHub,
    source: &'static str,
    stream: &'static str,
) {
    while bytes.last() == Some(&b'\r') {
        bytes.pop();
    }

    let mut message = String::from_utf8_lossy(bytes).into_owned();
    if *truncated {
        message.push_str(" …[line truncated]");
    }

    let empty_stdout_wrapper = message
        .rsplit_once("[STDOUT]:")
        .is_some_and(|(_, payload)| payload.trim().is_empty());

    let empty_stderr_wrapper = message
        .rsplit_once("[STDERR]:")
        .is_some_and(|(_, payload)| payload.trim().is_empty());

    if !message.is_empty() && !empty_stdout_wrapper && !empty_stderr_wrapper {
        let level = detect_level(&message, stream);
        hub.push(source, stream, level, &message);
    }

    bytes.clear();
    *truncated = false;
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LogRequest {
    protocol: u64,
    #[serde(rename = "type")]
    message_type: String,
    after_id: u64,
    limit: usize,
}

#[derive(Serialize)]
struct LogResponse {
    protocol: u64,
    #[serde(rename = "type")]
    message_type: &'static str,
    entries: Vec<LogEntry>,
    latest_id: u64,
    first_available_id: u64,
    dropped: u64,
    gap_detected: bool,
    missing_count: u64,
    cursor_reset: bool,
}

struct LogBatch {
    entries: Vec<LogEntry>,
    latest_id: u64,
    first_available_id: u64,
    dropped: u64,
    gap_detected: bool,
    missing_count: u64,
    cursor_reset: bool,
}

pub struct LogServer {
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    socket_path: PathBuf,
    socket_dev: u64,
    socket_ino: u64,
}

impl LogServer {
    pub fn start(hub: Arc<LogHub>) -> io::Result<Self> {
        let socket_path = window_manager::unique_socket_path("logs")?;
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
                "Log socket path is not a socket",
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
            .name("log-uds".to_owned())
            .spawn(move || {
                while !worker_shutdown.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if worker_shutdown.load(Ordering::Acquire) {
                                break;
                            }
                            if let Err(error) = handle_client(stream, &hub, &worker_shutdown) {
                                eprintln!("[LogHub] Client rejected: {error}");
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(POLL_INTERVAL);
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(error) => {
                            eprintln!("[LogHub] Accept failed: {error}");
                            thread::sleep(Duration::from_millis(20));
                        }
                    }
                }
                eprintln!("[LogHub] Worker stopped");
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

impl Drop for LogServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
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
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "log request deadline exceeded"))
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

fn handle_client(mut stream: UnixStream, hub: &LogHub, shutdown: &AtomicBool) -> io::Result<()> {
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    remaining(deadline)?;
    let _peer = window_peer::peer_identity(&stream)?;
    remaining(deadline)?;

    let bytes = read_request(&mut stream, deadline, shutdown)?;
    let request: LogRequest = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;

    if request.protocol != 1
        || request.message_type != "logs"
        || request.limit == 0
        || request.limit > MAX_BATCH
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid log request",
        ));
    }

    let batch = hub.batch(request.after_id, request.limit)?;
    let response = LogResponse {
        protocol: 1,
        message_type: "logs",
        entries: batch.entries,
        latest_id: batch.latest_id,
        first_available_id: batch.first_available_id,
        dropped: batch.dropped,
        gap_detected: batch.gap_detected,
        missing_count: batch.missing_count,
        cursor_reset: batch.cursor_reset,
    };

    let mut output = serde_json::to_vec(&response).map_err(io::Error::other)?;
    if output.len() + 1 > MAX_RESPONSE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "log response too large",
        ));
    }
    output.push(b'\n');

    stream.set_nonblocking(true)?;
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
    fn ring_buffer_is_bounded_and_counts_evictions() {
        let hub = LogHub::new();
        for index in 0..(CAPACITY + 3) {
            hub.push("fabric", "stdout", "INFO", &format!("line-{index}"));
        }

        let batch = hub.batch(0, MAX_BATCH).unwrap();
        assert_eq!(batch.entries.len(), MAX_BATCH);
        assert_eq!(batch.dropped, 3);
        assert_eq!(batch.latest_id, (CAPACITY + 3) as u64);
        assert_eq!(batch.entries.first().unwrap().id, 4);
        assert_eq!(batch.entries.first().unwrap().message, "line-3");
        assert_eq!(batch.entries.last().unwrap().message, "line-18");
    }

    #[test]
    fn cursor_returns_only_new_entries() {
        let hub = LogHub::new();
        hub.push("fusion", "stdout", "INFO", "one");
        hub.push("fabric", "stderr", "ERROR", "two");

        let batch = hub.batch(1, MAX_BATCH).unwrap();
        assert_eq!(batch.entries.len(), 1);
        assert_eq!(batch.entries[0].id, 2);
        assert_eq!(batch.entries[0].source, "fabric");
        assert_eq!(batch.entries[0].level, "ERROR");
    }

    #[test]
    fn messages_are_truncated_at_utf8_boundary() {
        let value = "界".repeat(MAX_MESSAGE_BYTES);
        let hub = LogHub::new();
        hub.push("fabric", "stdout", "INFO", &value);
        let batch = hub.batch(0, 1).unwrap();
        assert!(batch.entries[0].message.len() <= MAX_MESSAGE_BYTES);
        assert!(
            batch.entries[0]
                .message
                .is_char_boundary(batch.entries[0].message.len())
        );
    }

    #[test]
    fn cursor_reports_missing_entries_after_ring_eviction() {
        let hub = LogHub::new();

        for index in 0..(CAPACITY + 3) {
            hub.push("fabric", "stdout", "INFO", &format!("line-{index}"));
        }

        let batch = hub.batch(1, MAX_BATCH).unwrap();

        assert_eq!(batch.first_available_id, 4);
        assert!(batch.gap_detected);
        assert_eq!(batch.missing_count, 2);
        assert!(!batch.cursor_reset);
        assert_eq!(batch.entries.first().unwrap().id, 4);
    }

    #[test]
    fn cursor_reset_recovers_after_server_restart() {
        let hub = LogHub::new();
        hub.push("fabric", "stdout", "INFO", "one");
        hub.push("neoforge", "stdout", "INFO", "two");

        let batch = hub.batch(100, MAX_BATCH).unwrap();

        assert!(batch.cursor_reset);
        assert!(!batch.gap_detected);
        assert_eq!(batch.missing_count, 0);
        assert_eq!(batch.entries.len(), 2);
        assert_eq!(batch.entries[0].id, 1);
        assert_eq!(batch.entries[1].id, 2);
    }

    #[test]
    fn empty_hub_reports_next_available_id() {
        let hub = LogHub::new();
        let batch = hub.batch(0, MAX_BATCH).unwrap();

        assert_eq!(batch.latest_id, 0);
        assert_eq!(batch.first_available_id, 1);
        assert!(batch.entries.is_empty());
        assert!(!batch.gap_detected);
        assert!(!batch.cursor_reset);
    }

    #[test]
    fn level_detection_handles_common_prefixes() {
        assert_eq!(detect_level("[main/ERROR]: failed", "stdout"), "ERROR");
        assert_eq!(detect_level("[main/WARN]: warning", "stdout"), "WARN");
        assert_eq!(detect_level("plain stderr", "stderr"), "WARN");
        assert_eq!(detect_level("plain stdout", "stdout"), "UNKNOWN");
    }
}
