use jni::{InitArgsBuilder, JNIVersion, JavaVM};
use jni::objects::{JValue, JObject, JObjectArray, JClass};
use jni::jni_str;
use jni::jni_sig;
use std::env;
use std::fs;

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
    println!("=== Rust JVM Fusion Kernel: Full Classpath Probe ===");

    let jvm_args = InitArgsBuilder::new()
        .version(JNIVersion::V1_8)
        .build()
        .expect("Failed to build JVM arguments");

    let jvm = JavaVM::new(jvm_args).expect("Failed to start JVM");

    jvm.attach_current_thread(|env| {
        let current_dir = env::current_dir().unwrap();
        let sandbox_dir = current_dir.join("sandbox");

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

        let mc_libs_dir = sandbox_dir.join("mc_libraries");
        if mc_libs_dir.is_dir() {
            for entry in fs::read_dir(&mc_libs_dir).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().map_or(false, |ext| ext == "jar") {
                    urls.push(format!("file:{}", path.display()));
                }
            }
        }
        println!("[Rust] Total classpath entries: {}", urls.len());

        urls.push(format!("file:{}/minecraft-client.jar", sandbox_dir.display()));

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

        // 告诉 Fabric Loader 游戏 jar 在哪里
        let mc_jar_path = format!("{}/minecraft-client.jar", sandbox_dir.display());
        set_property(env, "fabric.gameJarPath", &mc_jar_path);
        println!("[Rust] Set fabric.gameJarPath = {}", mc_jar_path);

        // 关闭类路径隔离
        set_property(env, "fabric.debug.disableClassPathIsolation", "true");
        println!("[Rust] Set fabric.debug.disableClassPathIsolation = true");

        // 让 Fabric 加载 mappings
        set_property(env, "fabric.gameVersion", "1.21.4");
        set_property(env, "fabric.mappings", "net.fabricmc:yarn:1.21.4+build.8:v2");
        println!("[Rust] Set fabric.mappings for 1.21.4");

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
