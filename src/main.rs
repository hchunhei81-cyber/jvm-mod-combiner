use jni::jni_sig;
use jni::jni_str;
use jni::objects::{JClass, JObjectArray, JString, JValue};
use jni::{InitArgsBuilder, JNIVersion, JavaVM};
use serde_json::Value;
use std::env;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::thread;

fn rules_allow(lib: &Value) -> bool {
    let rules = match lib.get("rules").and_then(|r| r.as_array()) {
        Some(r) => r,
        None => return true,
    };
    let mut allow = false;
    for rule in rules {
        let action = rule
            .get("action")
            .and_then(|a| a.as_str())
            .unwrap_or("allow");
        let os_name = rule.pointer("/os/name").and_then(|v| v.as_str());
        let os_arch = rule.pointer("/os/arch").and_then(|v| v.as_str());
        let matches = (os_name.is_none() || os_name == Some("osx"))
            && (os_arch.is_none() || os_arch == Some("arm64"));
        if matches {
            allow = action == "allow";
        }
    }
    allow
}

fn find_local(rel_path: &str, search_dirs: &[PathBuf]) -> Option<PathBuf> {
    let basename = rel_path.rsplit('/').next().unwrap_or(rel_path);
    for dir in search_dirs {
        let full = dir.join(rel_path);
        if full.exists() {
            return Some(full);
        }
        let flat = dir.join(basename);
        if flat.exists() {
            return Some(flat);
        }
    }
    None
}

fn resolve_classpath(json_path: &Path, search_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let json_str = std::fs::read_to_string(json_path).expect("Failed to read version JSON");
    let json: Value = serde_json::from_str(&json_str).expect("Failed to parse version JSON");
    let empty: Vec<Value> = Vec::new();
    let libraries = json["libraries"].as_array().unwrap_or(&empty);
    let mut classpath: Vec<PathBuf> = Vec::new();
    let mut total = 0usize;
    for lib in libraries {
        if !rules_allow(lib) {
            continue;
        }
        total += 1;
        if let Some(art) = lib.pointer("/downloads/artifact") {
            if let Some(path) = art.get("path").and_then(|v| v.as_str()) {
                if let Some(found) = find_local(path, search_dirs) {
                    classpath.push(found);
                }
            }
        }
        if let Some(classifiers) = lib.pointer("/downloads/classifiers") {
            for key in ["natives-macos-arm64", "natives-macos"] {
                if let Some(c) = classifiers.get(key) {
                    if let Some(path) = c.get("path").and_then(|v| v.as_str()) {
                        if let Some(found) = find_local(path, search_dirs) {
                            classpath.push(found);
                        }
                    }
                }
            }
        }
    }
    println!(
        "[Rust] Resolved {}/{} libraries from {}",
        classpath.len(),
        total,
        json_path.display()
    );
    classpath
}

fn find_jars_in(dir: &Path, keyword: &str) -> Vec<PathBuf> {
    let mut result = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                result.extend(find_jars_in(&path, keyword));
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.contains(keyword) && name.ends_with(".jar") {
                    result.push(path);
                }
            }
        }
    }
    result
}

// Validate a JAR as a real ZIP archive before putting it on any JVM path.
// macOS provides /usr/bin/unzip. Original files are never modified.
fn jar_is_valid(path: &Path) -> bool {
    if !path.is_file() {
        eprintln!("[JAR-FILTER][REJECT] {} -- file missing", path.display());
        return false;
    }

    let output = std::process::Command::new("/usr/bin/unzip")
        .arg("-tqq")
        .arg(path)
        .output();

    match output {
        Ok(result) if result.status.success() => true,
        Ok(result) => {
            let detail = String::from_utf8_lossy(&result.stderr);
            eprintln!(
                "[JAR-FILTER][REJECT] {} -- invalid ZIP (status: {}, {})",
                path.display(),
                result.status,
                detail.trim()
            );
            false
        }
        Err(e) => {
            eprintln!(
                "[JAR-FILTER][REJECT] {} -- cannot execute unzip: {}",
                path.display(),
                e
            );
            false
        }
    }
}

// Reject known incompatible Forge/NeoForge generations.
// Shared libraries are not rejected merely because they have old versions;
// the Forge 1.21.4 version JSON remains the dependency source of truth.
fn incompatible_forge_dependency(path: &Path) -> bool {
    let value = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();

    value.contains("/net/neoforged/")
        || value.contains("/net/minecraftforge/fmlcore/1.16.5-")
        || value.contains("/net/minecraftforge/fmlloader/1.16.5-")
        || value.contains("/net/minecraftforge/fmlloader/36.2.")
        || value.contains("/net/minecraftforge/forge/1.16.5-")
        || value.contains("/net/minecraftforge/forge/36.2.")
}

fn accept_forge_jar(path: &Path) -> bool {
    if incompatible_forge_dependency(path) {
        eprintln!(
            "[JAR-FILTER][REJECT] {} -- incompatible Forge/NeoForge version",
            path.display()
        );
        return false;
    }

    jar_is_valid(path)
}

fn set_property(env: &mut jni::Env, key: &str, value: &str) {
    let system_class = env.find_class(jni_str!("java/lang/System")).unwrap();
    let key_str = env.new_string(key).unwrap();
    let value_str = env.new_string(value).unwrap();
    env.call_static_method(
        system_class,
        jni_str!("setProperty"),
        jni_sig!((java.lang.String, java.lang.String) -> java.lang.String),
        &[JValue::Object(&key_str), JValue::Object(&value_str)],
    )
    .unwrap();
}

fn build_classpath(is_forge: bool, sandbox_dir: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mc_root =
        PathBuf::from(env::var("HOME").unwrap()).join("Library/Application Support/minecraft");
    let mut legacy_cp: Vec<PathBuf> = Vec::new();
    let mut bootstrap_cp: Vec<PathBuf> = Vec::new();

    if is_forge {
        let _forge_env = sandbox_dir.join("forge_env");
        let v_json = mc_root.join("versions/1.21.4-forge-54.1.22/1.21.4-forge-54.1.22.json");
        let dirs = vec![
            sandbox_dir.to_path_buf(),
            mc_root.join("libraries"),
            mc_root.join("libraries"),
        ];

        // Minimal bootstrap classpath: only what Bootstrap itself needs to start
        for p in [
            mc_root.join("libraries/net/minecraftforge/bootstrap/2.1.8/bootstrap-2.1.8.jar"),
            mc_root.join("libraries/net/minecraftforge/bootstrap-api/2.1.8/bootstrap-api-2.1.8.jar"),
            mc_root.join("libraries/net/minecraftforge/forge/1.21.4-54.1.22/forge-1.21.4-54.1.22-shim.jar"),
            mc_root.join("libraries/net/minecraftforge/forge/1.21.4-54.1.22/forge-1.21.4-54.1.22-client.jar"),
            mc_root.join("libraries/net/minecraftforge/forge/1.21.4-54.1.22/forge-1.21.4-54.1.22-universal.jar"),

            mc_root.join("libraries/net/minecraftforge/securemodules/2.2.24/securemodules-2.2.24.jar"),
            mc_root.join("libraries/net/minecraftforge/unsafe/0.9.2/unsafe-0.9.2.jar"),
            mc_root.join("libraries/org/ow2/asm/asm/9.9.1/asm-9.9.1.jar"),
            mc_root.join("libraries/org/ow2/asm/asm-tree/9.9.1/asm-tree-9.9.1.jar"),
            mc_root.join("libraries/org/ow2/asm/asm-analysis/9.9.1/asm-analysis-9.9.1.jar"),
            mc_root.join("libraries/org/ow2/asm/asm-commons/9.9.1/asm-commons-9.9.1.jar"),
            mc_root.join("libraries/org/ow2/asm/asm-util/9.9.1/asm-util-9.9.1.jar"),
        ] {
            if p.exists() { bootstrap_cp.push(p); }
        }
        bootstrap_cp.push(sandbox_dir.join("minecraft-client.jar"));

        // Legacy classpath: everything else, Bootstrap will build the module layer from here
        for p in find_jars_in(&mc_root.join("libraries"), "") {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            // 方案三：bootstrap_cp 已加载 securejarhandler/securemodules，
            // legacyClassPath 必须排除它们，避免模块层重复定义 UnionFileSystemProvider
            if name == "bootstrap-2.1.8.jar"
                || name.contains("bootstraplauncher")
                || name.contains("securejarhandler")
                || name.contains("securemodules")
            {
                continue;
            }
            if !legacy_cp.contains(&p) {
                legacy_cp.push(p);
            }
        }
        for p in resolve_classpath(&v_json, &dirs) {
            if !legacy_cp.contains(&p) {
                legacy_cp.push(p.clone());
            }
        }
        legacy_cp.push(sandbox_dir.join("minecraft-client.jar"));
    } else {
        for name in [
            "fabric-loader.jar",
            "asm-9.6.jar",
            "asm-tree-9.6.jar",
            "asm-analysis-9.6.jar",
            "asm-commons-9.6.jar",
            "asm-util-9.6.jar",
            "sponge-mixin.jar",
        ] {
            let p = sandbox_dir.join(name);
            legacy_cp.push(p.clone());
            bootstrap_cp.push(p);
        }
        let v_json = sandbox_dir.join("1.21.4.json");
        let dirs = vec![
            sandbox_dir.to_path_buf(),
            sandbox_dir.join("mc_libraries"),
            mc_root.join("libraries"),
        ];
        for p in resolve_classpath(&v_json, &dirs) {
            if !legacy_cp.contains(&p) {
                legacy_cp.push(p.clone());
            }
            if !bootstrap_cp.contains(&p) {
                bootstrap_cp.push(p);
            }
        }
        let mc_jar = sandbox_dir.join("minecraft-client.jar");
        legacy_cp.push(mc_jar.clone());
        bootstrap_cp.push(mc_jar);
    }

    (bootstrap_cp, legacy_cp)
}

