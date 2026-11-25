use serde_json::json;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_NONCE: AtomicU64 = AtomicU64::new(0);

pub struct RuntimeManifest {
    path: PathBuf,
    dev: u64,
    ino: u64,
}

fn runtime_dir() -> io::Result<PathBuf> {
    let uid = unsafe { libc::geteuid() };
    let path = PathBuf::from(format!("/tmp/jvm-combiner-{uid}"));
    let metadata = fs::symlink_metadata(&path)?;

    if !metadata.file_type().is_dir() || metadata.uid() != uid || metadata.mode() & 0o777 != 0o700 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Runtime directory identity or permissions are invalid",
        ));
    }

    Ok(path)
}

fn validate_socket(path: &Path, dir: &Path, prefix: &str) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let name = path.file_name().and_then(|v| v.to_str()).unwrap_or("");

    if path.parent() != Some(dir)
        || !name.starts_with(prefix)
        || !name.ends_with(".sock")
        || !metadata.file_type().is_socket()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Runtime socket identity or permissions are invalid",
        ));
    }

    Ok(())
}

impl RuntimeManifest {
    pub fn publish(logs_socket: &Path, diagnostics_socket: &Path) -> io::Result<Self> {
        let dir = runtime_dir()?;
        validate_socket(logs_socket, &dir, "logs-")?;
        validate_socket(diagnostics_socket, &dir, "diagnostics-")?;

        let pid = std::process::id();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let counter = NEXT_NONCE.fetch_add(1, Ordering::Relaxed);
        let nonce = format!("{timestamp}-{counter}");

        let final_path = dir.join(format!("runtime-{pid}-{nonce}.json"));
        let temp_path = dir.join(format!(".runtime-{pid}-{nonce}.tmp"));

        let document = json!({
            "protocol": 1,
            "pid": pid,
            "nonce": nonce,
            "logs_socket": logs_socket,
            "diagnostics_socket": diagnostics_socket,
        });
        let bytes = serde_json::to_vec(&document).map_err(io::Error::other)?;

        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut file = options.open(&temp_path)?;

        let write_result = (|| {
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp_path, &final_path)?;
            File::open(&dir)?.sync_all()?;
            Ok::<(), io::Error>(())
        })();

        if let Err(error) = write_result {
            let _ = fs::remove_file(&temp_path);
            return Err(error);
        }

        let metadata = fs::symlink_metadata(&final_path)?;
        if !metadata.file_type().is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != 0o600
        {
            let _ = fs::remove_file(&final_path);
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Published runtime manifest failed identity validation",
            ));
        }

        eprintln!(
            "[RuntimeManifest] Published instance manifest: {}",
            final_path.display()
        );

        Ok(Self {
            path: final_path,
            dev: metadata.dev(),
            ino: metadata.ino(),
        })
    }
}

impl Drop for RuntimeManifest {
    fn drop(&mut self) {
        if let Ok(metadata) = fs::symlink_metadata(&self.path) {
            if metadata.file_type().is_file()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.dev() == self.dev
                && metadata.ino() == self.ino
            {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}
