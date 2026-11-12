use crate::window_manager::{self, ClientRegistration, WindowManagerHandle};
use std::env;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

const EXTERNAL_ENV: &str = "JVM_COMBINER_WINDOW_EXTERNAL";
const ADDRESS_ENV: &str = "JVM_COMBINER_WINDOW_ADDR";
const ADDRESS_PROPERTY: &str = "jvm.combiner.window.address";
const BRIDGE_JAR: &str = "window_bridge/build/window-bridge-agent.jar";

pub struct WindowRuntime {
    // 保留 handle，确保本地 Window Manager 不会提前退出。
    manager: Option<WindowManagerHandle>,
    address: SocketAddr,
    bridge_jar: PathBuf,
}

impl WindowRuntime {
    /// Fusion 父进程使用：无条件启动并拥有一个本地 Window Manager。
    pub fn start_local(project_root: &Path) -> io::Result<Self> {
        let manager = window_manager::start(window_manager::default_bind_addr())?;
        let address = manager.local_addr();

        let runtime = Self {
            manager: Some(manager),
            address,
            bridge_jar: Self::resolve_bridge_jar(project_root)?,
        };

        eprintln!("[WindowIntegration] Local Manager listening at {}", address);
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

        let address = if external {
            let configured = env::var(ADDRESS_ENV)
                .unwrap_or_else(|_| window_manager::DEFAULT_BIND_ADDR.to_owned());

            configured.parse::<SocketAddr>().map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Invalid {ADDRESS_ENV} value '{configured}': {error}"),
                )
            })?
        } else {
            window_manager::default_bind_addr()
        };

        let manager = if external {
            eprintln!("[WindowIntegration] Connecting to external Manager at {address}");
            None
        } else {
            let manager = window_manager::start(address)?;
            let actual_address = manager.local_addr();
            eprintln!("[WindowIntegration] Local Manager listening at {actual_address}");
            return Ok(Self {
                manager: Some(manager),
                address: actual_address,
                bridge_jar: Self::resolve_bridge_jar(project_root)?,
            });
        };

        Ok(Self {
            manager,
            address,
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
    pub fn registrations(&self) -> Vec<ClientRegistration> {
        self.manager
            .as_ref()
            .map_or_else(Vec::new, WindowManagerHandle::registrations)
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn address_string(&self) -> String {
        self.address.to_string()
    }

    pub fn java_property_arg(&self) -> String {
        format!("-D{ADDRESS_PROPERTY}={}", self.address)
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
            address: "127.0.0.1:25566".parse().unwrap(),
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
    fn java_property_contains_actual_manager_address() {
        let runtime = WindowRuntime {
            manager: None,
            address: "127.0.0.1:34567".parse().unwrap(),
            bridge_jar: PathBuf::new(),
        };

        assert_eq!(
            runtime.java_property_arg(),
            "-Djvm.combiner.window.address=127.0.0.1:34567"
        );
    }
}