#[allow(dead_code)]
fn set_manifest_attributes(section: &mut Vec<String>, updates: &[(&str, &str)]) {
    for (key, value) in updates {
        let prefix = format!("{}:", key);
        let mut rebuilt: Vec<String> = Vec::with_capacity(section.len() + 1);
        let mut inserted = false;
        let mut i = 0usize;
        while i < section.len() {
            if section[i].starts_with(&prefix) {
                // Replace the first occurrence and remove duplicates. Also discard any
                // folded continuation lines belonging to the old attribute.
                if !inserted {
                    rebuilt.push(format!("{}: {}", key, value));
                    inserted = true;
                }
                i += 1;
                while i < section.len() && section[i].starts_with(' ') {
                    i += 1;
                }
            } else {
                rebuilt.push(section[i].clone());
                i += 1;
            }
        }
        if !inserted {
            rebuilt.push(format!("{}: {}", key, value));
        }
        *section = rebuilt;
    }
}

#[allow(dead_code)]
fn patch_modlauncher_manifest(content: &str) -> Vec<u8> {
    // Normalize only for parsing; output is written with the CRLF separators required
    // by the JAR manifest format. Preserve unrelated headers and package sections.
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let mut sections: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for line in normalized.lines() {
        if line.is_empty() {
            if !current.is_empty() {
                sections.push(std::mem::take(&mut current));
            }
        } else {
            current.push(line.to_string());
        }
    }
    if !current.is_empty() {
        sections.push(current);
    }
    if sections.is_empty() {
        sections.push(vec!["Manifest-Version: 1.0".to_string()]);
    }

    set_manifest_attributes(
        &mut sections[0],
        &[
            ("Implementation-Version", "10.2.4"),
            ("Specification-Version", "10.2.4"),
            ("Automatic-Module-Name", "cpw.mods.modlauncher"),
        ],
    );

    // This is the package used by IEnvironment.class.getPackage() in ModLauncher.
    // Ensure both version attributes exist in its own manifest section, which avoids
    // a null value reaching ConcurrentHashMap.putIfAbsent during Launcher construction.
    let package_section = "Name: cpw/mods/modlauncher/api/";
    let found = sections
        .iter_mut()
        .skip(1)
        .find(|section| section.iter().any(|line| line.as_str() == package_section));
    if let Some(section) = found {
        set_manifest_attributes(
            section,
            &[
                ("Implementation-Version", "10.2.4"),
                ("Specification-Version", "10.2.4"),
            ],
        );
    } else {
        sections.push(vec![
            package_section.to_string(),
            "Implementation-Version: 10.2.4".to_string(),
            "Specification-Version: 10.2.4".to_string(),
        ]);
    }

    let mut output = sections
        .iter()
        .map(|section| section.join("\r\n"))
        .collect::<Vec<_>>()
        .join("\r\n\r\n");
    output.push_str("\r\n\r\n");
    output.into_bytes()
}

fn sanitize_jar(_src: &Path, _cache_dir: &Path) -> Option<PathBuf> {
    // Diagnostic safety: use official JARs unchanged.
    // Return None so existing callers fall back to the source path.
    None
}

