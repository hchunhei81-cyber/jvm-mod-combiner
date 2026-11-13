mod display_geometry;
mod display_topology;
mod window_accessibility;
mod window_discovery;
mod window_dry_run;
mod window_integration;
mod window_layout;
mod window_manager;
mod window_transaction;

use jni::jni_sig;
use jni::jni_str;
use jni::objects::{JClass, JObjectArray, JString, JValue};
use jni::{InitArgsBuilder, JNIVersion, JavaVM};
use serde_json::Value;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::thread;

// 默认使用正式 Agent；测试时可通过 JVM_COMBINER_AGENT_JAR 指定临时 JAR。
fn resolve_agent_jar(project_root: &Path) -> PathBuf {
    match env::var_os("JVM_COMBINER_AGENT_JAR").filter(|value| !value.is_empty()) {
        Some(value) => {
            let configured = PathBuf::from(value);
            if configured.is_absolute() {
                configured
            } else {
                project_root.join(configured)
            }
        }
        None => project_root.join("bus_test/bus-agent.jar"),
    }
}

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

fn resolve_classpath_recursive(
    json_path: &Path,
    search_dirs: &[PathBuf],
    visited: &mut HashSet<PathBuf>,
    total: &mut usize,
    selected_coordinates: &mut HashSet<String>,
) -> Vec<PathBuf> {
    let identity = json_path
        .canonicalize()
        .unwrap_or_else(|_| json_path.to_path_buf());

    if !visited.insert(identity.clone()) {
        eprintln!(
            "[Rust] WARNING: version inheritance cycle detected at {}",
            json_path.display()
        );
        return Vec::new();
    }

    let json_str = std::fs::read_to_string(json_path).expect("Failed to read version JSON");
    let json: Value = serde_json::from_str(&json_str).expect("Failed to parse version JSON");

    let mut classpath: Vec<PathBuf> = Vec::new();
    let mut classpath_seen: HashSet<PathBuf> = HashSet::new();

    // Resolve this version before its parents.
    // A child version's Maven dependency takes precedence over its parent.
    if let Some(libraries) = json["libraries"].as_array() {
        for lib in libraries {
            if !rules_allow(lib) {
                continue;
            }

            *total += 1;

            let coordinate = lib.get("name").and_then(Value::as_str).and_then(|name| {
                let parts: Vec<&str> = name.split(':').collect();

                if parts.len() >= 3 {
                    let classifier = parts
                        .get(3)
                        .map(|value| value.split('@').next().unwrap_or(value));

                    Some(match classifier {
                        Some(classifier) => {
                            format!("{}:{}:{}", parts[0], parts[1], classifier)
                        }
                        None => format!("{}:{}", parts[0], parts[1]),
                    })
                } else {
                    None
                }
            });

            if let Some(ref key) = coordinate {
                if selected_coordinates.contains(key) {
                    eprintln!(
                        "[Rust] Skipping inherited dependency overridden by child: {}",
                        key
                    );
                    continue;
                }
            }

            // Track successful resolution independently from classpath
            // insertion: an artifact may already be present via another
            // dependency, but this Maven coordinate must still be claimed.
            let mut resolved = false;

            if let Some(art) = lib.pointer("/downloads/artifact") {
                if let Some(path) = art.get("path").and_then(Value::as_str) {
                    if let Some(found) = find_local(path, search_dirs) {
                        resolved = true;
                        if classpath_seen.insert(found.clone()) {
                            classpath.push(found);
                        }
                    }
                }
            }

            if let Some(classifiers) = lib.pointer("/downloads/classifiers") {
                for key in ["natives-macos-arm64", "natives-macos"] {
                    if let Some(c) = classifiers.get(key) {
                        if let Some(path) = c.get("path").and_then(Value::as_str) {
                            if let Some(found) = find_local(path, search_dirs) {
                                resolved = true;
                                if classpath_seen.insert(found.clone()) {
                                    classpath.push(found);
                                }
                            }
                        }
                    }
                }
            }

            // Claim the Maven coordinate when at least one artifact exists,
            // even if its path was already inserted by another dependency.
            // Missing child artifacts still permit a parent fallback.
            if resolved {
                if let Some(key) = coordinate {
                    selected_coordinates.insert(key);
                }
            }
        }
    }

    if let Some(parent_id) = json["inheritsFrom"].as_str() {
        let parent_json = json_path
            .parent()
            .and_then(|version_dir| version_dir.parent())
            .map(|versions_dir| {
                versions_dir
                    .join(parent_id)
                    .join(format!("{parent_id}.json"))
            });

        if let Some(parent_json) = parent_json {
            if parent_json.is_file() {
                let inherited = resolve_classpath_recursive(
                    &parent_json,
                    search_dirs,
                    visited,
                    total,
                    selected_coordinates,
                );

                for path in inherited {
                    if classpath_seen.insert(path.clone()) {
                        classpath.push(path);
                    }
                }
            } else {
                eprintln!(
                    "[Rust] WARNING: inherited version JSON not found: {}",
                    parent_json.display()
                );
            }
        }
    }

    visited.remove(&identity);
    classpath
}

