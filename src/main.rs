use jni::{InitArgsBuilder, JNIVersion, JavaVM};
use jni::objects::{JValue, JClass, JString, JObjectArray};
use jni::jni_str;
use jni::jni_sig;
use serde_json::Value;
use std::env;
use std::path::{Path, PathBuf};
use std::fs;

fn rules_allow(lib: &Value) -> bool {
    let rules = match lib.get("rules").and_then(|r| r.as_array()) {
        Some(r) => r,
        None => return true,
    };
    let mut allow = false;
    for rule in rules {
        let action = rule.get("action").and_then(|a| a.as_str()).unwrap_or("allow");
        let os_name = rule.pointer("/os/name").and_then(|v| v.as_str());
        let os_arch = rule.pointer("/os/arch").and_then(|v| v.as_str());
        let matches = (os_name.is_none() || os_name == Some("osx"))
            && (os_arch.is_none() || os_arch == Some("arm64"));
        if matches { allow = action == "allow"; }
    }
    allow
}

fn find_local(rel_path: &str, search_dirs: &[PathBuf]) -> Option<PathBuf> {
    let basename = rel_path.rsplit('/').next().unwrap_or(rel_path);
    for dir in search_dirs {
        let full = dir.join(rel_path);
        if full.exists() { return Some(full); }
        let flat = dir.join(basename);
        if flat.exists() { return Some(flat); }
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
        if !rules_allow(lib) { continue; }
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
    println!("[Rust] Resolved {}/{} libraries from {}", classpath.len(), total, json_path.display());
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

fn set_property(env: &mut jni::Env, key: &str, value: &str) {
    let system_class = env.find_class(jni_str!("java/lang/System")).unwrap();
    let key_str = env.new_string(key).unwrap();
    let value_str = env.new_string(value).unwrap();
    env.call_static_method(
        system_class,
        jni_str!("setProperty"),
        jni_sig!((java.lang.String, java.lang.String) -> java.lang.String),
        &[JValue::Object(&key_str), JValue::Object(&value_str)],
    ).unwrap();
}

fn build_classpath(is_forge: bool, sandbox_dir: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mc_root = PathBuf::from(env::var("HOME").unwrap())
        .join("Library/Application Support/minecraft");
    let mut legacy_cp: Vec<PathBuf> = Vec::new();
    let mut bootstrap_cp: Vec<PathBuf> = Vec::new();

    if is_forge {
        let forge_env = sandbox_dir.join("forge_env");
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
            if !legacy_cp.contains(&p) { legacy_cp.push(p); }
        }
        for p in resolve_classpath(&v_json, &dirs) {
            if !legacy_cp.contains(&p) { legacy_cp.push(p.clone()); }
        }
        legacy_cp.push(sandbox_dir.join("minecraft-client.jar"));
    } else {
        for name in ["fabric-loader.jar", "asm-9.6.jar", "asm-tree-9.6.jar", "asm-analysis-9.6.jar", "asm-commons-9.6.jar", "asm-util-9.6.jar", "sponge-mixin.jar"] {
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
            if !legacy_cp.contains(&p) { legacy_cp.push(p.clone()); }
            if !bootstrap_cp.contains(&p) { bootstrap_cp.push(p); }
        }
        let mc_jar = sandbox_dir.join("minecraft-client.jar");
        legacy_cp.push(mc_jar.clone());
        bootstrap_cp.push(mc_jar);
    }

    (bootstrap_cp, legacy_cp)
}



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
    let found = sections.iter_mut().skip(1).find(|section| {
        section.iter().any(|line| line.as_str() == package_section)
    });
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

    let forge_env = sandbox_dir.join("forge_env");
    let mc_root = PathBuf::from(
        env::var("HOME").expect("HOME is not set")
    ).join("Library/Application Support/minecraft");
    let libs = mc_root.join("libraries");

    // Build legacyClassPath: all jars except bootstrap-related modules
    let mut legacy: Vec<String> = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<String>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() { walk(&p, out); }
                else if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(".jar")
                        && !name.starts_with("bootstrap-")
                        && !name.starts_with("bootstrap-api-")
                        && !name.contains("bootstraplauncher")
                        && !name.contains("securejarhandler")
                        && !name.contains("securemodules")
                        // && !name.contains("jopt-simple") // 已修復：允許載入 jopt-simple
                        && !name.contains("jna")
                        && !name.contains("unsafe")
                        && !name.contains("forgespi")
                        && !name.contains("fmlloader")
    // && !name.contains("logging") // 已修復：允許載入日誌庫
    // && !name.contains("slf4j") // 已修復：允許載入 SLF4J
                        && !name.contains("fmlloader-1.21.4-54.1.22.jar")
                        && !name.contains("client-1.21.4-official")
                        && !name.contains("log4j")
                        && !name.contains("terminalconsoleappender")
                        && !name.contains("jline")
                        && !name.contains("log4j")
                        && !name.contains("terminalconsoleappender")
                    {
                        out.push(p.to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    walk(&libs, &mut legacy);

    // Keep ModLauncher off the JVM's initial --module-path. Bootstrap is responsible for
    // loading it from legacyClassPath into its managed module layer, where package version
    // metadata is available. Directly resolving it as a boot-layer module caused the NPE.
    let modlauncher_path = libs.join("net/minecraftforge/modlauncher/10.2.4/modlauncher-10.2.4.jar");
    let patched_ml_path = sandbox_dir.join(".jar_cache/modlauncher-10.2.4.jar");
    if patched_ml_path.is_file() {
        eprintln!("[forge] ModLauncher 使用补丁版: {}", patched_ml_path.display());
    } else if modlauncher_path.is_file() {
        eprintln!("[forge] WARN: 补丁版缺失，回退原始: {}", modlauncher_path.display());
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
    let mut final_modlauncher = modlauncher_path.to_string_lossy().to_string();
    if modlauncher_path.is_file() {
        if let Some(sanitized_ml) = sanitize_jar(&modlauncher_path, &cache_dir) {
            eprintln!("[forge] 使用已補丁的 ModLauncher: {}", sanitized_ml.display());
            final_modlauncher = sanitized_ml.to_string_lossy().to_string();
        } else {
            eprintln!("[forge] 使用原始 ModLauncher");
        }
        // legacy.push(final_modlauncher); // 已停用：防止雙重載入衝突
    }

    // Bootstrap 2.1.8 builds its input list from `java.class.path` via
    // findAllClassPathEntries(). The `legacyClassPath` property alone is not
    // sufficient for this version, so ensure Forge's client JAR is present and
    // pass the complete legacy list to the JVM's actual -cp argument below.
    
    // 強制將 bootstrap 與 bootstrap-api 加入 legacy_cp 作為 Classpath 根基
    let bootstrap_jar = libs.join("net/minecraftforge/bootstrap/2.1.8/bootstrap-2.1.8.jar");
    let bootstrap_api_jar = libs.join("net/minecraftforge/bootstrap-api/2.1.8/bootstrap-api-2.1.8.jar");
    if bootstrap_jar.is_file() && !legacy.iter().any(|p| Path::new(p) == bootstrap_jar.as_path()) {
        legacy.insert(0, bootstrap_jar.to_string_lossy().to_string());
    }
    if bootstrap_api_jar.is_file() && !legacy.iter().any(|p| Path::new(p) == bootstrap_api_jar.as_path()) {
        legacy.insert(0, bootstrap_api_jar.to_string_lossy().to_string());
    }

    
    // 強制將 bootstrap 與 bootstrap-api 加入 legacy_cp 作為 Classpath 根基
    let bootstrap_jar = libs.join("net/minecraftforge/bootstrap/2.1.8/bootstrap-2.1.8.jar");
    let bootstrap_api_jar = libs.join("net/minecraftforge/bootstrap-api/2.1.8/bootstrap-api-2.1.8.jar");
    if bootstrap_jar.is_file() && !legacy.iter().any(|p| Path::new(p) == bootstrap_jar.as_path()) {
        legacy.insert(0, bootstrap_jar.to_string_lossy().to_string());
    }
    if bootstrap_api_jar.is_file() && !legacy.iter().any(|p| Path::new(p) == bootstrap_api_jar.as_path()) {
        legacy.insert(0, bootstrap_api_jar.to_string_lossy().to_string());
    }

    let forge_client = libs.join("net/minecraftforge/forge/1.21.4-54.1.22/forge-1.21.4-54.1.22-client.jar");


    if forge_client.is_file()
        && !legacy.iter().any(|p| Path::new(p) == forge_client.as_path())
    {
        legacy.insert(0, forge_client.to_string_lossy().to_string());
    }
    
    // 確保 Bootstrap、Bootstrap-API 與 Forge Client 必定存在於 Classpath 頂端
    let bootstrap_path = libs.join("net/minecraftforge/bootstrap/2.1.8/bootstrap-2.1.8.jar");
    let bootstrap_api_path = libs.join("net/minecraftforge/bootstrap-api/2.1.8/bootstrap-api-2.1.8.jar");
    
    if bootstrap_path.exists() {
        let p_str = bootstrap_path.to_string_lossy().to_string();
        if !legacy.contains(&p_str) { legacy.insert(0, p_str); }
    }
    if bootstrap_api_path.exists() {
        let p_str = bootstrap_api_path.to_string_lossy().to_string();
        if !legacy.contains(&p_str) { legacy.insert(0, p_str); }
    }
    if forge_client.exists() {
        let p_str = forge_client.to_string_lossy().to_string();
        if !legacy.contains(&p_str) { legacy.insert(0, p_str); }
    }

    
    // 確保 Bootstrap、Bootstrap-API 與 Forge Client 必定存在於 Classpath 頂端
    let bootstrap_path = libs.join("net/minecraftforge/bootstrap/2.1.8/bootstrap-2.1.8.jar");
    let bootstrap_api_path = libs.join("net/minecraftforge/bootstrap-api/2.1.8/bootstrap-api-2.1.8.jar");
    
    if bootstrap_path.exists() {
        let p_str = bootstrap_path.to_string_lossy().to_string();
        if !legacy.contains(&p_str) { legacy.insert(0, p_str); }
    }
    if bootstrap_api_path.exists() {
        let p_str = bootstrap_api_path.to_string_lossy().to_string();
        if !legacy.contains(&p_str) { legacy.insert(0, p_str); }
    }
    if forge_client.exists() {
        let p_str = forge_client.to_string_lossy().to_string();
        if !legacy.contains(&p_str) { legacy.insert(0, p_str); }
    }

    // Bootstrap API stays on the initial module path; do not expose it
    // again to Bootstrap's managed-layer classpath scanner.
    legacy.retain(|p| {
        !p.contains("bootstrap-api")
            && !p.contains("/bootstrap/2.1.8/")
            // Keep the newer official failureaccess module only.
            && !p.contains("/failureaccess/1.0.1/")
            // Do not let sandbox copies enter Bootstrap's module layer.
            && !(p.contains("/sandbox/mc_libraries/")
                && p.to_ascii_lowercase().contains("failureaccess-"))
    });
    // 把补丁版 ModLauncher 放进 legacy，让 Bootstrap 从 java.class.path 扫描到它
    let patched_ml = sandbox_dir.join(".jar_cache/modlauncher-10.2.4.jar");
    if patched_ml.is_file() {
        legacy.retain(|p| !p.contains("/modlauncher/"));
        legacy.insert(0, patched_ml.to_string_lossy().to_string());
        eprintln!("[forge] legacyClassPath 使用补丁 ModLauncher: {}", patched_ml.display());
    } else {
        eprintln!("[forge] ERROR: 补丁 ModLauncher 不存在: {}", patched_ml.display());
    }

    legacy.retain(|p| !p.contains("/bootstrap/2.1.7/"));
    let legacy_cp = legacy.join(":");



    // JVM module path contains the bootstrap/runtime modules. ModLauncher itself is deliberately
    // supplied through legacyClassPath so Bootstrap can place it in its managed module layer.
    let _modlauncher_module_pb = if let Some(sanitized_ml) = sanitize_jar(&modlauncher_path, &cache_dir) {
        sanitized_ml
    } else {
        modlauncher_path.clone()
    };

            let mut module_jars: Vec<String> = {
        let mut paths = vec![
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
            libs.join("net/minecrell/terminalconsoleappender/1.2.0/terminalconsoleappender-1.2.0.jar"),
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
                let file_name = p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                
                // 絕對防線：嚴格禁止任何非快取或多餘的 modlauncher 進入模組路徑
                if file_name.contains("modlauncher") || file_name.contains("cpw.mods.modlauncher") {
                    if s.contains(".jar_cache") && file_name == "modlauncher-10.2.4.jar" && !seen_ml {
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
                println!("[forge-debug] 最終加入 module-path 的 modlauncher 路徑: {}", m);
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
            "net/minecraftforge/forgespi/7.1.5/forgespi-7.1.5.jar",
            "net/minecraftforge/mergetool-api/1.0/mergetool-api-1.0.jar",
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
            "net/minecraftforge/fmlloader/1.21.4-54.1.22/fmlloader-1.21.4-54.1.22.jar",
        ];
                for rel in core_rel_paths {
            let path = libs.join(rel);
            if !path.is_file() {
                continue;
            }
            // ModLauncher 用终端 python3 生成的补丁 JAR，避免 getPackage() 版本为 null 的 NPE
            let effective = if rel.contains("modlauncher/") {
                let patched = sandbox_dir.join(".jar_cache/modlauncher-10.2.4.jar");
                if patched.is_file() {
                    eprintln!("[forge] module-path ModLauncher 使用补丁版: {}", patched.display());
                    patched
                } else {
                    eprintln!("[forge] WARN: 补丁 JAR 不存在，回退官方版: {}", path.display());
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
        .arg("-v").arg("21")
        .output() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            eprintln!("[forge] 使用自動檢測的 Java 21: {}", path);
            format!("{}/bin/java", path)
        } else {
            env::var("JAVA_HOME").map(|h| format!("{}/bin/java", h)).unwrap_or_else(|_| "java".to_string())
        }
    } else {
        env::var("JAVA_HOME").map(|h| format!("{}/bin/java", h)).unwrap_or_else(|_| "java".to_string())
    };

    let mut cmd = std::process::Command::new(&java);
    // Critical: Bootstrap 2.1.8 scans `java.class.path` to build its runtime
    // module layer. Pass the full legacy JAR list as the actual JVM classpath;
    // setting only -DlegacyClassPath does not make those JARs visible to that scan.
    
    // 【強制修正】直接建構包含 bootstrap 與所有 forge libraries 的絕對 Classpath
    // Build the actual JVM classpath from installed libraries and sandbox game files.
    let bootstrap_jar = libs.join("net/minecraftforge/bootstrap/2.1.8/bootstrap-2.1.8.jar");
    let bootstrap_api_jar = libs.join("net/minecraftforge/bootstrap-api/2.1.8/bootstrap-api-2.1.8.jar");
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
        if Path::new(path).is_file() && !real_cp.contains(path) {
            real_cp.push(path.clone());
        }
    }

    for path in find_jars_in(&sandbox_dir.join("mc_libraries"), "") {
        let value = path.to_string_lossy().to_string();
        let filename = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        // Keep Mojang Logging from the installed Minecraft libraries.
        // Sandbox copies use conflicting automatic module names.
        if filename.starts_with("logging-")
            || filename.contains("com.mojang.logging")
        {
            eprintln!("[forge] 排除沙盒重复 Logging JAR: {}", value);
            continue;
        }

        if Path::new(&value).is_file() && !real_cp.contains(&value) {
            real_cp.push(value);
        }
    }

    // Avoid discovering Bootstrap API a second time from java.class.path.
    real_cp.retain(|p| {
        !p.contains("bootstrap-api")
            && !p.contains("jopt-simple/6.0-alpha-3/")
            // Avoid loading the old automatic module named "failureaccess".
            && !p.contains("/failureaccess/1.0.1/")
            // Sandbox copies can introduce additional automatic modules.
            && !(p.contains("/sandbox/mc_libraries/")
                && p.to_ascii_lowercase().contains("failureaccess-"))
    });
    if patched_ml.is_file() {
        real_cp.retain(|p| !p.contains("/modlauncher/"));
        let patched_str = patched_ml.to_string_lossy().to_string();
        if !real_cp.contains(&patched_str) {
            real_cp.insert(0, patched_str);
        }
    }
    for p in real_cp.iter().filter(|p| p.contains("modlauncher")) {
        eprintln!("[forge][debug] real_cp modlauncher: {}", p);
    }
    let forced_cp_str = real_cp.join(":");
    cmd.arg("-cp").arg(&forced_cp_str);

    if forge_client.is_file() {
        eprintln!("[forge] forge-client included in Bootstrap classpath: {}", forge_client.display());
    } else {
        eprintln!("[forge] ERROR: forge-client JAR missing: {}", forge_client.display());
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
    let mc_root = PathBuf::from(env::var("HOME").expect("HOME")).join("Library/Application Support/minecraft");
    let version = "neoforge-21.1.191";
    let neoforge_ver = "21.1.191";
    let unix_args = mc_root.join(format!("libraries/net/neoforged/neoforge/{}/unix_args.txt", neoforge_ver));
    if !unix_args.is_file() {
        eprintln!("[neoforge] ERROR: unix_args.txt 不存在: {}", unix_args.display());
        eprintln!("[neoforge] 先用 --installServer 跑一次 installer 生成它");
        return;
    }

    // 读 unix_args.txt，拆成参数。文件里每行可能含多个以空格分隔的参数，
    // 但路径里也可能有空格（macOS 的 Application Support），所以按行读、
    // 只对每行的第一个 token 识别，其余保留原样。
    let content = fs::read_to_string(&unix_args).expect("read unix_args.txt");
    let mut jvm_args: Vec<String> = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        // unix_args.txt 里每个逻辑参数一行，但 -p 那一行是 "-p <path>" 两个 token
        // 以及 "-Dkey=value" 是单 token。按空白切分，但路径里的空格需要保留：
        // 官方文件里路径不含空格（相对路径 libraries/...），所以直接切分即可。
        for tok in line.split_whitespace() {
            jvm_args.push(tok.to_string());
        }
    }

    // 把 forgeserver 换成 forgeclient
    for a in jvm_args.iter_mut() {
        if a == "forgeserver" { *a = "forgeclient".to_string(); }
    }

    // 扫描 LWJGL jar 并追加到 legacyClassPath（服务端 unix_args 不含客户端 LWJGL 依赖）
    let lwjgl_root = mc_root.join("libraries/org/lwjgl");
    let mut lwjgl_jars: Vec<String> = Vec::new();
    fn walk_lwjgl(dir: &Path, out: &mut Vec<String>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk_lwjgl(&p, out);
                } else if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
                    let ps = p.to_string_lossy();
                    // 只要路径里含 /3.3.3/ 的 jar（普通 + natives 都要），排除 3.4.x
                    if n.ends_with(".jar") && ps.contains("/3.3.3/") {
                        out.push(ps.to_string());
                    }
                }
            }
        }
    }
    walk_lwjgl(&lwjgl_root, &mut lwjgl_jars);
    eprintln!("[neoforge] 扫描到 {} 个 LWJGL jar", lwjgl_jars.len());

    // 扫描 ICU4J（客户端字体渲染需要）
    let icu_root = mc_root.join("libraries/com/ibm/icu");
    let mut icu_jars: Vec<String> = Vec::new();
    fn walk_icu(dir: &Path, out: &mut Vec<String>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() { walk_icu(&p, out); }
                else if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
                    if n.ends_with(".jar") {
                        out.push(p.to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    walk_icu(&icu_root, &mut icu_jars);
    eprintln!("[neoforge] 扫描到 {} 个 ICU4J jar", icu_jars.len());

    // 扫描 text2speech（客户端 Narrator 需要）
    let tts_root = mc_root.join("libraries/com/mojang/text2speech");
    let mut tts_jars: Vec<String> = Vec::new();
    fn walk_tts(dir: &Path, out: &mut Vec<String>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() { walk_tts(&p, out); }
                else if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
                    if n.ends_with(".jar") {
                        out.push(p.to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    walk_tts(&tts_root, &mut tts_jars);
    eprintln!("[neoforge] 扫描到 {} 个 text2speech jar", tts_jars.len());
    // 找到 jvm_args 里的 -DlegacyClassPath=...，追加 LWJGL
    for a in jvm_args.iter_mut() {
        if a.starts_with("-DlegacyClassPath=") {
            let mut val = a.trim_start_matches("-DlegacyClassPath=").to_string();
            for j in &lwjgl_jars {
                if !val.contains(j) {
                    val.push(':');
                    val.push_str(j);
                }
            }
            for j in &icu_jars {
                if !val.contains(j) {
                    val.push(':');
                    val.push_str(j);
                }
            }
            for j in &tts_jars {
                if !val.contains(j) {
                    val.push(':');
                    val.push_str(j);
                }
            }
            *a = format!("-DlegacyClassPath={}", val);
            break;
        }
    }

    // 补客户端 game 参数
    let mut game_args: Vec<String> = vec![
        "--gameDir".into(), sandbox_dir.to_string_lossy().to_string(),
        "--assetsDir".into(), sandbox_dir.join("assets").to_string_lossy().to_string(),
        "--assetIndex".into(), "17".into(),
        "--version".into(), "1.21.1".into(),
        "--username".into(), "Dev".into(),
        "--uuid".into(), "00000000000000000000000000000000".into(),
        "--accessToken".into(), "0".into(),
        "--userType".into(), "legacy".into(),
        "--versionType".into(), "release".into(),
    ];

    let java = if let Ok(out) = std::process::Command::new("/usr/libexec/java_home")
        .arg("-v").arg("21").output() {
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !p.is_empty() { format!("{}/bin/java", p) } else { "java".to_string() }
    } else { "java".to_string() };

    let mut cmd = std::process::Command::new(&java);
    // 工作目录设成 mc_root，因为 unix_args.txt 里是相对路径 libraries/...
    cmd.current_dir(&mc_root);
    // macOS: GLFW 窗口必须在 main thread 创建
    if cfg!(target_os = "macos") {
        cmd.arg("-XstartOnFirstThread");
    }
    for a in &jvm_args { cmd.arg(a); }
    for a in &game_args { cmd.arg(a); }

    println!("[neoforge] Launching: {:?}", cmd);
    cmd.stdout(std::process::Stdio::inherit());
    cmd.stderr(std::process::Stdio::inherit());
    match cmd.spawn() {
        Ok(mut child) => { let _ = child.wait(); }
        Err(e) => eprintln!("[neoforge] spawn failed: {}", e),
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let is_forge = args.windows(2).any(|pair| {
        pair[0] == "--mode" && pair[1].eq_ignore_ascii_case("forge")
    }) || args.iter().any(|arg| arg == "forge");
    println!("=== Rust JVM Combiner Kernel: {} Mode ===", if is_forge { "Forge" } else { "Fabric" });

    let current_dir = env::current_dir().unwrap();
    let sandbox_dir = current_dir.join("sandbox");

    // Forge: launch as a subprocess with proper -p / --add-modules
    if is_forge {
        launch_forge_subprocess(&sandbox_dir);
        return;
    }

    let is_neoforge = args.windows(2).any(|pair| {
        pair[0] == "--mode" && pair[1].eq_ignore_ascii_case("neoforge")
    }) || args.iter().any(|arg| arg == "neoforge");
    if is_neoforge {
        launch_neoforge_subprocess(&sandbox_dir);
        return;
    }
    let (bootstrap_cp, legacy_cp) = build_classpath(is_forge, &sandbox_dir);

    let join = |v: &Vec<PathBuf>| -> String {
        v.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>().join(":")
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