fn launch_forge_subprocess(sandbox_dir: &Path) {
    let forge_env = sandbox_dir.join("forge_env");
    let mods_dir = forge_env.join("mods");
    std::fs::create_dir_all(&mods_dir)
        .unwrap_or_else(|e| eprintln!("[forge] 无法创建 mods 目录: {}", e));
    eprintln!("[forge] mods 目录: {}", mods_dir.display());

    let _forge_env = sandbox_dir.join("forge_env");
    let mc_root = PathBuf::from(env::var("HOME").expect("HOME is not set"))
        .join("Library/Application Support/minecraft");
    let libs = mc_root.join("libraries");

    // Forge 1.21.4: resolve dependencies from its version JSON only.
    // Never recursively scan the user's entire Minecraft libraries directory.
    let mut legacy: Vec<String> = Vec::new();

    let forge_json = mc_root.join("versions/1.21.4-forge-54.1.22/1.21.4-forge-54.1.22.json");

    if !forge_json.is_file() {
        eprintln!(
            "[JAR-FILTER][FATAL] Forge version JSON missing: {}",
            forge_json.display()
        );
        return;
    }

    let search_dirs = vec![libs.clone(), sandbox_dir.to_path_buf()];

    for path in resolve_classpath(&forge_json, &search_dirs) {
        if !accept_forge_jar(&path) {
            continue;
        }

        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        // These are supplied separately or loaded through the module path.
        if name.starts_with("bootstrap-")
            || name.starts_with("bootstrap-api-")
            || name.contains("bootstraplauncher")
            || name.contains("securejarhandler")
            || name.contains("securemodules")
            || name.contains("modlauncher")
            || name.contains("fmlloader")
            || name.contains("forgespi")
            || name.contains("mergetool-api")
            || name.contains("unsafe")
            || name.contains("terminalconsoleappender")
            || name.contains("jline")
            || name.contains("log4j")
        {
            continue;
        }

        let value = path.to_string_lossy().to_string();
        if !legacy.contains(&value) {
            legacy.push(value);
        }
    }

    eprintln!(
        "[JAR-FILTER] Forge JSON dependencies accepted for legacy classpath: {}",
        legacy.len()
    );

    // Keep ModLauncher off the JVM's initial --module-path. Bootstrap is responsible for
    // loading it from legacyClassPath into its managed module layer, where package version
    // metadata is available. Directly resolving it as a boot-layer module caused the NPE.
    let modlauncher_path =
        libs.join("net/minecraftforge/modlauncher/10.2.4/modlauncher-10.2.4.jar");
    let patched_ml_path = sandbox_dir.join(".jar_cache/modlauncher-10.2.4.jar");
    if patched_ml_path.is_file() {
        eprintln!(
            "[forge] ModLauncher 使用补丁版: {}",
            patched_ml_path.display()
        );
    } else if modlauncher_path.is_file() {
        eprintln!(
            "[forge] WARN: 补丁版缺失，回退原始: {}",
            modlauncher_path.display()
        );
    } else {
        eprintln!("[forge] ERROR: ModLauncher JAR not found");
    }

    let cache_dir = sandbox_dir.join(".jar_cache");
    fs::create_dir_all(&cache_dir).ok();
    let mut sanitized_all: Vec<String> = Vec::new();
    for path_str in legacy.drain(..) {
        let path = PathBuf::from(&path_str);
        match sanitize_jar(&path, &cache_dir) {
            Some(new_path) => {
                eprintln!("[forge] sanitized {}", new_path.display());
                sanitized_all.push(new_path.to_string_lossy().to_string());
            }
            None => sanitized_all.push(path_str),
        }
    }
    legacy = sanitized_all;

    // 物理剔除 legacy 中的 mergetool-api，防止模块重名
    // Remove auto-discovered module-path duplicates; the official ModLauncher JAR is added back once below.
    // 物理剔除 legacy 中的 fmlloader，防止模块双重读取
    // 彻底从 legacy 剔除所有已在 module path 上的核心模块，防止模块双重读取
    legacy.retain(|p| {
        !p.contains("modlauncher")
            && !p.contains("fmlloader")
            && !p.contains("mergetool-api")
            && !p.contains("forgespi")
            && !p.contains("securemodules")
            && !p.contains("securejarhandler")
            && !p.contains("bootstraplauncher")
            && !p.contains("/bootstrap/2.1.8/")
            && !p.contains("/bootstrap-api/2.1.8/")
            && !p.contains("/securemodules/")
            && !p.contains("/modlauncher/")
            && !p.contains("/unsafe/")
            && !p.contains("/JarJar")
            && !p.contains("/asm/")
            && !p.contains("bootstrap-api")
            && !p.contains("log4j")
            && !p.contains("terminalconsoleappender")
            && !p.contains("jline")
    });
    // The ordinary walk above also finds a cached/escaped duplicate named like
    // "cpw\.mods.modlauncher-...jar". The retain() removes all ModLauncher paths;
    // add back exactly the official dependency once, and let Bootstrap create its module.
    let mut _final_modlauncher = modlauncher_path.to_string_lossy().to_string();
    if modlauncher_path.is_file() {
        if let Some(sanitized_ml) = sanitize_jar(&modlauncher_path, &cache_dir) {
            eprintln!(
                "[forge] 使用已補丁的 ModLauncher: {}",
                sanitized_ml.display()
            );
            _final_modlauncher = sanitized_ml.to_string_lossy().to_string();
        } else {
            eprintln!("[forge] 使用原始 ModLauncher");
        }
        // legacy.push(_final_modlauncher); // 已停用：防止雙重載入衝突
    }

    // Bootstrap 2.1.8 builds its input list from `java.class.path` via
    // findAllClassPathEntries(). The `legacyClassPath` property alone is not
    // sufficient for this version, so ensure Forge's client JAR is present and
    // pass the complete legacy list to the JVM's actual -cp argument below.

    // ===== 只做一次：把 Bootstrap 核心 JAR 插入 legacy 顶部 =====
    let bootstrap_jar = libs.join("net/minecraftforge/bootstrap/2.1.8/bootstrap-2.1.8.jar");
    let bootstrap_api_jar =
        libs.join("net/minecraftforge/bootstrap-api/2.1.8/bootstrap-api-2.1.8.jar");
    let forge_client =
        libs.join("net/minecraftforge/forge/1.21.4-54.1.22/forge-1.21.4-54.1.22-client.jar");

    for jar in [&bootstrap_jar, &bootstrap_api_jar, &forge_client] {
        if jar.is_file() {
            let s = jar.to_string_lossy().to_string();
            if !legacy.contains(&s) {
                legacy.insert(0, s);
            }
        }
    }

    legacy.retain(|p| {
        !p.contains("bootstrap-api")
            && !(p.contains("/sandbox/mc_libraries/")
                && p.to_ascii_lowercase().contains("failureaccess-"))
    });

    // Bootstrap API stays on the initial module path; do not expose it
    // again to Bootstrap's managed-layer classpath scanner.
    legacy.retain(|p| {
        !p.contains("bootstrap-api")
            && !p.contains("/bootstrap/2.1.8/")
            // Keep the newer official failureaccess module only.
            // Do not let sandbox copies enter Bootstrap's module layer.
            && !(p.contains("/sandbox/mc_libraries/")
                && p.to_ascii_lowercase().contains("failureaccess-"))
    });
    // Use the official ModLauncher JAR. sanitize_jar is intentionally disabled.
    // Keep exactly one official ModLauncher entry on the legacy classpath.
    if modlauncher_path.is_file() {
        legacy.retain(|p| !p.contains("/modlauncher/"));
        let official_ml = modlauncher_path.to_string_lossy().to_string();
        legacy.insert(0, official_ml.clone());
        eprintln!(
            "[forge] legacyClassPath 使用官方 ModLauncher: {}",
            official_ml
        );
    } else {
        eprintln!(
            "[forge] ERROR: 官方 ModLauncher 不存在: {}",
            modlauncher_path.display()
        );
    }

    legacy.retain(|p| !p.contains("/bootstrap/2.1.7/"));

    // Final integrity gate: no invalid or incompatible archive can reach Bootstrap.
    legacy.retain(|value| accept_forge_jar(Path::new(value)));

    let legacy_cp = legacy.join(":");

    // JVM module path contains the bootstrap/runtime modules. ModLauncher itself is deliberately
    // supplied through legacyClassPath so Bootstrap can place it in its managed module layer.
    let _modlauncher_module_pb =
        if let Some(sanitized_ml) = sanitize_jar(&modlauncher_path, &cache_dir) {
            sanitized_ml
        } else {
            modlauncher_path.clone()
        };

    let _module_jars: Vec<String> = {
        let paths = vec![
            if let Some(sanitized_ml) = sanitize_jar(&modlauncher_path, &cache_dir) {
                sanitized_ml
            } else {
                modlauncher_path.clone()
            },
            libs.join("net/minecraftforge/securemodules/2.2.24/securemodules-2.2.24.jar"),
            libs.join("net/minecraftforge/forgespi/7.1.5/forgespi-7.1.5.jar"),
            libs.join("net/minecraftforge/mergetool-api/1.0/mergetool-api-1.0.jar"),
            // 自動補上 log4j-slf4j 實現，解決 SLF4J(W): No SLF4J providers were found.
            // paths.push(libs.join("org/apache/logging/log4j/log4j-slf4j2-impl/2.25.2/log4j-slf4j2-impl-2.25.2.jar")); // 已暫時註解以排查巨集錯誤
            libs.join("org/apache/logging/log4j/log4j-core/2.25.2/log4j-core-2.25.2.jar"),
            libs.join("org/apache/logging/log4j/log4j-api/2.25.2/log4j-api-2.25.2.jar"),
            libs.join(
                "net/minecrell/terminalconsoleappender/1.2.0/terminalconsoleappender-1.2.0.jar",
            ),
            libs.join("org/jline/jline-terminal/3.25.1/jline-terminal-3.25.1.jar"),
            libs.join("org/jline/jline-reader/3.25.1/jline-reader-3.25.1.jar"),
            libs.join("net/minecraftforge/fmlloader/1.21.4-54.1.22/fmlloader-1.21.4-54.1.22.jar"),
            libs.join("net/minecraftforge/bootstrap/2.1.8/bootstrap-2.1.8.jar"),
            libs.join("net/minecraftforge/bootstrap-api/2.1.8/bootstrap-api-2.1.8.jar"),
            sandbox_dir.join("mc_libraries/logging-1.7.12.jar"),
            sandbox_dir.join("mc_libraries/slf4j-api-2.0.17.jar"),
            libs.join("net/minecraftforge/unsafe/0.9.2/unsafe-0.9.2.jar"),
            libs.join("org/ow2/asm/asm/9.9.1/asm-9.9.1.jar"),
            libs.join("org/ow2/asm/asm-tree/9.9.1/asm-tree-9.9.1.jar"),
            libs.join("org/ow2/asm/asm-analysis/9.9.1/asm-analysis-9.9.1.jar"),
            libs.join("org/ow2/asm/asm-commons/9.9.1/asm-commons-9.9.1.jar"),
            libs.join("org/ow2/asm/asm-util/9.9.1/asm-util-9.9.1.jar"),
            libs.join("net/sf/jopt-simple/jopt-simple/5.0.4/jopt-simple-5.0.4.jar"),
        ];

        // 過濾不存在的路徑並轉為 String
        let mut strings: Vec<String> = Vec::new();
        let mut seen_ml = false;
        for p in paths {
            if p.exists() {
                let s = p.to_string_lossy().to_string();
                let file_name = p
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default();

                // 絕對防線：嚴格禁止任何非快取或多餘的 modlauncher 進入模組路徑
                if file_name.contains("modlauncher") || file_name.contains("cpw.mods.modlauncher") {
                    if s.contains(".jar_cache") && file_name == "modlauncher-10.2.4.jar" && !seen_ml
                    {
                        seen_ml = true;
                        strings.push(s);
                    }
                } else {
                    strings.push(s.clone());
                }
            } else {
                eprintln!("[forge] module path missing: {}", p.display());
            }
        }
        for m in &strings {
            if m.contains("modlauncher") {
                println!(
                    "[forge-debug] 最終加入 module-path 的 modlauncher 路徑: {}",
                    m
                );
            }
        }
        strings
    };
    let module_path = {
        let mut jars = Vec::new();
        let core_rel_paths = [
            "net/sf/jopt-simple/jopt-simple/5.0.4/jopt-simple-5.0.4.jar",
            // "net/minecraftforge/modlauncher/10.2.4/modlauncher-10.2.4.jar", // 用 .jar_cache 补丁版
            "net/minecraftforge/securemodules/2.2.24/securemodules-2.2.24.jar",
            "net/minecraftforge/bootstrap-api/2.1.8/bootstrap-api-2.1.8.jar",
            "net/minecraftforge/unsafe/0.9.2/unsafe-0.9.2.jar",
            "org/ow2/asm/asm/9.9.1/asm-9.9.1.jar",
            "org/ow2/asm/asm-tree/9.9.1/asm-tree-9.9.1.jar",
            "org/ow2/asm/asm-analysis/9.9.1/asm-analysis-9.9.1.jar",
            "org/ow2/asm/asm-commons/9.9.1/asm-commons-9.9.1.jar",
            "org/ow2/asm/asm-util/9.9.1/asm-util-9.9.1.jar",
            "org/apache/logging/log4j/log4j-core/2.25.2/log4j-core-2.25.2.jar",
            "org/apache/logging/log4j/log4j-api/2.25.2/log4j-api-2.25.2.jar",
            "net/minecrell/terminalconsoleappender/1.2.0/terminalconsoleappender-1.2.0.jar",
            "org/jline/jline-terminal/3.25.1/jline-terminal-3.25.1.jar",
            "org/jline/jline-reader/3.25.1/jline-reader-3.25.1.jar",
        ];
        for rel in core_rel_paths {
            let path = libs.join(rel);
            if !accept_forge_jar(&path) {
                continue;
            }
            // ModLauncher 用终端 python3 生成的补丁 JAR，避免 getPackage() 版本为 null 的 NPE
            let effective = if rel.contains("modlauncher/") {
                let patched = sandbox_dir.join(".jar_cache/modlauncher-10.2.4.jar");
                if patched.is_file() {
                    eprintln!(
                        "[forge] module-path ModLauncher 使用补丁版: {}",
                        patched.display()
                    );
                    patched
                } else {
                    eprintln!(
                        "[forge] WARN: 补丁 JAR 不存在，回退官方版: {}",
                        path.display()
                    );
                    path
                }
            } else {
                path
            };
            let value = effective.to_string_lossy().to_string();
            if !jars.contains(&value) {
                jars.push(value);
            }
        }
        jars.join(":")
    };

    let java = if let Ok(output) = std::process::Command::new("/usr/libexec/java_home")
        .arg("-v")
        .arg("21")
        .output()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            eprintln!("[forge] 使用自動檢測的 Java 21: {}", path);
            format!("{}/bin/java", path)
        } else {
            env::var("JAVA_HOME")
                .map(|h| format!("{}/bin/java", h))
                .unwrap_or_else(|_| "java".to_string())
        }
    } else {
        env::var("JAVA_HOME")
            .map(|h| format!("{}/bin/java", h))
            .unwrap_or_else(|_| "java".to_string())
    };

    let mut cmd = std::process::Command::new(&java);

    // Critical: Bootstrap 2.1.8 scans `java.class.path` to build its runtime
    // module layer. Pass the full legacy JAR list as the actual JVM classpath;
    // setting only -DlegacyClassPath does not make those JARs visible to that scan.

    // 【強制修正】直接建構包含 bootstrap 與所有 forge libraries 的絕對 Classpath
    // Build the actual JVM classpath from installed libraries and sandbox game files.
    let bootstrap_jar = libs.join("net/minecraftforge/bootstrap/2.1.8/bootstrap-2.1.8.jar");
    let bootstrap_api_jar =
        libs.join("net/minecraftforge/bootstrap-api/2.1.8/bootstrap-api-2.1.8.jar");
    let mut real_cp: Vec<String> = Vec::new();

    for path in [
        bootstrap_jar,
        bootstrap_api_jar,
        forge_client.clone(),
        sandbox_dir.join("minecraft-client.jar"),
    ] {
        if path.is_file() {
            let value = path.to_string_lossy().to_string();
            if !real_cp.contains(&value) {
                real_cp.push(value);
            }
        } else {
            eprintln!("[forge] classpath file missing: {}", path.display());
        }
    }

    for path in &legacy {
        let candidate = Path::new(path);
        if accept_forge_jar(candidate) && !real_cp.contains(path) {
            real_cp.push(path.clone());
        }
    }

    for path in find_jars_in(&sandbox_dir.join("mc_libraries"), "") {
        // Exclude sandbox copies of Mojang Logging; the official 1.2.7 JAR
        // is added separately and must be the only com.mojang.logging module.
        {
            let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if fname.starts_with("logging-") {
                eprintln!("[forge] 排除沙盒 logging: {}", path.display());
                continue;
            }
        }

        let value = path.to_string_lossy().to_string();

        if accept_forge_jar(&path) && !real_cp.contains(&value) {
            real_cp.push(value);
        }
    }

    // Avoid discovering Bootstrap API a second time from java.class.path.
    real_cp.retain(|p| {
        !p.contains("bootstrap-api")
            && !p.contains("jopt-simple/6.0-alpha-3/")
            // Avoid loading the old automatic module named "failureaccess".
            // Sandbox copies can introduce additional automatic modules.
            && !(p.contains("/sandbox/mc_libraries/")
                && p.to_ascii_lowercase().contains("failureaccess-"))
    });

    // ===== FORGE_RUNTIME_MODULES_PATCH =====
    // Bootstrap 2.1.8 discovers runtime modules through java.class.path.
    // These Forge service-provider JARs must therefore be present here.
    let required_forge_runtime_jars = [
        libs.join("net/minecraftforge/fmlloader/1.21.4-54.1.22/fmlloader-1.21.4-54.1.22.jar"),
        libs.join("net/minecraftforge/forgespi/7.1.5/forgespi-7.1.5.jar"),
        libs.join("net/minecraftforge/mergetool-api/1.0/mergetool-api-1.0.jar"),
    ];

    for jar in required_forge_runtime_jars {
        if !jar.is_file() {
            panic!(
                "[forge] Required Forge runtime JAR missing: {}",
                jar.display()
            );
        }

        let jar_string = jar.to_string_lossy().to_string();

        if !real_cp
            .iter()
            .any(|p| Path::new(p).canonicalize().ok() == jar.canonicalize().ok())
        {
            eprintln!("[forge] Added Bootstrap runtime module: {}", jar.display());
            real_cp.push(jar_string);
        }
    }

    // ===== MOJANG_LOGGING_PATCH =====
    // fmlloader 1.21.4 needs com.mojang.logging.LogUtils at bootstrap time.
    // Sandbox copies (logging-*.jar) are excluded to avoid conflicts,
    // but the official Mojang Logging JAR must be present on real_cp.
    {
        let candidates = [
            libs.join("com/mojang/logging/1.2.7/logging-1.2.7.jar"),
            mc_root.join("libraries/com/mojang/logging/1.2.7/logging-1.2.7.jar"),
        ];
        let mut added = false;
        for cand in candidates.iter() {
            if cand.is_file() {
                let s = cand.to_string_lossy().to_string();
                if !real_cp.contains(&s) {
                    real_cp.push(s.clone());
                }
                eprintln!("[forge] Added Mojang Logging: {}", cand.display());
                added = true;
                break;
            }
        }
        if !added {
            eprintln!(
                "[forge] ERROR: Mojang Logging JAR not found (expected com/mojang/logging/1.2.7/logging-1.2.7.jar)"
            );
        }
    }
    // ===== END MOJANG_LOGGING_PATCH =====
    // ===== JARJAR_SELECTOR_PATCH =====
    // fmlloader 1.21.4 depends on JarJarSelector for Jar-in-Jar resolution.
    {
        let jj = libs.join("net/minecraftforge/JarJarSelector/0.3.26/JarJarSelector-0.3.26.jar");
        if jj.is_file() {
            let s = jj.to_string_lossy().to_string();
            if !real_cp.contains(&s) {
                real_cp.push(s.clone());
            }
            eprintln!("[forge] Added JarJarSelector: {}", jj.display());
        } else {
            eprintln!(
                "[forge] ERROR: JarJarSelector JAR not found: {}",
                jj.display()
            );
        }
    }
    // ===== END JARJAR_SELECTOR_PATCH =====
    // ===== JARJAR_METADATA_PATCH =====
    // JarJarSelector 0.3.26 depends on JarJarMetadata for jar-in-jar metadata parsing.
    {
        let meta = libs.join("net/minecraftforge/JarJarMetadata/0.3.26/JarJarMetadata-0.3.26.jar");
        if meta.is_file() {
            let s = meta.to_string_lossy().to_string();
            if !real_cp.contains(&s) {
                real_cp.push(s.clone());
            }
            eprintln!("[forge] Added JarJarMetadata: {}", meta.display());
        } else {
            eprintln!(
                "[forge] ERROR: JarJarMetadata JAR not found: {}",
                meta.display()
            );
        }
    }
    // ===== END JARJAR_METADATA_PATCH =====
    // ===== FAILUREACCESS_PATCH =====
    // Guava's ListenableFuture internals need failureaccess at runtime.
    {
        let fa = libs.join("com/google/guava/failureaccess/1.0.1/failureaccess-1.0.1.jar");
        if fa.is_file() {
            let s = fa.to_string_lossy().to_string();
            if !real_cp.contains(&s) {
                real_cp.push(s.clone());
            }
            eprintln!("[forge] Added failureaccess: {}", fa.display());
        } else {
            eprintln!(
                "[forge] ERROR: failureaccess JAR not found: {}",
                fa.display()
            );
        }
    }
    // ===== END FAILUREACCESS_PATCH =====

    // Remove duplicate physical JAR paths while preserving order.
    let mut seen_forge_cp = std::collections::HashSet::new();
    real_cp.retain(|p| {
        let key = Path::new(p)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(p));
        seen_forge_cp.insert(key)
    });
    // ===== END FORGE_RUNTIME_MODULES_PATCH =====

    // Use the official ModLauncher JAR; sanitize_jar is disabled.
    // Do not refer to a patched JAR that is not generated.
    if accept_forge_jar(&modlauncher_path) {
        real_cp.retain(|p| !p.contains("/modlauncher/"));
        let official_ml = modlauncher_path.to_string_lossy().to_string();
        if !real_cp.contains(&official_ml) {
            real_cp.insert(0, official_ml);
        }
    } else {
        eprintln!(
            "[forge] ERROR: 官方 ModLauncher 不存在: {}",
            modlauncher_path.display()
        );
    }
    for p in real_cp.iter().filter(|p| p.contains("modlauncher")) {
        eprintln!("[forge][debug] real_cp modlauncher: {}", p);
    }
    let forced_cp_str = real_cp.join(":");
    cmd.arg("-cp").arg(&forced_cp_str);

    if forge_client.is_file() {
        eprintln!(
            "[forge] forge-client included in Bootstrap classpath: {}",
            forge_client.display()
        );
    } else {
        eprintln!(
            "[forge] ERROR: forge-client JAR missing: {}",
            forge_client.display()
        );
    }
    eprintln!("[forge] Bootstrap JVM classpath entries: {}", legacy.len());
    cmd.arg("--module-path").arg(&module_path)
        .arg("--add-modules").arg("ALL-MODULE-PATH,java.instrument")
        .arg(format!("-DlegacyClassPath={}", legacy_cp))
        .arg(format!("-DlibraryDirectory={}", libs.display()))
        .arg("-DignoreList=asm,securejarhandler,bootstrap,bootstrap-api,securemodules,unsafe,JarJar,client-1.21.4-official,modlauncher")
                                .arg("-Dforge.logging.console.level=debug")
        .arg("-Dbsl.debug=true")
        .arg("-Dmodlauncher.containsInjectables=true")
        .arg("-Dmodlauncher.naming=srg")
        .arg("-Dnet.minecraftforge.forgeVersion=54.1.22")
        .arg("-Dnet.minecraftforge.mcVersion=1.21.4")
        .arg("-Dterminal.ansi=true")
        .arg("-Dmodlauncher.version=10.2.4")
        .arg("-Dfml.version=54.1.22")
        .arg("-Dfml.mcVersion=1.21.4")
        .arg("-XX:-OmitStackTraceInFastThrow")
        .arg("--add-opens=java.base/java.util.jar=ALL-UNNAMED")
        .arg("--add-exports=java.base/sun.security.util=ALL-UNNAMED")
        .arg("--add-opens=java.base/java.lang.invoke=ALL-UNNAMED")
        .arg("--add-opens=java.base/java.lang.invoke=cpw.mods.securejarhandler")
        .arg("--add-opens=java.base/java.nio=ALL-UNNAMED")
        .arg("--add-opens=java.base/sun.nio.ch=ALL-UNNAMED")
        .arg("--add-opens=java.base/java.io=ALL-UNNAMED")
        .arg("--add-opens=java.base/java.util=ALL-UNNAMED")
        .arg("--add-opens=java.base/java.lang=ALL-UNNAMED")
        .arg("-XstartOnFirstThread")
        .arg("-Dfml.mcVersion=1.21.4")
        .arg("-Dfml.forgeVersion=54.1.22")
        .arg("-Dfml.mcpVersion=20241203.143248")
        .arg("-Dfml.forgeGroup=net.minecraftforge")
        .arg("-Dfml.ignoreInvalidMinecraftCertificates=true")
        .arg("-Dfml.ignorePatchDiscrepancies=true")
        .arg("net.minecraftforge.bootstrap.ForgeBootstrap")
        .arg("--launchTarget").arg("forge_client")
        .arg("--fml.forgeVersion").arg("54.1.22")
        .arg("--fml.mcVersion").arg("1.21.4")
        .arg("--fml.mcpVersion").arg("20241203.143248")
        .arg("--fml.forgeGroup").arg("net.minecraftforge")
        .arg("--gameDir").arg(sandbox_dir.to_string_lossy().to_string())
        .arg("--assetsDir").arg(sandbox_dir.join("assets").to_string_lossy().to_string())
        .arg("--assetIndex").arg("17")
        .arg("--version").arg("1.21.4")
        .arg("--username").arg("Dev")
        .arg("--uuid").arg("00000000000000000000000000000000")
        .arg("--accessToken").arg("0")
        .arg("--userType").arg("legacy")
        .arg("--versionType").arg("release");

    // ZIP diagnostic: print every archive passed to the JVM.
    eprintln!("\n========== ZIP DIAGNOSTIC BEGIN ==========");

    eprintln!("[ZIP-DIAG] MODULE_PATH");
    for (i, item) in module_path.split(':').enumerate() {
        eprintln!("[ZIP-DIAG][MODULE][{}] {}", i, item);
    }

    eprintln!("[ZIP-DIAG] REAL_CLASSPATH");
    for (i, item) in real_cp.iter().enumerate() {
        eprintln!("[ZIP-DIAG][CP][{}] {}", i, item);
    }

    eprintln!("[ZIP-DIAG] LEGACY_CLASSPATH");
    for (i, item) in legacy.iter().enumerate() {
        eprintln!("[ZIP-DIAG][LEGACY][{}] {}", i, item);
    }

    eprintln!("========== ZIP DIAGNOSTIC END ==========\n");

    println!("[Forge] Launching subprocess: {:?}", cmd);
    // 直接用 Stdio::inherit 把日志直接倒进终端，避免管道死锁
    cmd.stdout(std::process::Stdio::inherit());
    cmd.stderr(std::process::Stdio::inherit());

    match cmd.spawn() {
        Ok(mut child) => {
            let status = child.wait();
            println!("[Forge] Subprocess exited: {:?}", status);
        }
        Err(e) => {
            println!("[Forge] Failed to spawn: {}", e);
        }
    }
}

