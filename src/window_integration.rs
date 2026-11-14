use crate::window_manager::{self, ClientRegistration, RegistryError, WindowManagerHandle};
use std::env;
use std::io;
use std::path::{Path, PathBuf};

const EXTERNAL_ENV: &str = "JVM_COMBINER_WINDOW_EXTERNAL";
const SOCKET_ENV: &str = "JVM_COMBINER_WINDOW_SOCKET";
const SOCKET_PROPERTY: &str = "jvm.combiner.window.socket";
const BRIDGE_JAR: &str = "window_bridge/build/window-bridge-agent.jar";

pub struct WindowRuntime {
    // 保留 handle，确保本地 Window Manager 不会提前退出。
    manager: Option<WindowManagerHandle>,
    socket_path: PathBuf,
    bridge_jar: PathBuf,
}

impl WindowRuntime {
    /// Fusion 父进程使用：无条件启动并拥有一个本地 Window Manager。
    pub fn start_local(project_root: &Path) -> io::Result<Self> {
        let socket_path = window_manager::default_socket_path()?;
        let manager = window_manager::start(&socket_path)?;
        let socket_path = manager.socket_path().to_path_buf();

        let runtime = Self {
            manager: Some(manager),
            socket_path,
            bridge_jar: Self::resolve_bridge_jar(project_root)?,
        };

        eprintln!(
            "[WindowIntegration] Local Manager listening at {}",
            runtime.socket_path.display()
        );
        Ok(runtime)
    }

    /// 独立启动模式使用：连接外部 Manager，或启动本地 Manager。
    pub fn start_or_connect(project_root: &Path) -> io::Result<Self> {
        let external = env::var(EXTERNAL_ENV)
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes"
                )
            })
            .unwrap_or(false);

        let socket_path = if external {
            let configured = env::var_os(SOCKET_ENV).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{SOCKET_ENV} must name the external Manager Unix socket"),
                )
            })?;
            let configured = PathBuf::from(configured);

            if !configured.is_absolute() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{SOCKET_ENV} must be an absolute socket path"),
                ));
            }

            configured
        } else {
            window_manager::default_socket_path()?
        };

        let manager = if external {
            eprintln!(
                "[WindowIntegration] Connecting to external Manager at {}",
                socket_path.display()
            );
            None
        } else {
            let manager = window_manager::start(&socket_path)?;
            eprintln!(
                "[WindowIntegration] Local Manager listening at {}",
                socket_path.display()
            );
            Some(manager)
        };

        Ok(Self {
            manager,
            socket_path,
            bridge_jar: Self::resolve_bridge_jar(project_root)?,
        })
    }

    fn resolve_bridge_jar(project_root: &Path) -> io::Result<PathBuf> {
        let path = project_root.join(BRIDGE_JAR);

        if !path.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "Window Bridge JAR not found: {}. Build it with window_bridge/build.sh",
                    path.display()
                ),
            ));
        }

        Ok(path)
    }

    /// Return registrations owned by this process's local Manager.
    /// External clients have no access to the parent's in-memory registry.
    /// Returns Some(registrations) for a locally owned Manager.
    /// Returns None when this process is an external Manager client.
    /// Registry failures are propagated instead of being treated as empty.
    pub fn registrations(&self) -> Result<Option<Vec<ClientRegistration>>, RegistryError> {
        self.manager
            .as_ref()
            .map(WindowManagerHandle::registrations)
            .transpose()
    }

    pub fn socket_path_string(&self) -> String {
        self.socket_path.to_string_lossy().into_owned()
    }

    pub fn java_property_arg(&self) -> String {
        format!("-D{SOCKET_PROPERTY}={}", self.socket_path.to_string_lossy())
    }

    pub fn bridge_agent_arg(&self, loader: &str) -> io::Result<String> {
        let loader = loader.to_ascii_lowercase();

        if !matches!(loader.as_str(), "fabric" | "forge" | "neoforge") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Unsupported Window Bridge loader: {loader}"),
            ));
        }

        Ok(format!("-javaagent:{}={loader}", self.bridge_jar.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn bridge_agent_argument_uses_loader_name() {
        let root = env::temp_dir().join(format!(
            "window-integration-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let bridge_dir = root.join("window_bridge/build");
        fs::create_dir_all(&bridge_dir).unwrap();
        fs::write(bridge_dir.join("window-bridge-agent.jar"), b"test").unwrap();

        let runtime = WindowRuntime {
            manager: None,
            socket_path: PathBuf::from("/tmp/window-manager.sock"),
            bridge_jar: root.join(BRIDGE_JAR),
        };

        assert!(
            runtime
                .bridge_agent_arg("fabric")
                .unwrap()
                .ends_with("=fabric")
        );
        assert!(
            runtime
                .bridge_agent_arg("Forge")
                .unwrap()
                .ends_with("=forge")
        );
        assert!(
            runtime
                .bridge_agent_arg("neoforge")
                .unwrap()
                .ends_with("=neoforge")
        );
        assert!(runtime.bridge_agent_arg("unknown").is_err());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn external_runtime_reports_registry_as_unavailable_not_empty() {
        let runtime = WindowRuntime {
            manager: None,
            socket_path: PathBuf::from("/tmp/window-manager.sock"),
            bridge_jar: PathBuf::new(),
        };

        assert_eq!(runtime.registrations().unwrap(), None);
    }

    #[test]
    fn java_property_contains_manager_socket_path() {
        let runtime = WindowRuntime {
            manager: None,
            socket_path: PathBuf::from("/tmp/jvm-combiner-501/wm.sock"),
            bridge_jar: PathBuf::new(),
        };

        assert_eq!(
            runtime.java_property_arg(),
            "-Djvm.combiner.window.socket=/tmp/jvm-combiner-501/wm.sock"
        );
    }
}
