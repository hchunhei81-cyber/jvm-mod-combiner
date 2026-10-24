use jni::{InitArgsBuilder, JNIVersion, JavaVM};
use jni::objects::{JValue, JObject, JObjectArray, JClass};
use jni::jni_str;
use jni::jni_sig;
use serde_json::Value;
use std::env;
use std::path::{Path, PathBuf};

fn rules_allow(lib: &Value) -> bool {
    let rules = match lib.get("rules").and_then(|r| r.as_array()) {
        Some(r) => r,
        None => return true, // No rules = allow everywhere
    };
    let mut allow = false;
    for rule in rules {
        let action = rule.get("action").and_then(|a| a.as_str()).unwrap_or("allow");
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

        // 1. Main artifact
        if let Some(art) = lib.pointer("/downloads/artifact") {
            if let Some(path) = art.get("path").and_then(|v| v.as_str()) {
                if let Some(found) = find_local(path, search_dirs) {
                    classpath.push(found);
                } else {
                    println!("[Rust] MISSING: {}", path);
                }
            }
        }

        // 2. Natives classifiers (macOS ARM64)
        if let Some(classifiers) = lib.pointer("/downloads/classifiers") {
            for key in ["natives-macos-arm64", "natives-macos"] {
                if let Some(c) = classifiers.get(key) {
                    if let Some(path) = c.get("path").and_then(|v| v.as_str()) {
                        if let Some(found) = find_local(path, search_dirs) {
                            classpath.push(found);
                        } else {
                            println!("[Rust] MISSING native: {}", path);
                        }
                    }
                }
            }
        }
    }

    println!("[Rust] Resolved {}/{} libraries from version JSON", classpath.len(), total);
    classpath
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

fn main() {
    println!("=== Rust JVM Fusion Kernel: Dynamic Classpath Test ===");

    let jvm_args = InitArgsBuilder::new()
        .version(JNIVersion::V1_8)
        .build()
        .expect("Failed to build JVM arguments");

    let jvm = JavaVM::new(jvm_args).expect("Failed to start JVM");

    jvm.attach_current_thread(|env| {
        let current_dir = env::current_dir().unwrap();
        let sandbox_dir = current_dir.join("sandbox");
        let mc_root = PathBuf::from(std::env::var("HOME").unwrap())
            .join("Library/Application Support/minecraft");

        // Search order: flat sandbox/mc_libraries first, then real MC libraries
        let search_dirs = vec![
            sandbox_dir.clone(),
            sandbox_dir.join("mc_libraries"),
            mc_root.join("libraries"),
        ];

        // 1. Build URLs: fabric-loader + asm + mixin (hardcoded)
        let mut urls: Vec<String> = vec![
            "fabric-loader.jar",
            "asm-9.6.jar",
            "asm-tree-9.6.jar",
            "asm-analysis-9.6.jar",
            "asm-commons-9.6.jar",
            "asm-util-9.6.jar",
            "sponge-mixin.jar",
        ]
        .iter()
        .map(|name| format!("file:{}/{}", sandbox_dir.display(), name))
        .collect();

        // 2. Dynamic: parse version JSON for exact libs
        let version_json = sandbox_dir.join("1.21.4.json");
        let lib_paths = resolve_classpath(&version_json, &search_dirs);
        for p in &lib_paths {
            urls.push(format!("file:{}", p.display()));
        }

        // 3. Game jar
        urls.push(format!("file:{}/minecraft-client.jar", sandbox_dir.display()));

        println!("[Rust] Total classpath entries: {}", urls.len());

        // 4. Build URL[]
        let url_class = env.find_class(jni_str!("java/net/URL")).unwrap();
        let url_array: JObjectArray<JObject> =
            JObjectArray::<JObject>::new(env, urls.len(), JObject::null()).unwrap();
        for (i, path) in urls.iter().enumerate() {
            let url_str = env.new_string(path).unwrap();
            let url_obj = env.new_object(
                &url_class,
                jni_sig!((java.lang.String) -> void),
                &[JValue::Object(&url_str)],
            ).unwrap();
            url_array.set_element(env, i, &url_obj).unwrap();
        }

        // 5. URLClassLoader
        let class_loader_class = env.find_class(jni_str!("java/lang/ClassLoader")).unwrap();
        let parent_loader = env.call_static_method(
            class_loader_class,
            jni_str!("getSystemClassLoader"),
            jni_sig!(() -> java.lang.ClassLoader),
            &[],
        ).unwrap().l().unwrap();

        let url_class_loader_class = env.find_class(jni_str!("java/net/URLClassLoader")).unwrap();
        let loader = env.new_object(
            url_class_loader_class,
            jni_sig!((java.net.URL[], java.lang.ClassLoader) -> void),
            &[JValue::Object(&url_array), JValue::Object(&parent_loader)],
        ).unwrap();

        // 6. Load KnotClient
        let target = env.new_string("net.fabricmc.loader.impl.launch.knot.KnotClient").unwrap();
        let result = env.call_method(
            &loader,
            jni_str!("loadClass"),
            jni_sig!((java.lang.String) -> java.lang.Class),
            &[JValue::Object(&target)],
        ).unwrap();
        let class_obj = result.l().unwrap();
        let knot_class: JClass = unsafe { JClass::from_raw(env, class_obj.into_raw()) };
        println!("[OK] KnotClient loaded");

        // 7. Tell Fabric where the game jar is and unblock classpath
        let mc_jar_path = format!("{}/minecraft-client.jar", sandbox_dir.display());
        set_property(env, "fabric.gameJarPath", &mc_jar_path);
        set_property(env, "fabric.debug.disableClassPathIsolation", "true");
        set_property(env, "fabric.gameVersion", "1.21.4");

        // 8. Launch
        println!("[Rust] Calling KnotClient.main(...) ...");
        let args_array: JObjectArray<JObject> = JObjectArray::<JObject>::new(env, 0, JObject::null()).unwrap();
        let call_result = env.call_static_method(
            &knot_class,
            jni_str!("main"),
            jni_sig!((java.lang.String[]) -> void),
            &[JValue::Object(&args_array)],
        );
        match call_result {
            Ok(_) => println!("[OK] KnotClient.main() returned."),
            Err(e) => {
                println!("[FAIL] KnotClient.main() threw: {:?}", e);
                if let Some(exc) = env.exception_occurred() {
                    env.exception_clear();
                    let _ = env.call_method(&exc, jni_str!("printStackTrace"), jni_sig!(() -> void), &[]);
                }
            }
        }

        Ok::<(), jni::errors::Error>(())
    }).expect("Failed to attach thread");

    println!("=== Test finished ===");
}