fn launch_neoforge_subprocess(sandbox_dir: &Path) {
    use std::process::{Command, Stdio};

    const PROFILE_ID: &str = "neoforge-21.4.161";
    const MC_VERSION: &str = "1.21.4";

    fn fail(message: &str) {
        eprintln!("[neoforge] ERROR: {}", message);
    }

    fn append_profile_args(value: &Value, output: &mut Vec<String>, allow_features: bool) {
        match value {
            Value::String(s) => output.push(s.clone()),

            Value::Array(items) => {
                for item in items {
                    append_profile_args(item, output, allow_features);
                }
            }

            Value::Object(object) => {
                if let Some(rules) = object.get("rules").and_then(Value::as_array) {
                    // 本启动器目前不启用 demo、quick-play、自定义分辨率等功能。
                    if !allow_features
                        && rules.iter().any(|r| {
                            r.get("features")
                                .and_then(Value::as_object)
                                .is_some_and(|f| !f.is_empty())
                        })
                    {
                        return;
                    }
                }

                if !rules_allow(value) {
                    return;
                }

                if let Some(inner) = object.get("value") {
                    append_profile_args(inner, output, allow_features);
                }
            }

            _ => {}
        }
    }

    fn replace_vars(input: &str, vars: &[(&str, &str)]) -> String {
        let mut output = input.to_string();
        for (key, value) in vars {
            output = output.replace(key, value);
        }
        output
    }

    let home = match env::var("HOME") {
        Ok(v) => PathBuf::from(v),
        Err(e) => {
            fail(&format!("无法读取 HOME: {}", e));
            return;
        }
    };

    let mc_root = home.join("Library/Application Support/minecraft");
    let libraries_dir = mc_root.join("libraries");

    let profile_path = mc_root
        .join("versions")
        .join(PROFILE_ID)
        .join(format!("{}.json", PROFILE_ID));

    let vanilla_path = mc_root
        .join("versions")
        .join(MC_VERSION)
        .join(format!("{}.json", MC_VERSION));

    if !profile_path.is_file() {
        fail(&format!(
            "NeoForge profile 不存在: {}",
            profile_path.display()
        ));
        return;
    }

    if !vanilla_path.is_file() {
        fail(&format!(
            "Minecraft profile 不存在: {}",
            vanilla_path.display()
        ));
        return;
    }

    let profile: Value = match fs::read_to_string(&profile_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
    {
        Some(v) => v,
        None => {
            fail("NeoForge JSON 解析失败");
            return;
        }
    };

    let vanilla: Value = match fs::read_to_string(&vanilla_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
    {
        Some(v) => v,
        None => {
            fail("Minecraft JSON 解析失败");
            return;
        }
    };

    let profile_id = profile
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(PROFILE_ID);

    let inherited = profile
        .get("inheritsFrom")
        .and_then(Value::as_str)
        .unwrap_or("");

    if inherited != MC_VERSION {
        fail(&format!(
            "版本不匹配: NeoForge inheritsFrom={}，预期 {}",
            inherited, MC_VERSION
        ));
        return;
    }

    let main_class = match profile.get("mainClass").and_then(Value::as_str) {
        Some(v) => v,
        None => {
            fail("profile 缺少 mainClass");
            return;
        }
    };

    let asset_index = match vanilla.pointer("/assetIndex/id").and_then(Value::as_str) {
        Some(v) => v,
        None => {
            fail("原版 profile 缺少 assetIndex.id");
            return;
        }
    };

    let assets_root = mc_root.join("assets");
    let natives_dir = sandbox_dir.join("natives");

    if let Err(e) = fs::create_dir_all(&natives_dir) {
        fail(&format!("创建 natives 目录失败: {}", e));
        return;
    }

    // ---------------------------------------------------------
    // 1. 收集 Minecraft + NeoForge 依赖
    // ---------------------------------------------------------

    let mut classpath: Vec<String> = Vec::new();
    let mut native_jars: Vec<PathBuf> = Vec::new();

    let mut all_libraries: Vec<&Value> = Vec::new();

    for lib in profile
        .get("libraries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        all_libraries.push(lib);
    }

    for lib in vanilla
        .get("libraries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        all_libraries.push(lib);
    }

    for lib in all_libraries {
        if !rules_allow(lib) {
            continue;
        }

        let name = lib.get("name").and_then(Value::as_str).unwrap_or("");

        // NeoForge 21.4.161 requires ASM 9.8.
        // Exclude older ASM artifacts inherited from Minecraft 1.21.4.
        if name.starts_with("org.ow2.asm:") && !name.ends_with(":9.8") {
            eprintln!("[neoforge] Skip incompatible ASM dependency: {}", name);
            continue;
        }

        let artifact_path = lib
            .pointer("/downloads/artifact/path")
            .and_then(Value::as_str);

        let is_native = name.contains(":natives-macos") || name.contains(":natives-macos-arm64");

        if is_native {
            if let Some(relative) = artifact_path {
                let path = libraries_dir.join(relative);
                if path.is_file() && !native_jars.contains(&path) {
                    native_jars.push(path);
                } else if !path.is_file() {
                    eprintln!("[neoforge] WARNING: native JAR 不存在: {}", path.display());
                }
            }
            continue;
        }

        if let Some(relative) = artifact_path {
            let path = libraries_dir.join(relative);

            if path.is_file() {
                let value = path.to_string_lossy().to_string();
                if !classpath.contains(&value) {
                    classpath.push(value);
                }
            } else {
                eprintln!("[neoforge] WARNING: dependency 缺失: {}", path.display());
            }
        }
    }

    // 2. 提取 macOS 原生库
    // ---------------------------------------------------------

    let mut extracted_count = 0usize;

    for jar in &native_jars {
        let status = Command::new("unzip")
            .arg("-oq")
            .arg(jar)
            .arg("-d")
            .arg(&natives_dir)
            .status();

        match status {
            Ok(s) if s.success() => extracted_count += 1,
            Ok(s) => {
                eprintln!(
                    "[neoforge] WARNING: native 提取失败 {}，status={}",
                    jar.display(),
                    s
                );
            }
            Err(e) => {
                fail(&format!("无法执行 unzip: {}", e));
                return;
            }
        }
    }

    // ---------------------------------------------------------
    // 3. JVM 参数：NeoForge + Vanilla
    // ---------------------------------------------------------

    let mut jvm_args: Vec<String> = Vec::new();

    append_profile_args(
        &profile.pointer("/arguments/jvm").unwrap_or(&Value::Null),
        &mut jvm_args,
        false,
    );

    append_profile_args(
        &vanilla.pointer("/arguments/jvm").unwrap_or(&Value::Null),
        &mut jvm_args,
        false,
    );

    let classpath_string = classpath.join(":");

    let vars = [
        ("${library_directory}", libraries_dir.to_str().unwrap_or("")),
        ("${classpath_separator}", ":"),
        ("${classpath}", classpath_string.as_str()),
        ("${natives_directory}", natives_dir.to_str().unwrap_or("")),
        ("${assets_root}", assets_root.to_str().unwrap_or("")),
        ("${assets_index_name}", asset_index),
        ("${version_name}", profile_id),
        ("${launcher_name}", "JVM-Mod-Combiner"),
        ("${launcher_version}", "0.1.0"),
    ];

    for arg in jvm_args.iter_mut() {
        *arg = replace_vars(arg, &vars);
    }

    // 修复 profile 中 ASM 模块路径缺少 classpath_separator 的拼接。
    for arg in jvm_args.iter_mut() {
        *arg = arg.replace(".jar${library_directory}", ".jar:${library_directory}");
    }

    if !jvm_args.iter().any(|a| a == "-XstartOnFirstThread") {
        eprintln!("[neoforge] WARNING: JVM 参数中没有 -XstartOnFirstThread");
    }

    // 检查模块路径中的文件是否存在。
    if let Some(index) = jvm_args.iter().position(|a| a == "-p") {
        if let Some(module_path) = jvm_args.get(index + 1) {
            for entry in module_path.split(':') {
                if !entry.is_empty() && !Path::new(entry).is_file() {
                    fail(&format!("模块路径 JAR 不存在: {}", entry));
                    return;
                }
            }
        } else {
            fail("-p 后缺少模块路径");
            return;
        }
    } else {
        fail("NeoForge JVM 参数缺少 -p 模块路径");
        return;
    }

    // ---------------------------------------------------------
    // 4. Game 参数：Vanilla + NeoForge
    // ---------------------------------------------------------

    let mut game_args: Vec<String> = Vec::new();

    append_profile_args(
        &vanilla.pointer("/arguments/game").unwrap_or(&Value::Null),
        &mut game_args,
        false,
    );

    append_profile_args(
        &profile.pointer("/arguments/game").unwrap_or(&Value::Null),
        &mut game_args,
        false,
    );

    let game_vars = [
        ("${auth_player_name}", "Dev"),
        ("${version_name}", profile_id),
        ("${game_directory}", sandbox_dir.to_str().unwrap_or("")),
        ("${assets_root}", assets_root.to_str().unwrap_or("")),
        ("${assets_index_name}", asset_index),
        ("${auth_uuid}", "00000000000000000000000000000000"),
        ("${auth_access_token}", "0"),
        ("${clientid}", ""),
        ("${auth_xuid}", ""),
        ("${user_type}", "legacy"),
        ("${version_type}", "release"),
    ];

    for arg in game_args.iter_mut() {
        *arg = replace_vars(arg, &game_vars);
    }

    // ---------------------------------------------------------
    // 5. Java 21
    // ---------------------------------------------------------

    let java = if let Ok(out) = Command::new("/usr/libexec/java_home")
        .arg("-v")
        .arg("21")
        .output()
    {
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();

        if !path.is_empty() {
            format!("{}/bin/java", path)
        } else {
            "java".to_string()
        }
    } else {
        "java".to_string()
    };

    // ---------------------------------------------------------
    // 6. Java Agent
    // ---------------------------------------------------------

    let project_root = match env::current_dir() {
        Ok(v) => v,
        Err(e) => {
            fail(&format!("无法获取当前目录: {}", e));
            return;
        }
    };

    let agent_jar = project_root.join("bus_test/bus-agent.jar");

    let mut cmd = Command::new(&java);
    cmd.current_dir(&mc_root);

    // 旧代码通过 JAVA_TOOL_OPTIONS 注入，可能造成重复注入。
    // 新实现直接作为 JVM 参数注入。
    cmd.env_remove("JAVA_TOOL_OPTIONS");

    for arg in &jvm_args {
        cmd.arg(arg);
    }

    if agent_jar.is_file() {
        let agent_arg = format!("-javaagent:{}=NeoForge-Sandbox", agent_jar.display());
        cmd.arg(agent_arg);
        eprintln!("[neoforge] Java Agent 已加入 JVM 参数");
    } else {
        eprintln!(
            "[neoforge] WARNING: Agent JAR 不存在: {}",
            agent_jar.display()
        );
    }

    cmd.arg(main_class);

    for arg in &game_args {
        cmd.arg(arg);
    }

    eprintln!("[neoforge] Profile: {}", profile_id);
    eprintln!("[neoforge] Minecraft: {}", MC_VERSION);
    eprintln!("[neoforge] Main class: {}", main_class);
    eprintln!("[neoforge] Classpath JAR 数量: {}", classpath.len());
    eprintln!("[neoforge] Native JAR 数量: {}", native_jars.len());
    eprintln!("[neoforge] Native 提取成功数量: {}", extracted_count);
    eprintln!("[neoforge] Asset index: {}", asset_index);
    eprintln!("[neoforge] JVM 参数数量: {}", jvm_args.len());
    eprintln!("[neoforge] Game 参数数量: {}", game_args.len());
    eprintln!("[neoforge] Java: {}", java);
    eprintln!("[neoforge] 启动 NeoForge 21.4.161...");

    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    match cmd.spawn() {
        Ok(mut child) => match child.wait() {
            Ok(status) => {
                eprintln!("[neoforge] 进程退出状态: {}", status);
            }
            Err(e) => fail(&format!("等待 NeoForge 进程失败: {}", e)),
        },
        Err(e) => fail(&format!("启动 Java 失败: {}", e)),
    }
}

fn start_data_bus() -> std::io::Result<()> {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::sync::{atomic::{AtomicU64, Ordering}, Arc, Mutex};

    // Each registration is tied to one specific TCP connection.
    struct PeerRegistration {
        connection_id: u64,
        sender: std::sync::mpsc::Sender<String>,
    }

    // name -> current connection registration
    let registry: Arc<Mutex<HashMap<String, PeerRegistration>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Monotonically increasing ID prevents an old connection from
    // deleting a newer connection registered under the same name.
    let next_connection_id = Arc::new(AtomicU64::new(1));

    let listener = TcpListener::bind("127.0.0.1:25565")?;
    println!("[DataBus] 路由总线已启动，监听 127.0.0.1:25565");

    thread::spawn(move || {
        for stream in listener.incoming() {
            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    println!("[DataBus] accept 失败: {}", e);
                    continue;
                }
            };

            let registry = registry.clone();
            let next_connection_id = next_connection_id.clone();

            thread::spawn(move || {
                let connection_id =
                    next_connection_id.fetch_add(1, Ordering::Relaxed);

                let peer = stream
                    .peer_addr()
                    .map(|a| a.to_string())
                    .unwrap_or_default();
                let reader_stream = match stream.try_clone() {
                    Ok(s) => s,
                    Err(e) => {
                        println!("[DataBus] clone 失败: {}", e);
                        return;
                    }
                };
                let mut writer = stream;
                let reader = BufReader::new(reader_stream);

                // 每个连接一个写线程 + mpsc，防止跨线程写交错
                let (tx, rx) = std::sync::mpsc::channel::<String>();
                let write_handle = thread::spawn(move || {
                    for line in rx {
                        if writer.write_all(line.as_bytes()).is_err() {
                            break;
                        }
                        if writer
                            .write_all(
                                b"
",
                            )
                            .is_err()
                        {
                            break;
                        }
                        let _ = writer.flush();
                    }
                });

                let mut my_name: Option<String> = None;

                for line in reader.lines() {
                    let line = match line {
                        Ok(l) => l,
                        Err(_) => break,
                    };
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }

                    // 第一条必须是 register
                    if my_name.is_none() {
                        let name = serde_json::from_str::<serde_json::Value>(line)
                            .ok()
                            .and_then(|v| {
                                v.get("register")
                                    .and_then(|r| r.as_str())
                                    .map(|s| s.to_string())
                            });
                        match name {
                            Some(n) => {
                                println!("[DataBus] {} 注册为 '{}'", peer, n);
                                registry.lock().unwrap().insert(
                                    n.clone(),
                                    PeerRegistration {
                                        connection_id,
                                        sender: tx.clone(),
                                    },
                                );
                                my_name = Some(n);
                            }
                            None => {
                                let _ = tx.send(r#"{"error":"must_register_first"}"#.to_string());
                                break;
                            }
                        }
                        continue;
                    }

                    // 路由
                    let v: serde_json::Value = match serde_json::from_str(line) {
                        Ok(v) => v,
                        Err(_) => {
                            let _ = tx.send(format!(r#"{{"error":"bad_json","raw":{:?}}}"#, line));
                            continue;
                        }
                    };
                    let to = v.get("to").and_then(|t| t.as_str()).unwrap_or("");
                    let is_broadcast = v
                        .get("broadcast")
                        .and_then(|b| b.as_bool())
                        .unwrap_or(false);
                    let from = my_name.clone().unwrap_or_default();

                    // One-way authority policy:
                    // Fabric is the only authoritative publisher of player
                    // and Monster state messages.
                    let message_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

                    let fabric_only_message = matches!(
                        message_type,
                        "player_move" |
                        "monster_spawn" |
                        "monster_update" |
                        "monster_remove"
                    );

                    if fabric_only_message && from != "fabric" {
                        println!(
                            "[DataBus][REJECT] '{}' attempted to publish {}",
                            from,
                            message_type
                        );

                        let rejection = serde_json::json!({
                            "error": "state_authority_violation",
                            "message_type": message_type,
                            "authoritative_sender": "fabric"
                        });

                        let _ = tx.send(rejection.to_string());

                        continue;
                    }

                    if to.is_empty() && !is_broadcast {
                        let _ = tx.send(r#"{"error":"missing_to"}"#.to_string());
                        continue;
                    }

                    let mut forward = v.clone();
                    forward["from"] = serde_json::Value::String(from.clone());

                    if is_broadcast {
                        // 广播模式：发给所有已注册的客户端
                        let payload = serde_json::to_string(&forward).unwrap_or_default();
                        let reg = registry.lock().unwrap();
                        for (name, registration) in reg.iter() {
                            if name != &from {
                                if registration.sender.send(payload.clone()).is_ok() {
                                    println!("[DataBus] {} 广播 -> {}", from, name);
                                }
                            }
                        }
                    } else {
                        // 点对点路由模式
                        let target_tx = registry
                            .lock()
                            .unwrap()
                            .get(to)
                            .map(|registration| registration.sender.clone());
                        match target_tx {
                            Some(peer_tx) => {
                                let payload = serde_json::to_string(&forward).unwrap_or_default();
                                if peer_tx.send(payload).is_ok() {
                                    println!("[DataBus] {} -> {} routed", from, to);
                                    let ack = format!(
                                        r#"{{"routed":true,"from":"{}","to":"{}"}}"#,
                                        from, to
                                    );
                                    let _ = tx.send(ack);
                                } else {
                                    let _ = tx.send(format!(
                                        r#"{{"error":"peer_write_failed","to":"{}"}}"#,
                                        to
                                    ));
                                }
                            }
                            None => {
                                println!("[DataBus] {} -> {} 目标不在线", from, to);
                                let _ =
                                    tx.send(format!(r#"{{"error":"peer_offline","to":"{}"}}"#, to));
                            }
                        }
                    }
                }

                if let Some(name) = my_name {
                    let mut reg = registry.lock().unwrap();

                    let is_current_connection = reg
                        .get(&name)
                        .map(|registration| {
                            registration.connection_id == connection_id
                        })
                        .unwrap_or(false);

                    if is_current_connection {
                        reg.remove(&name);
                        println!(
                            "[DataBus] '{}' 断开，已清理当前注册项",
                            name
                        );
                    } else {
                        println!(
                            "[DataBus] '{}' 旧连接断开，保留新连接注册项",
                            name
                        );
                    }
                }
                drop(tx);
                let _ = write_handle;
            });
        }
    });
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();

    let is_fusion = args
        .iter()
        .any(|arg| arg == "fusion" || arg == "--mode=fusion" || arg == "--mode=fusion");
    let is_bus_test = args
        .iter()
        .any(|arg| arg == "bus-test" || arg == "--mode=bus-test");

    if is_bus_test {
        println!("=== 启动数据总线测试模式 ===");
        start_data_bus().expect("[DataBus] bus-test 无法监听 127.0.0.1:25565");
        loop {
            thread::sleep(std::time::Duration::from_secs(60));
        }
    }

    if is_fusion {
        println!("=== 启动双沙盒 Fusion 模式 ===");
        start_data_bus().expect("[DataBus] Fusion 父进程无法监听 127.0.0.1:25565");
        let current_dir = env::current_dir().unwrap();
        let sandbox_dir = current_dir.join("sandbox");

        // 启动 Fabric
        let _sandbox_fab = sandbox_dir.clone();

        let _fab_thread = thread::spawn(move || {
            let mut cmd = std::process::Command::new("cargo");

            // Agent 由 Fabric JVM 的 jvm_args 显式注入。
            // 不要再通过 JAVA_TOOL_OPTIONS 注入第二次。
            cmd.args(&["run", "--", "--mode", "fabric"]);

            cmd.env("JVM_COMBINER_BUS_EXTERNAL", "1");
            cmd.status().unwrap();
        });

        // 启动 NeoForge
        // 启动 NeoForge
        let _sandbox_nf = sandbox_dir.clone();
        let _nf_thread = thread::spawn(move || {
            let mut cmd = std::process::Command::new("cargo");

            cmd.args(&["run", "--", "--mode", "neoforge"]);
            cmd.env("JVM_COMBINER_BUS_EXTERNAL", "1");
            cmd.status().unwrap();
        });

        loop {
            thread::sleep(std::time::Duration::from_secs(60));
        }
    }
    let is_forge = args
        .windows(2)
        .any(|pair| pair[0] == "--mode" && pair[1].eq_ignore_ascii_case("forge"))
        || args.iter().any(|arg| arg == "forge");
    println!(
        "=== Rust JVM Combiner Kernel: {} Mode ===",
        if is_forge { "Forge" } else { "Fabric" }
    );

    let current_dir = env::current_dir().unwrap();
    let sandbox_dir = current_dir.join("sandbox");

    // Forge: launch as a subprocess with proper -p / --add-modules
    if is_forge {
        launch_forge_subprocess(&sandbox_dir);
        return;
    }

    let is_neoforge = args
        .windows(2)
        .any(|pair| pair[0] == "--mode" && pair[1].eq_ignore_ascii_case("neoforge"))
        || args.iter().any(|arg| arg == "neoforge");
    if is_neoforge {
        if env::var_os("JVM_COMBINER_BUS_EXTERNAL").is_none() {
            start_data_bus().expect("[DataBus] NeoForge 无法监听 127.0.0.1:25565");
        }

        launch_neoforge_subprocess(&sandbox_dir);
        return;
    }
    // Fabric uses JNI, so start DataBus before creating the JVM.
    if env::var_os("JVM_COMBINER_BUS_EXTERNAL").is_none() {
        start_data_bus().expect("[DataBus] Fabric 无法监听 127.0.0.1:25565");
    }

    let (bootstrap_cp, legacy_cp) = build_classpath(is_forge, &sandbox_dir);

    let join = |v: &Vec<PathBuf>| -> String {
        v.iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(":")
    };
    let bootstrap_cp_str = join(&bootstrap_cp);
    let legacy_cp_str = join(&legacy_cp);
    println!("[Rust] Bootstrap CP entries: {}", bootstrap_cp.len());
    println!("[Rust] Legacy CP entries: {}", legacy_cp.len());

    let mut jvm_args: Vec<String> = Vec::new();
    jvm_args.push("-Dlog4j.skipJansi=false".to_string());
    jvm_args.push("-Dlog4j2.formatMsgNoLookups=true".to_string());
    jvm_args.push(format!("-Djava.class.path={}", bootstrap_cp_str));
    if is_forge {
        let lib_dir = sandbox_dir.join("forge_env/libraries");
        jvm_args.push(format!("-DlegacyClassPath={}", legacy_cp_str));
        jvm_args.push(format!("-DlibraryDirectory={}", lib_dir.display()));
        jvm_args.push("-DignoreList=asm,securejarhandler".to_string());
        jvm_args.push("-Dforge.logging.console.level=debug".to_string());
        jvm_args.push("-Dorg.slf4j.simpleLogger.defaultLogLevel=debug".to_string());
        jvm_args.push("-Djava.net.preferIPv6Addresses=system".to_string());
        for o in [
            "--add-opens=java.base/java.util.jar=ALL-UNNAMED",
            "--add-exports=java.base/sun.security.util=ALL-UNNAMED",
            "--add-opens=java.base/java.lang.invoke=ALL-UNNAMED",
            "--add-opens=java.base/java.nio=ALL-UNNAMED",
            "--add-opens=java.base/sun.nio.ch=ALL-UNNAMED",
            "--add-opens=java.base/java.io=ALL-UNNAMED",
            "--add-opens=java.base/java.util=ALL-UNNAMED",
            "--add-opens=java.base/java.lang=ALL-UNNAMED",
        ] {
            jvm_args.push(o.to_string());
        }
    }

    let agent_jar = std::env::current_dir()
        .unwrap()
        .join("bus_test/bus-agent.jar");
    if agent_jar.exists() {
        jvm_args.push(format!(
            "-javaagent:{}={}",
            agent_jar.display(),
            "Fabric-Sandbox"
        ));
        println!("[Rust] 强行注入 -javaagent: {}", agent_jar.display());
    }

    let mut builder = InitArgsBuilder::new().version(JNIVersion::V1_8);
    for arg in &jvm_args {
        builder = builder.option(arg);
    }
    let jvm = JavaVM::new(builder.build().expect("Failed to build JVM arguments"))
        .expect("Failed to start JVM");

    jvm.attach_current_thread(|env| {
        let mc_jar_path = format!("{}/minecraft-client.jar", sandbox_dir.display());
        let main_class_jni = if is_forge {
            jni_str!("net/minecraftforge/bootstrap/ForgeBootstrap")
        } else {
            jni_str!("net/fabricmc/loader/impl/launch/knot/KnotClient")
        };
        let main_class_name = if is_forge {
            "net.minecraftforge.bootstrap.ForgeBootstrap"
        } else {
            "net.fabricmc.loader.impl.launch.knot.KnotClient"
        };

        let class_obj: JClass = match env.find_class(main_class_jni) {
            Ok(c) => c,
            Err(e) => {
                println!("[FAIL] find_class({}) threw: {:?}", main_class_name, e);
                // Print the full Java stack trace BEFORE clearing the exception
                if let Some(exc) = env.exception_occurred() {
                    println!("[Java exception] printing stack trace:");
                    let _ = env.call_method(&exc, jni_str!("printStackTrace"), jni_sig!(() -> void), &[]);
                    env.exception_clear();
                } else {
                    println!("[WARN] No pending Java exception after find_class failed.");
                }
                return Ok(());
            }
        };
        println!("[OK] {} loaded", main_class_name);

        if is_forge {
            set_property(env, "legacyClassPath", &legacy_cp_str);
            set_property(env, "libraryDirectory", format!("{}/forge_env/libraries", sandbox_dir.display()).as_str());
            set_property(env, "ignoreList", "asm,securejarhandler");
        } else {
            set_property(env, "fabric.gameJarPath", &mc_jar_path);
            set_property(env, "fabric.debug.disableClassPathIsolation", "true");
            set_property(env, "fabric.gameVersion", "1.21.4");
        }

        println!("[Rust] Calling {}.main(...) ...", main_class_name);
        let empty_jstring = env.new_string("").unwrap();
        let args_array: JObjectArray<JString> = JObjectArray::<JString>::new(env, 0, &empty_jstring).unwrap();
        let call_result = env.call_static_method(
            &class_obj,
            jni_str!("main"),
            jni_sig!((java.lang.String[]) -> void),
            &[JValue::Object(&args_array)],
        );
        match call_result {
            Ok(_) => println!("[OK] {} finished.", main_class_name),
            Err(e) => {
                println!("[FAIL] {} threw: {:?}", main_class_name, e);
                if let Some(exc) = env.exception_occurred() {
                    println!("[Java exception caught]");
                    env.exception_clear();
                    let _ = env.call_method(&exc, jni_str!("printStackTrace"), jni_sig!(() -> void), &[]);
                } else {
                    println!("[WARN] No pending Java exception. JVM may be shutting down (System.exit was likely called).");
                }
            }
        }
        Ok::<(), jni::errors::Error>(())
    }).expect("Failed to attach thread");
    println!("=== Test finished ===");
}