fn resolve_classpath(json_path: &Path, search_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut total = 0usize;
    let mut selected_coordinates: HashSet<String> = HashSet::new();

    let classpath = resolve_classpath_recursive(
        json_path,
        search_dirs,
        &mut visited,
        &mut total,
        &mut selected_coordinates,
    );

    println!(
        "[Rust] Resolved {}/{} libraries across version inheritance from {}",
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

// Cache canonicalization results for the duration of one Forge launch.
// Cache failures too, preserving the existing Option-based comparison behavior.
fn cached_canonicalize(
    path: &Path,
    cache: &mut std::collections::HashMap<PathBuf, Option<PathBuf>>,
) -> Option<PathBuf> {
    if let Some(cached) = cache.get(path) {
        return cached.clone();
    }

    let canonical = path.canonicalize().ok();
    cache.insert(path.to_path_buf(), canonical.clone());
    canonical
}

// Cache validation results for the duration of one Forge launch.
// Canonical paths make equivalent path spellings share one result.
fn cached_jar_validation<F>(
    path: &Path,
    cache: &mut std::collections::HashMap<PathBuf, bool>,
    canonical_path_cache: &mut std::collections::HashMap<PathBuf, Option<PathBuf>>,
    validate: F,
) -> bool
where
    F: FnOnce(&Path) -> bool,
{
    let cache_key =
        cached_canonicalize(path, canonical_path_cache).unwrap_or_else(|| path.to_path_buf());

    if let Some(&cached) = cache.get(&cache_key) {
        return cached;
    }

    let valid = validate(path);
    cache.insert(cache_key, valid);
    valid
}

// Validate a JAR as a real ZIP archive before putting it on any JVM path.
// macOS provides /usr/bin/unzip. Original files are never modified.
fn jar_is_valid(
    path: &Path,
    cache: &mut std::collections::HashMap<PathBuf, bool>,
    canonical_path_cache: &mut std::collections::HashMap<PathBuf, Option<PathBuf>>,
) -> bool {
    cached_jar_validation(path, cache, canonical_path_cache, |path| {
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
    })
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

fn accept_forge_jar(
    path: &Path,
    cache: &mut std::collections::HashMap<PathBuf, bool>,
    canonical_path_cache: &mut std::collections::HashMap<PathBuf, Option<PathBuf>>,
) -> bool {
    if incompatible_forge_dependency(path) {
        eprintln!(
            "[JAR-FILTER][REJECT] {} -- incompatible Forge/NeoForge version",
            path.display()
        );
        return false;
    }

    jar_is_valid(path, cache, canonical_path_cache)
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
    let mut legacy_cp_seen: HashSet<PathBuf> = HashSet::new();
    let mut bootstrap_cp_seen: HashSet<PathBuf> = HashSet::new();

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
            push_unique_ordered(&mut legacy_cp, &mut legacy_cp_seen, p);
        }
        for p in resolve_classpath(&v_json, &dirs) {
            push_unique_ordered(&mut legacy_cp, &mut legacy_cp_seen, p);
        }
        legacy_cp.push(sandbox_dir.join("minecraft-client.jar"));
    } else {
        // Fabric: resolve vanilla + Fabric dependencies from the merged
        // Mojang-style profile. Do not inject obsolete flat ASM/Mixin JARs.
        let v_json = sandbox_dir.join("1.21.4.json");
        let dirs = vec![
            sandbox_dir.to_path_buf(),
            sandbox_dir.join("mc_libraries"),
            mc_root.join("libraries"),
        ];

        for p in resolve_classpath(&v_json, &dirs) {
            push_unique_ordered(&mut legacy_cp, &mut legacy_cp_seen, p.clone());
            push_unique_ordered(&mut bootstrap_cp, &mut bootstrap_cp_seen, p);
        }

        let mc_jar = sandbox_dir.join("minecraft-client.jar");

        push_unique_ordered(&mut legacy_cp, &mut legacy_cp_seen, mc_jar.clone());
        push_unique_ordered(&mut bootstrap_cp, &mut bootstrap_cp_seen, mc_jar);
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
    // Validation results are scoped to this Forge launch only.
    let mut jar_validation_cache: std::collections::HashMap<PathBuf, bool> =
        std::collections::HashMap::new();
    let mut canonical_path_cache: std::collections::HashMap<PathBuf, Option<PathBuf>> =
        std::collections::HashMap::new();
    let forge_env = sandbox_dir.join("forge_env");
    let mods_dir = forge_env.join("mods");
    std::fs::create_dir_all(&mods_dir)
        .unwrap_or_else(|e| eprintln!("[forge] 无法创建 mods 目录: {}", e));
    eprintln!("[forge] mods 目录: {}", mods_dir.display());

    let _forge_env = sandbox_dir.join("forge_env");
    let mc_root = PathBuf::from(env::var("HOME").expect("HOME is not set"))
        .join("Library/Application Support/minecraft");
    let libs = mc_root.join("libraries");

    // Forge 1.21.4 inherits the asset index from vanilla 1.21.4.
    let vanilla_json_path = mc_root.join("versions/1.21.4/1.21.4.json");
    let vanilla_json_text = std::fs::read_to_string(&vanilla_json_path).unwrap_or_else(|e| {
        panic!(
            "[forge] 无法读取 Vanilla JSON {}: {}",
            vanilla_json_path.display(),
            e
        )
    });

    let vanilla_json: Value = serde_json::from_str(&vanilla_json_text)
        .unwrap_or_else(|e| panic!("[forge] Vanilla JSON 解析失败: {}", e));

    let asset_index = vanilla_json
        .pointer("/assetIndex/id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("[forge] Vanilla JSON 缺少 assetIndex.id"))
        .to_owned();

    let assets_dir = mc_root.join("assets");
    if !assets_dir.is_dir() {
        panic!(
            "[forge] Minecraft assets 目录不存在: {}",
            assets_dir.display()
        );
    }

    let asset_index_file = assets_dir
        .join("indexes")
        .join(format!("{}.json", asset_index));

    if !asset_index_file.is_file() {
        panic!("[forge] 资源索引文件不存在: {}", asset_index_file.display());
    }

    eprintln!("[forge] assetsDir: {}", assets_dir.display());
    eprintln!("[forge] assetIndex: {}", asset_index);

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
        if !accept_forge_jar(&path, &mut jar_validation_cache, &mut canonical_path_cache) {
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
    legacy.retain(|value| {
        accept_forge_jar(
            Path::new(value),
            &mut jar_validation_cache,
            &mut canonical_path_cache,
        )
    });

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
            libs.join("com/mojang/logging/1.2.7/logging-1.2.7.jar"),
            libs.join("org/slf4j/slf4j-api/2.0.17/slf4j-api-2.0.17.jar"),
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
            "org/slf4j/slf4j-api/2.0.17/slf4j-api-2.0.17.jar",
            "net/minecrell/terminalconsoleappender/1.2.0/terminalconsoleappender-1.2.0.jar",
            "org/jline/jline-terminal/3.25.1/jline-terminal-3.25.1.jar",
            "org/jline/jline-reader/3.25.1/jline-reader-3.25.1.jar",
        ];
        for rel in core_rel_paths {
            let path = libs.join(rel);
            if !accept_forge_jar(&path, &mut jar_validation_cache, &mut canonical_path_cache) {
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

    // Agent 通过明确的 JVM 参数注入，避免 JAVA_TOOL_OPTIONS 重复注入。
    cmd.env_remove("JAVA_TOOL_OPTIONS");

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
        if accept_forge_jar(
            candidate,
            &mut jar_validation_cache,
            &mut canonical_path_cache,
        ) && !real_cp.contains(path)
        {
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

        if accept_forge_jar(&path, &mut jar_validation_cache, &mut canonical_path_cache)
            && !real_cp.contains(&value)
        {
            real_cp.push(value);
        }
    }

    // Avoid discovering Bootstrap API a second time from java.class.path.
    // ASM 9.9.1 is supplied explicitly through --module-path.
    // Do not let sandbox ASM 9.10.1 enter Bootstrap's runtime module discovery.
    for p in real_cp
        .iter()
        .filter(|p| p.contains("mc_libraries/org/ow2/asm/"))
    {
        eprintln!("[forge] 排除沙盒 ASM，避免 JPMS 模块冲突: {}", p);
    }

    // Fabric Loader registers Mixin service providers that require
    // FabricLauncherBase initialization. They must never enter Forge's classpath.
    for p in real_cp.iter().filter(|p| p.contains("/net/fabricmc/")) {
        eprintln!("[forge] 排除 Fabric 专属依赖: {}", p);
    }

    real_cp.retain(|p| {
        !p.contains("bootstrap-api")
            && !p.contains("jopt-simple/6.0-alpha-3/")
            && !p.contains("mc_libraries/org/ow2/asm/")
            // Fabric Loader and Fabric-specific services are incompatible
            // with the standalone Forge ModLauncher environment.
            && !p.contains("/net/fabricmc/")
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

        let jar_canonical = cached_canonicalize(&jar, &mut canonical_path_cache);
        if !real_cp
            .iter()
            .any(|p| cached_canonicalize(Path::new(p), &mut canonical_path_cache) == jar_canonical)
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
        let key = cached_canonicalize(Path::new(p), &mut canonical_path_cache)
            .unwrap_or_else(|| PathBuf::from(p));
        seen_forge_cp.insert(key)
    });
    // ===== END FORGE_RUNTIME_MODULES_PATCH =====

    // Use the official ModLauncher JAR; sanitize_jar is disabled.
    // Do not refer to a patched JAR that is not generated.
    if accept_forge_jar(
        &modlauncher_path,
        &mut jar_validation_cache,
        &mut canonical_path_cache,
    ) {
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
        .arg("-Dfml.ignorePatchDiscrepancies=true");

    // Java Agent：先验证 Forge 能注册到 DataBus，再实现 Forge 实体采样。
    let project_root = env::current_dir().expect("[forge] 无法获取项目目录");
    let window_runtime = window_integration::WindowRuntime::start_or_connect(&project_root)
        .expect("[WindowIntegration] Forge 无法启动或连接 Window Manager");
    cmd.arg(window_runtime.java_property_arg());
    cmd.arg(
        window_runtime
            .bridge_agent_arg("forge")
            .expect("[WindowIntegration] 无法生成 Forge Bridge 参数"),
    );
    let agent_jar = project_root.join("bus_test/bus-agent.jar");

    if agent_jar.is_file() {
        let agent_arg = format!("-javaagent:{}=Forge-Sandbox", agent_jar.display());
        cmd.arg(agent_arg);
        eprintln!(
            "[forge] Java Agent 已加入 JVM 参数: {}",
            agent_jar.display()
        );
    } else {
        eprintln!("[forge] WARNING: Agent JAR 不存在: {}", agent_jar.display());
    }

    cmd.arg("net.minecraftforge.bootstrap.ForgeBootstrap")
        .arg("--launchTarget")
        .arg("forge_client")
        .arg("--fml.forgeVersion")
        .arg("54.1.22")
        .arg("--fml.mcVersion")
        .arg("1.21.4")
        .arg("--fml.mcpVersion")
        .arg("20241203.143248")
        .arg("--fml.forgeGroup")
        .arg("net.minecraftforge")
        .arg("--gameDir")
        .arg(sandbox_dir.to_string_lossy().to_string())
        .arg("--assetsDir")
        .arg(assets_dir.to_string_lossy().to_string())
        .arg("--assetIndex")
        .arg(&asset_index)
        .arg("--version")
        .arg("1.21.4")
        .arg("--username")
        .arg("Dev")
        .arg("--uuid")
        .arg("00000000000000000000000000000000")
        .arg("--accessToken")
        .arg("0")
        .arg("--userType")
        .arg("legacy")
        .arg("--versionType")
        .arg("release");

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

fn push_unique_ordered<T>(items: &mut Vec<T>, seen: &mut HashSet<T>, item: T)
where
    T: Eq + std::hash::Hash + Clone,
{
    if seen.insert(item.clone()) {
        items.push(item);
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
    let mut classpath_seen: HashSet<String> = HashSet::new();

    let mut native_jars: Vec<PathBuf> = Vec::new();
    let mut native_jars_seen: HashSet<PathBuf> = HashSet::new();

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
                if path.is_file() && native_jars_seen.insert(path.clone()) {
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
                push_unique_ordered(&mut classpath, &mut classpath_seen, value);
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

    let agent_jar = resolve_agent_jar(&project_root);

    let mut cmd = Command::new(&java);
    cmd.current_dir(&mc_root);
    let window_runtime = window_integration::WindowRuntime::start_or_connect(&project_root)
        .expect("[WindowIntegration] NeoForge 无法启动或连接 Window Manager");
    cmd.arg(window_runtime.java_property_arg());
    cmd.arg(
        window_runtime
            .bridge_agent_arg("neoforge")
            .expect("[WindowIntegration] 无法生成 NeoForge Bridge 参数"),
    );

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
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    };

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
                let connection_id = next_connection_id.fetch_add(1, Ordering::Relaxed);

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
                        "player_move" | "monster_spawn" | "monster_update" | "monster_remove"
                    );

                    if fabric_only_message && from != "fabric" {
                        println!(
                            "[DataBus][REJECT] '{}' attempted to publish {}",
                            from, message_type
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
                        .map(|registration| registration.connection_id == connection_id)
                        .unwrap_or(false);

                    if is_current_connection {
                        reg.remove(&name);
                        println!("[DataBus] '{}' 断开，已清理当前注册项", name);
                    } else {
                        println!("[DataBus] '{}' 旧连接断开，保留新连接注册项", name);
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

    let is_window_dry_run = args
        .windows(2)
        .any(|pair| pair[0] == "--mode" && pair[1].eq_ignore_ascii_case("window-dry-run"))
        || args.iter().any(|arg| {
            arg.eq_ignore_ascii_case("window-dry-run")
                || arg.eq_ignore_ascii_case("--mode=window-dry-run")
        });

    if is_window_dry_run {
        use window_layout::{LayoutConstraints, LayoutDirection};

        println!("=== Window Layout Dry Run (read-only) ===");

        let invalid_argument = |message: &str| -> ! {
            eprintln!("[WindowDryRun] {message}");
            std::process::exit(2);
        };

        let mut window_ids: Option<Vec<u32>> = None;
        let mut direction_value: Option<String> = None;
        let mut display_id: Option<u32> = None;
        let mut mode_seen = false;

        let mut arg_index = 1;
        while arg_index < args.len() {
            let arg = args[arg_index].as_str();

            if arg == "--mode" {
                if mode_seen {
                    invalid_argument("Duplicate mode selector.");
                }

                let Some(value) = args.get(arg_index + 1) else {
                    invalid_argument("Missing value after --mode.");
                };

                if !value.eq_ignore_ascii_case("window-dry-run") {
                    invalid_argument("Unexpected mode value.");
                }

                mode_seen = true;
                arg_index += 2;
                continue;
            }

            if let Some(value) = arg.strip_prefix("--mode=") {
                if mode_seen {
                    invalid_argument("Duplicate mode selector.");
                }

                if !value.eq_ignore_ascii_case("window-dry-run") {
                    invalid_argument("Unexpected mode value.");
                }

                mode_seen = true;
                arg_index += 1;
                continue;
            }

            if arg.eq_ignore_ascii_case("window-dry-run") {
                if mode_seen {
                    invalid_argument("Duplicate mode selector.");
                }

                mode_seen = true;
                arg_index += 1;
                continue;
            }

            let (name, value, consumed) = if let Some((name, value)) = arg.split_once('=') {
                (name, value, 1)
            } else {
                let Some(value) = args.get(arg_index + 1) else {
                    invalid_argument("Missing value for option.");
                };

                if value.starts_with("--") {
                    invalid_argument("Missing value before the next option.");
                }

                (arg, value.as_str(), 2)
            };

            if value.is_empty() {
                invalid_argument("Option values must not be empty.");
            }

            match name {
                "--window-ids" => {
                    if window_ids.is_some() {
                        invalid_argument("Duplicate --window-ids option.");
                    }

                    let parsed: Result<Vec<u32>, _> = value
                        .split(',')
                        .map(|part| part.trim().parse::<u32>())
                        .collect();

                    match parsed {
                        Ok(ids) if ids.len() >= 2 && ids.iter().all(|id| *id > 0) => {
                            window_ids = Some(ids);
                        }
                        _ => invalid_argument(
                            "Invalid --window-ids. Provide at least two comma-separated positive IDs.",
                        ),
                    }
                }
                "--direction" => {
                    if direction_value.is_some() {
                        invalid_argument("Duplicate --direction option.");
                    }

                    if !value.eq_ignore_ascii_case("horizontal")
                        && !value.eq_ignore_ascii_case("vertical")
                    {
                        invalid_argument("Invalid --direction. Expected horizontal or vertical.");
                    }

                    direction_value = Some(value.to_owned());
                }
                "--display-id" => {
                    if display_id.is_some() {
                        invalid_argument("Duplicate --display-id option.");
                    }

                    match value.parse::<u32>() {
                        Ok(id) if id > 0 => display_id = Some(id),
                        _ => invalid_argument(
                            "Invalid --display-id. Expected a positive display ID.",
                        ),
                    }
                }
                _ => {
                    invalid_argument("Unknown option.");
                }
            }

            arg_index += consumed;
        }

        let window_ids = match window_ids {
            Some(ids) => ids,
            None => invalid_argument(
                "Missing --window-ids. Provide at least two comma-separated window IDs.",
            ),
        };

        let direction = match direction_value.as_deref().unwrap_or("horizontal") {
            value if value.eq_ignore_ascii_case("horizontal") => LayoutDirection::Horizontal,
            value if value.eq_ignore_ascii_case("vertical") => LayoutDirection::Vertical,
            _ => unreachable!(),
        };

        let constraints = LayoutConstraints::default();

        match window_dry_run::build_window_dry_run(&window_ids, direction, display_id, constraints)
        {
            Ok(report) => {
                println!("[WindowDryRun] Target display: {}", report.display_id);
                println!("[WindowDryRun] Direction: {}", report.direction);
                println!("[WindowDryRun] Windows: {}", report.entries.len());
                println!(
                    "[WindowDryRun] Constraints: margin=({}, {}) gap={} min={}x{}",
                    constraints.margin_x,
                    constraints.margin_y,
                    constraints.gap,
                    constraints.min_width,
                    constraints.min_height,
                );

                for entry in &report.entries {
                    let current = &entry.current_bounds;
                    let target = &entry.target_bounds;

                    println!();
                    println!(
                        "[Window {}] pid={} owner={:?}",
                        entry.window_id, entry.pid, entry.owner_name
                    );
                    println!("[Window {}] title={:?}", entry.window_id, entry.title);
                    println!(
                        "[Window {}] AX index={}",
                        entry.window_id, entry.accessibility_index
                    );
                    println!(
                        "[Window {}] current=({:.2}, {:.2}, {:.2}, {:.2})",
                        entry.window_id, current.x, current.y, current.width, current.height,
                    );
                    println!(
                        "[Window {}] target=({:.2}, {:.2}, {:.2}, {:.2})",
                        entry.window_id, target.x, target.y, target.width, target.height,
                    );
                    println!("[Window {}] identity=VERIFIED", entry.window_id);
                }

                println!();
                println!("[WindowDryRun] All selected windows passed verification.");
                println!("[WindowDryRun] DRY RUN ONLY: no window properties were changed.");
            }
            Err(error) => {
                eprintln!("[WindowDryRun] ABORTED: {error}");
                eprintln!("[WindowDryRun] No window properties were changed.");
                std::process::exit(1);
            }
        }

        return;
    }

    let is_window_verify = args
        .windows(2)
        .any(|pair| pair[0] == "--mode" && pair[1].eq_ignore_ascii_case("window-verify"))
        || args.iter().any(|arg| {
            arg.eq_ignore_ascii_case("window-verify")
                || arg.eq_ignore_ascii_case("--mode=window-verify")
        });

    if is_window_verify {
        println!("=== Window Identity Verification (read-only) ===");

        let parse_option = |name: &str| -> Option<&str> {
            args.windows(2)
                .find(|pair| pair[0] == name)
                .map(|pair| pair[1].as_str())
                .or_else(|| {
                    args.iter()
                        .find_map(|arg| arg.strip_prefix(&format!("{name}=")))
                })
        };

        let window_id =
            match parse_option("--window-id").and_then(|value| value.parse::<u32>().ok()) {
                Some(id) if id > 0 => id,
                _ => {
                    eprintln!("[WindowVerify] Usage: --mode window-verify --window-id <WINDOW_ID>");
                    std::process::exit(2);
                }
            };

        let verification = (|| -> Result<(), String> {
            let discovered = window_discovery::discover_windows()
                .map_err(|error| format!("CoreGraphics discovery failed: {error}"))?;

            let initial_matches: Vec<_> = discovered
                .iter()
                .filter(|window| window.window_id == window_id)
                .collect();

            if initial_matches.len() != 1 {
                return Err(format!(
                    "expected exactly one CoreGraphics window with ID {window_id}, found {}",
                    initial_matches.len()
                ));
            }

            let target = initial_matches[0];

            if target.pid == 0 {
                return Err("target window has an invalid PID".to_owned());
            }

            if target.title.as_deref().is_none_or(str::is_empty) {
                return Err("target window has no usable title".to_owned());
            }

            let bounds = &target.bounds;
            if [bounds.x, bounds.y, bounds.width, bounds.height]
                .iter()
                .any(|value| !value.is_finite())
                || bounds.width <= 0.0
                || bounds.height <= 0.0
            {
                return Err("target window has invalid geometry".to_owned());
            }

            println!(
                "[WindowVerify] Initial identity: id={} pid={} owner={:?} title={:?}",
                target.window_id, target.pid, target.owner_name, target.title
            );
            println!(
                "[WindowVerify] Initial bounds: ({:.1}, {:.1}, {:.1}, {:.1})",
                bounds.x, bounds.y, bounds.width, bounds.height
            );

            let accessibility_windows =
                window_accessibility::inspect_application_windows(target.pid)
                    .map_err(|error| format!("Accessibility inspection failed: {error}"))?;

            let accessibility_index =
                window_accessibility::match_accessibility_window(target, &accessibility_windows)
                    .map_err(|error| format!("Accessibility identity match failed: {error}"))?;

            let refreshed = window_discovery::discover_windows()
                .map_err(|error| format!("Final CoreGraphics discovery failed: {error}"))?;

            let final_matches: Vec<_> = refreshed
                .iter()
                .filter(|window| window.window_id == window_id)
                .collect();

            if final_matches.len() != 1 {
                return Err(format!(
                    "target window changed or disappeared during verification; final matches: {}",
                    final_matches.len()
                ));
            }

            let current = final_matches[0];

            if current.pid != target.pid || current.title != target.title {
                return Err("target window identity changed during verification".to_owned());
            }

            let current_bounds = &current.bounds;
            let geometry_stable = [
                (current_bounds.x, bounds.x),
                (current_bounds.y, bounds.y),
                (current_bounds.width, bounds.width),
                (current_bounds.height, bounds.height),
            ]
            .iter()
            .all(|(actual, expected)| actual.is_finite() && (actual - expected).abs() <= 2.0)
                && current_bounds.width > 0.0
                && current_bounds.height > 0.0;

            if !geometry_stable {
                return Err("target window geometry changed during verification".to_owned());
            }

            println!("[WindowVerify] Accessibility match: AX index={accessibility_index}");
            println!(
                "[WindowVerify] Final bounds: ({:.1}, {:.1}, {:.1}, {:.1})",
                current_bounds.x, current_bounds.y, current_bounds.width, current_bounds.height
            );
            println!("[WindowVerify] Identity and geometry remained stable.");
            println!("[WindowVerify] READ-ONLY: no window properties were changed.");

            Ok(())
        })();

        if let Err(error) = verification {
            eprintln!("[WindowVerify] FAILED: {error}");
            std::process::exit(1);
        }

        return;
    }

    let is_window_layout = args
        .windows(2)
        .any(|pair| pair[0] == "--mode" && pair[1].eq_ignore_ascii_case("window-layout"))
        || args.iter().any(|arg| {
            arg.eq_ignore_ascii_case("window-layout")
                || arg.eq_ignore_ascii_case("--mode=window-layout")
        });

    if is_window_layout {
        use window_layout::{
            LayoutConstraints, LayoutDirection, plan_on_display_id, plan_on_main_display,
        };

        println!("=== Window Layout Planner (read-only) ===");

        let parse_option = |name: &str| -> Option<&str> {
            args.windows(2)
                .find(|pair| pair[0] == name)
                .map(|pair| pair[1].as_str())
                .or_else(|| {
                    args.iter()
                        .find_map(|arg| arg.strip_prefix(&format!("{name}=")))
                })
        };

        let window_count = match parse_option("--count") {
            Some(value) => match value.parse::<usize>() {
                Ok(count) if count > 0 => count,
                _ => {
                    eprintln!("[WindowLayout] Invalid --count. Expected a positive integer.");
                    std::process::exit(2);
                }
            },
            None => 2,
        };

        let direction = match parse_option("--direction").unwrap_or("horizontal") {
            value if value.eq_ignore_ascii_case("horizontal") => LayoutDirection::Horizontal,
            value if value.eq_ignore_ascii_case("vertical") => LayoutDirection::Vertical,
            _ => {
                eprintln!("[WindowLayout] Invalid --direction. Use horizontal or vertical.");
                std::process::exit(2);
            }
        };

        let display_id = match parse_option("--display-id") {
            Some(value) => match value.parse::<u32>() {
                Ok(id) if id > 0 => Some(id),
                _ => {
                    eprintln!("[WindowLayout] Invalid --display-id.");
                    std::process::exit(2);
                }
            },
            None => None,
        };

        let displays = match display_geometry::discover_displays() {
            Ok(displays) => displays,
            Err(error) => {
                eprintln!("[WindowLayout] Display discovery failed: {error}");
                std::process::exit(1);
            }
        };

        let constraints = LayoutConstraints::default();

        println!("[WindowLayout] Direction: {direction}");
        println!("[WindowLayout] Window count: {window_count}");
        println!(
            "[WindowLayout] Constraints: margin=({}, {}) gap={} min={}x{}",
            constraints.margin_x,
            constraints.margin_y,
            constraints.gap,
            constraints.min_width,
            constraints.min_height
        );

        let result = match display_id {
            Some(id) => plan_on_display_id(&displays, id, window_count, direction, constraints),
            None => plan_on_main_display(&displays, window_count, direction, constraints),
        };

        match result {
            Ok(plan) => {
                println!("[WindowLayout] Target display: {}", plan.display_id);
                for placement in &plan.placements {
                    let b = &placement.bounds;
                    println!(
                        "[Placement {}] x={:.2} y={:.2} width={:.2} height={:.2}",
                        placement.index, b.x, b.y, b.width, b.height
                    );
                }
                println!("[WindowLayout] Planning completed. No windows were modified.");
            }
            Err(error) => {
                eprintln!("[WindowLayout] Planning failed: {error}");
                std::process::exit(1);
            }
        }

        return;
    }

    let is_window_display = args
        .windows(2)
        .any(|pair| pair[0] == "--mode" && pair[1].eq_ignore_ascii_case("window-display"))
        || args.iter().any(|arg| {
            arg.eq_ignore_ascii_case("window-display")
                || arg.eq_ignore_ascii_case("--mode=window-display")
        });

    if is_window_display {
        println!("=== macOS Display Geometry Probe (read-only) ===");

        match display_geometry::discover_displays() {
            Ok(displays) => {
                println!("[DisplayGeometry] Found {} active displays", displays.len());

                for (index, display) in displays.iter().enumerate() {
                    println!(
                        "[Display {}] id={} main={} builtin={} rotation={:.1}deg logical=({:.1}, {:.1}, {:.1}, {:.1}) backing_pixels={}x{} cg_pixels={}x{} scale=({:.4}, {:.4})",
                        index + 1,
                        display.display_id,
                        display.is_main,
                        display.is_builtin,
                        display.rotation_degrees,
                        display.bounds.x,
                        display.bounds.y,
                        display.bounds.width,
                        display.bounds.height,
                        display.pixel_width,
                        display.pixel_height,
                        display.cg_pixel_width,
                        display.cg_pixel_height,
                        display.scale_x,
                        display.scale_y,
                    );
                }

                println!();
                println!("=== Display Topology ===");

                match display_topology::analyze_displays(&displays) {
                    Ok(topology) => {
                        println!("[Topology] Main display ID: {}", topology.main_display_id);
                        println!(
                            "[VirtualDesktop] bounds=({:.1}, {:.1}, {:.1}, {:.1})",
                            topology.virtual_bounds.x,
                            topology.virtual_bounds.y,
                            topology.virtual_bounds.width,
                            topology.virtual_bounds.height,
                        );
                        for offset in &topology.main_relative_offsets {
                            println!(
                                "[DisplayOffset] id={} relative_to_main=({:+.1}, {:+.1})",
                                offset.display_id, offset.offset_x, offset.offset_y,
                            );
                        }

                        if topology.relations.is_empty() {
                            println!("[Topology] No display pairs to analyze");
                        }

                        for relation in &topology.relations {
                            let position = match relation.relative_position {
                                display_topology::RelativePosition::Left => "Left",
                                display_topology::RelativePosition::Right => "Right",
                                display_topology::RelativePosition::Above => "Above",
                                display_topology::RelativePosition::Below => "Below",
                                display_topology::RelativePosition::Diagonal => "Diagonal",
                                display_topology::RelativePosition::Overlapping => "Overlapping",
                            };

                            println!(
                                "[Pair {} -> {}] position={} horizontal_gap={:+.1} vertical_gap={:+.1} overlap={}x{}",
                                relation.first_id,
                                relation.second_id,
                                position,
                                relation.horizontal_gap,
                                relation.vertical_gap,
                                relation.overlap_width,
                                relation.overlap_height,
                            );
                        }
                    }
                    Err(error) => {
                        eprintln!("[Topology] ERROR: {error}");
                    }
                }
            }
            Err(error) => {
                eprintln!("[DisplayGeometry] ERROR: {error}");
                std::process::exit(1);
            }
        }

        return;
    }

    let is_window_accessibility = args
        .windows(2)
        .any(|pair| pair[0] == "--mode" && pair[1].eq_ignore_ascii_case("window-accessibility"))
        || args.iter().any(|arg| {
            arg.eq_ignore_ascii_case("window-accessibility")
                || arg.eq_ignore_ascii_case("--mode=window-accessibility")
        });

    if is_window_accessibility {
        println!("=== macOS Accessibility Probe (read-only) ===");

        let pid_value = args
            .windows(2)
            .find(|pair| pair[0] == "--pid")
            .map(|pair| pair[1].as_str())
            .or_else(|| args.iter().find_map(|arg| arg.strip_prefix("--pid=")));

        let pid = match pid_value.and_then(|value| value.parse::<u32>().ok()) {
            Some(pid) if pid > 0 => pid,
            _ => {
                eprintln!("[Accessibility] Usage: --mode window-accessibility --pid <PID>");
                std::process::exit(2);
            }
        };

        match window_accessibility::is_accessibility_trusted() {
            Ok(true) => {
                println!("[Accessibility] Permission: granted");
            }
            Ok(false) => {
                println!("[Accessibility] Permission: not granted");
                println!(
                    "[Accessibility] Authorize the terminal or application in System Settings > Privacy & Security > Accessibility."
                );
                return;
            }
            Err(error) => {
                eprintln!("[Accessibility] Permission check failed: {error}");
                std::process::exit(1);
            }
        }

        println!("[Accessibility] Target PID: {pid}");

        match window_accessibility::inspect_application_windows(pid) {
            Ok(windows) => {
                println!(
                    "[Accessibility] Found {} application windows",
                    windows.len()
                );

                for window in &windows {
                    match &window.bounds {
                        Some(bounds) => println!(
                            "[AX Window {}] title={:?} position=({:.1}, {:.1}) size=({:.1}, {:.1})",
                            window.index,
                            window.title.as_deref().unwrap_or("<untitled>"),
                            bounds.x,
                            bounds.y,
                            bounds.width,
                            bounds.height,
                        ),
                        None => println!(
                            "[AX Window {}] title={:?} bounds=<unavailable>",
                            window.index,
                            window.title.as_deref().unwrap_or("<untitled>"),
                        ),
                    }
                }

                if windows.is_empty() {
                    println!("[Accessibility] No application windows returned.");
                }
            }
            Err(error) => {
                eprintln!("[Accessibility] Inspection failed: {error}");
                std::process::exit(1);
            }
        }

        return;
    }

    let is_window_discovery = args
        .windows(2)
        .any(|pair| pair[0] == "--mode" && pair[1].eq_ignore_ascii_case("window-discovery"))
        || args.iter().any(|arg| {
            arg.eq_ignore_ascii_case("window-discovery")
                || arg.eq_ignore_ascii_case("--mode=window-discovery")
        });

    if is_window_discovery {
        println!("=== macOS Window Discovery (read-only) ===");

        match window_discovery::discover_windows() {
            Ok(windows) => {
                println!(
                    "[WindowDiscovery] Found {} on-screen windows",
                    windows.len()
                );

                for (index, window) in windows.iter().enumerate() {
                    println!(
                        "[Window {}] id={} pid={} owner={:?} title={:?} bounds=({:.1}, {:.1}, {:.1}, {:.1})",
                        index + 1,
                        window.window_id,
                        window.pid,
                        window.owner_name.as_deref().unwrap_or("<unknown>"),
                        window.title.as_deref().unwrap_or("<untitled>"),
                        window.bounds.x,
                        window.bounds.y,
                        window.bounds.width,
                        window.bounds.height,
                    );
                }

                if windows.is_empty() {
                    println!("[WindowDiscovery] No eligible on-screen windows were returned.");
                }
            }
            Err(error) => {
                eprintln!("[WindowDiscovery] ERROR: {error}");
                std::process::exit(1);
            }
        }

        return;
    }

    let is_fusion = args
        .windows(2)
        .any(|pair| pair[0] == "--mode" && pair[1].eq_ignore_ascii_case("fusion"))
        || args.iter().any(|arg| {
            arg.eq_ignore_ascii_case("fusion") || arg.eq_ignore_ascii_case("--mode=fusion")
        });
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
        let window_runtime = window_integration::WindowRuntime::start_local(&current_dir)
            .expect("[WindowIntegration] Fusion 无法启动 Window Manager");
        let window_addr = window_runtime.address_string();

        // 启动 Fabric
        let _sandbox_fab = sandbox_dir.clone();
        let fab_window_addr = window_addr.clone();

        let _fab_thread = thread::spawn(move || {
            let mut cmd = std::process::Command::new("cargo");

            // Agent 由 Fabric JVM 的 jvm_args 显式注入。
            // 不要再通过 JAVA_TOOL_OPTIONS 注入第二次。
            cmd.args(&["run", "--", "--mode", "fabric"]);

            cmd.env("JVM_COMBINER_BUS_EXTERNAL", "1");
            cmd.env("JVM_COMBINER_WINDOW_EXTERNAL", "1");
            cmd.env("JVM_COMBINER_WINDOW_ADDR", fab_window_addr);
            cmd.status().unwrap();
        });

        // 启动 NeoForge
        let _sandbox_nf = sandbox_dir.clone();
        let nf_window_addr = window_addr.clone();
        let _nf_thread = thread::spawn(move || {
            let mut cmd = std::process::Command::new("cargo");

            cmd.args(&["run", "--", "--mode", "neoforge"]);
            cmd.env("JVM_COMBINER_BUS_EXTERNAL", "1");
            cmd.env("JVM_COMBINER_WINDOW_EXTERNAL", "1");
            cmd.env("JVM_COMBINER_WINDOW_ADDR", nf_window_addr);
            cmd.status().unwrap();
        });

        // Read-only window monitoring. The Fusion parent owns the
        // Window Manager and is therefore the authoritative registry reader.
        let mut last_registered_windows = Vec::new();
        let mut discovery_failed = false;

        loop {
            match window_discovery::discover_windows() {
                Ok(windows) => {
                    discovery_failed = false;

                    let registrations = window_runtime.registrations();
                    let mut registered_windows =
                        window_discovery::associate_windows(windows, &registrations);

                    registered_windows.retain(|record| {
                        record.loader.is_some()
                            && record.role.as_deref() == Some("minecraft_client")
                    });

                    if registered_windows != last_registered_windows {
                        eprintln!(
                            "[WindowDiscovery] Registered Minecraft windows: {}",
                            registered_windows.len()
                        );

                        for record in &registered_windows {
                            eprintln!(
                                "[MinecraftWindow] id={} pid={} loader={} title={:?} bounds=({}, {}, {}, {})",
                                record.window.window_id,
                                record.window.pid,
                                record.loader.as_deref().unwrap_or("unknown"),
                                record.window.title,
                                record.window.bounds.x,
                                record.window.bounds.y,
                                record.window.bounds.width,
                                record.window.bounds.height,
                            );
                        }

                        last_registered_windows = registered_windows;
                    }
                }
                Err(error) => {
                    if !discovery_failed {
                        eprintln!("[WindowDiscovery] Discovery failed: {error}");
                        discovery_failed = true;
                    }
                }
            }

            thread::sleep(std::time::Duration::from_secs(2));
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
        if env::var_os("JVM_COMBINER_BUS_EXTERNAL").is_none() {
            start_data_bus().expect("[DataBus] Forge 无法监听 127.0.0.1:25565");
        }

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

    let project_root = env::current_dir().expect("无法获取项目目录");
    let window_runtime = window_integration::WindowRuntime::start_or_connect(&project_root)
        .expect("[WindowIntegration] Fabric 无法启动或连接 Window Manager");
    jvm_args.push(window_runtime.java_property_arg());
    jvm_args.push(
        window_runtime
            .bridge_agent_arg("fabric")
            .expect("[WindowIntegration] 无法生成 Fabric Bridge 参数"),
    );
    let agent_jar = resolve_agent_jar(&project_root);
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

#[cfg(test)]
mod classpath_resolver_tests {
    use super::resolve_classpath;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestTree {
        root: PathBuf,
    }

    impl TestTree {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock error")
                .as_nanos();

            let root = std::env::temp_dir().join(format!(
                "jvm-combiner-r1-test-{}-{}",
                std::process::id(),
                nonce
            ));

            fs::create_dir_all(&root).expect("create test root");

            Self { root }
        }

        fn artifact(&self, relative: &str) -> PathBuf {
            let path = self.root.join(relative);

            fs::create_dir_all(path.parent().expect("artifact parent directory"))
                .expect("create artifact directory");

            fs::write(&path, b"test artifact").expect("write artifact");
            path
        }

        fn version(&self, id: &str, parent: Option<&str>, libraries: &[(&str, &str)]) -> PathBuf {
            let version_dir = self.root.join("versions").join(id);
            fs::create_dir_all(&version_dir).expect("create version directory");

            let library_values: Vec<_> = libraries
                .iter()
                .map(|(name, artifact_path)| {
                    json!({
                        "name": name,
                        "downloads": {
                            "artifact": {
                                "path": artifact_path
                            }
                        }
                    })
                })
                .collect();

            let mut document = json!({
                "libraries": library_values
            });

            if let Some(parent_id) = parent {
                document["inheritsFrom"] = json!(parent_id);
            }

            let json_path = version_dir.join(format!("{id}.json"));

            fs::write(
                &json_path,
                serde_json::to_vec_pretty(&document).expect("serialize version JSON"),
            )
            .expect("write version JSON");

            json_path
        }

        fn search_dirs(&self) -> Vec<PathBuf> {
            vec![self.root.clone()]
        }
    }

    impl Drop for TestTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn child_dependency_overrides_parent_dependency() {
        let tree = TestTree::new();

        let child_artifact = tree.artifact("libraries/example/child.jar");
        tree.artifact("libraries/example/parent.jar");

        tree.version(
            "parent",
            None,
            &[("example:library:1.0", "libraries/example/parent.jar")],
        );

        let child_json = tree.version(
            "child",
            Some("parent"),
            &[("example:library:2.0", "libraries/example/child.jar")],
        );

        let result = resolve_classpath(&child_json, &tree.search_dirs());

        assert_eq!(result, vec![child_artifact]);
    }

    #[test]
    fn child_dependency_overrides_parent_even_when_artifact_path_is_deduplicated() {
        let tree = TestTree::new();

        let shared_artifact = tree.artifact("libraries/shared/library.jar");
        tree.artifact("libraries/example/parent.jar");

        tree.version(
            "parent",
            None,
            &[("example:library:1.0", "libraries/example/parent.jar")],
        );

        let child_json = tree.version(
            "child",
            Some("parent"),
            &[
                ("example:other:1.0", "libraries/shared/library.jar"),
                ("example:library:2.0", "libraries/shared/library.jar"),
            ],
        );

        let result = resolve_classpath(&child_json, &tree.search_dirs());

        assert_eq!(result, vec![shared_artifact]);
    }

    #[test]
    fn missing_child_artifact_falls_back_to_parent() {
        let tree = TestTree::new();

        let parent_artifact = tree.artifact("libraries/example/parent.jar");

        tree.version(
            "parent",
            None,
            &[("example:library:1.0", "libraries/example/parent.jar")],
        );

        let child_json = tree.version(
            "child",
            Some("parent"),
            &[("example:library:2.0", "libraries/example/missing.jar")],
        );

        let result = resolve_classpath(&child_json, &tree.search_dirs());

        assert_eq!(result, vec![parent_artifact]);
    }

    #[test]
    fn duplicate_artifact_paths_are_emitted_once() {
        let tree = TestTree::new();

        let shared_artifact = tree.artifact("libraries/shared/library.jar");

        let child_json = tree.version(
            "child",
            None,
            &[
                ("example:first:1.0", "libraries/shared/library.jar"),
                ("example:second:1.0", "libraries/shared/library.jar"),
            ],
        );

        let result = resolve_classpath(&child_json, &tree.search_dirs());

        assert_eq!(result, vec![shared_artifact]);
    }

    #[test]
    fn inheritance_preserves_child_parent_grandparent_order() {
        let tree = TestTree::new();

        let child_artifact = tree.artifact("libraries/order/child.jar");
        let parent_artifact = tree.artifact("libraries/order/parent.jar");
        let grandparent_artifact = tree.artifact("libraries/order/grandparent.jar");

        tree.version(
            "grandparent",
            None,
            &[("example:grandparent:1.0", "libraries/order/grandparent.jar")],
        );

        tree.version(
            "parent",
            Some("grandparent"),
            &[("example:parent:1.0", "libraries/order/parent.jar")],
        );

        let child_json = tree.version(
            "child",
            Some("parent"),
            &[("example:child:1.0", "libraries/order/child.jar")],
        );

        let result = resolve_classpath(&child_json, &tree.search_dirs());

        assert_eq!(
            result,
            vec![child_artifact, parent_artifact, grandparent_artifact,]
        );
    }

    #[test]
    fn inheritance_cycle_terminates_and_keeps_resolved_dependencies() {
        let tree = TestTree::new();

        let child_artifact = tree.artifact("libraries/cycle/a.jar");
        let parent_artifact = tree.artifact("libraries/cycle/b.jar");

        let version_a = tree.version(
            "a",
            Some("b"),
            &[("example:a:1.0", "libraries/cycle/a.jar")],
        );

        tree.version(
            "b",
            Some("a"),
            &[("example:b:1.0", "libraries/cycle/b.jar")],
        );

        let result = resolve_classpath(&version_a, &tree.search_dirs());

        assert_eq!(result, vec![child_artifact, parent_artifact]);
    }
}

#[cfg(test)]
mod forge_jar_cache_tests {
    use super::cached_jar_validation;
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestFile {
        root: PathBuf,
        path: PathBuf,
    }

    impl TestFile {
        fn new(name: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock error")
                .as_nanos();

            let root = std::env::temp_dir().join(format!(
                "jvm-combiner-r2-cache-{}-{}",
                std::process::id(),
                nonce
            ));

            fs::create_dir_all(&root).expect("create test directory");

            let path = root.join(name);
            fs::write(&path, b"test").expect("create test file");

            Self { root, path }
        }
    }

    impl Drop for TestFile {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn canonicalization_results_are_cached_for_repeated_paths() {
        let file = TestFile::new("cached.jar");
        let mut cache = HashMap::new();

        let first = super::cached_canonicalize(&file.path, &mut cache);
        let second = super::cached_canonicalize(&file.path, &mut cache);

        assert_eq!(first, second);
        assert_eq!(cache.len(), 1);
        assert_eq!(first, Some(file.path.canonicalize().unwrap()));
    }

    #[test]
    fn canonicalization_failures_are_cached_as_none() {
        let file = TestFile::new("created-later.jar");
        fs::remove_file(&file.path).expect("remove test file");

        let mut cache = HashMap::new();
        assert_eq!(super::cached_canonicalize(&file.path, &mut cache), None);

        fs::write(&file.path, b"created after first lookup").expect("recreate test file");

        assert_eq!(super::cached_canonicalize(&file.path, &mut cache), None);
        assert_eq!(cache.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn canonical_path_aliases_resolve_to_the_same_value() {
        let file = TestFile::new("alias-canonical.jar");
        let alias = file.root.join("alias-link.jar");
        std::os::unix::fs::symlink(&file.path, &alias).expect("create symlink alias");

        let mut cache = HashMap::new();

        let first = super::cached_canonicalize(&file.path, &mut cache);
        let second = super::cached_canonicalize(&alias, &mut cache);

        assert_ne!(file.path, alias);
        assert_eq!(first, second);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn two_failed_canonicalizations_preserve_none_equality() {
        let root = std::env::temp_dir().join(format!(
            "jvm-combiner-r3-missing-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock error")
                .as_nanos()
        ));
        let first = root.join("first.jar");
        let second = root.join("second.jar");
        let mut cache = HashMap::new();

        let first_result = super::cached_canonicalize(&first, &mut cache);
        let second_result = super::cached_canonicalize(&second, &mut cache);

        assert_eq!(first_result, None);
        assert_eq!(second_result, None);
        assert_eq!(first_result, second_result);
    }

    #[test]
    fn successful_result_is_validated_only_once() {
        let file = TestFile::new("valid.jar");
        let mut cache = HashMap::new();
        let mut canonical_path_cache = HashMap::new();
        let mut calls = 0;

        assert!(cached_jar_validation(
            &file.path,
            &mut cache,
            &mut canonical_path_cache,
            |_| {
                calls += 1;
                true
            }
        ));

        assert!(cached_jar_validation(
            &file.path,
            &mut cache,
            &mut canonical_path_cache,
            |_| {
                calls += 1;
                false
            }
        ));

        assert_eq!(calls, 1);
    }

    #[test]
    fn failed_result_is_validated_only_once() {
        let file = TestFile::new("invalid.jar");
        let mut cache = HashMap::new();
        let mut canonical_path_cache = HashMap::new();
        let mut calls = 0;

        assert!(!cached_jar_validation(
            &file.path,
            &mut cache,
            &mut canonical_path_cache,
            |_| {
                calls += 1;
                false
            }
        ));

        assert!(!cached_jar_validation(
            &file.path,
            &mut cache,
            &mut canonical_path_cache,
            |_| {
                calls += 1;
                true
            }
        ));

        assert_eq!(calls, 1);
    }

    #[test]
    fn different_paths_are_validated_independently() {
        let first = TestFile::new("first.jar");
        let second = TestFile::new("second.jar");
        let mut cache = HashMap::new();
        let mut canonical_path_cache = HashMap::new();
        let mut calls = 0;

        assert!(cached_jar_validation(
            &first.path,
            &mut cache,
            &mut canonical_path_cache,
            |_| {
                calls += 1;
                true
            }
        ));

        assert!(!cached_jar_validation(
            &second.path,
            &mut cache,
            &mut canonical_path_cache,
            |_| {
                calls += 1;
                false
            }
        ));

        assert_eq!(calls, 2);
    }

    #[cfg(unix)]
    #[test]
    fn canonical_path_aliases_share_one_cache_entry() {
        let file = TestFile::new("alias.jar");
        let alias = file.root.join("alias-link.jar");
        std::os::unix::fs::symlink(&file.path, &alias).expect("create symlink alias");

        let mut cache = HashMap::new();
        let mut canonical_path_cache = HashMap::new();
        let mut calls = 0;

        assert!(cached_jar_validation(
            &file.path,
            &mut cache,
            &mut canonical_path_cache,
            |_| {
                calls += 1;
                true
            }
        ));

        assert!(cached_jar_validation(
            Path::new(&alias),
            &mut cache,
            &mut canonical_path_cache,
            |_| {
                calls += 1;
                false
            }
        ));

        assert_eq!(calls, 1);
    }
}

#[cfg(test)]
mod neoforge_classpath_tests {
    use super::push_unique_ordered;
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn classpath_strings_are_deduplicated() {
        let mut items = Vec::new();
        let mut seen = HashSet::new();

        push_unique_ordered(&mut items, &mut seen, String::from("a.jar"));
        push_unique_ordered(&mut items, &mut seen, String::from("b.jar"));
        push_unique_ordered(&mut items, &mut seen, String::from("a.jar"));

        assert_eq!(items, vec!["a.jar", "b.jar"]);
        assert_eq!(seen.len(), 2);
    }

    #[test]
    fn distinct_path_strings_remain_distinct() {
        let mut items = Vec::new();
        let mut seen = HashSet::new();

        push_unique_ordered(&mut items, &mut seen, String::from("libs/a.jar"));
        push_unique_ordered(&mut items, &mut seen, String::from("./libs/a.jar"));

        assert_eq!(items, vec!["libs/a.jar", "./libs/a.jar"]);
        assert_eq!(seen.len(), 2);
    }

    #[test]
    fn native_paths_are_deduplicated() {
        let mut items = Vec::new();
        let mut seen = HashSet::new();

        push_unique_ordered(&mut items, &mut seen, PathBuf::from("natives/first.jar"));
        push_unique_ordered(&mut items, &mut seen, PathBuf::from("natives/second.jar"));
        push_unique_ordered(&mut items, &mut seen, PathBuf::from("natives/first.jar"));

        assert_eq!(
            items,
            vec![
                PathBuf::from("natives/first.jar"),
                PathBuf::from("natives/second.jar"),
            ]
        );
        assert_eq!(seen.len(), 2);
    }

    #[test]
    fn insertion_order_is_preserved() {
        let mut items = Vec::new();
        let mut seen = HashSet::new();

        for value in ["third.jar", "first.jar", "second.jar", "first.jar"] {
            push_unique_ordered(&mut items, &mut seen, String::from(value));
        }

        assert_eq!(items, vec!["third.jar", "first.jar", "second.jar"]);
    }
}

#[cfg(test)]
mod build_classpath_dedup_tests {
    use super::push_unique_ordered;
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn forge_scan_and_resolved_dependencies_share_dedup_state() {
        let mut items = Vec::new();
        let mut seen = HashSet::new();

        for path in ["scan-a.jar", "scan-b.jar"] {
            push_unique_ordered(&mut items, &mut seen, PathBuf::from(path));
        }

        for path in ["scan-b.jar", "resolved-c.jar", "scan-a.jar"] {
            push_unique_ordered(&mut items, &mut seen, PathBuf::from(path));
        }

        assert_eq!(
            items,
            vec![
                PathBuf::from("scan-a.jar"),
                PathBuf::from("scan-b.jar"),
                PathBuf::from("resolved-c.jar"),
            ]
        );
        assert_eq!(seen.len(), 3);
    }

    #[test]
    fn fabric_legacy_and_bootstrap_classpaths_deduplicate_independently() {
        let mut legacy = Vec::new();
        let mut bootstrap = Vec::new();
        let mut legacy_seen = HashSet::new();
        let mut bootstrap_seen = HashSet::new();

        let shared = PathBuf::from("shared.jar");

        push_unique_ordered(&mut legacy, &mut legacy_seen, shared.clone());
        push_unique_ordered(&mut legacy, &mut legacy_seen, shared.clone());

        push_unique_ordered(&mut bootstrap, &mut bootstrap_seen, shared.clone());
        push_unique_ordered(&mut bootstrap, &mut bootstrap_seen, shared.clone());

        assert_eq!(legacy, vec![shared.clone()]);
        assert_eq!(bootstrap, vec![shared]);
        assert_eq!(legacy_seen.len(), 1);
        assert_eq!(bootstrap_seen.len(), 1);
    }

    #[test]
    fn fabric_minecraft_jar_membership_is_checked_per_classpath() {
        let mc_jar = PathBuf::from("minecraft-client.jar");

        let mut legacy = vec![mc_jar.clone()];
        let mut legacy_seen = HashSet::from([mc_jar.clone()]);

        let mut bootstrap = Vec::new();
        let mut bootstrap_seen = HashSet::new();

        push_unique_ordered(&mut legacy, &mut legacy_seen, mc_jar.clone());
        push_unique_ordered(&mut bootstrap, &mut bootstrap_seen, mc_jar.clone());

        assert_eq!(legacy, vec![mc_jar.clone()]);
        assert_eq!(bootstrap, vec![mc_jar]);
    }

    #[test]
    fn distinct_path_spellings_remain_distinct_and_ordered() {
        let mut items = Vec::new();
        let mut seen = HashSet::new();

        for path in ["libs/a.jar", "./libs/a.jar", "libs/b.jar", "libs/a.jar"] {
            push_unique_ordered(&mut items, &mut seen, PathBuf::from(path));
        }

        assert_eq!(
            items,
            vec![
                PathBuf::from("libs/a.jar"),
                PathBuf::from("./libs/a.jar"),
                PathBuf::from("libs/b.jar"),
            ]
        );
        assert_eq!(seen.len(), 3);
    }
}
