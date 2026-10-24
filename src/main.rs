use jni::{InitArgsBuilder, JNIVersion, JavaVM};
use jni::objects::{JValue, JObject, JObjectArray, JClass};
use jni::jni_str;
use jni::jni_sig;
use std::env;

fn main() {
    println!("=== Rust JVM Fusion Kernel: Sandbox Isolation Test ===");

    let jvm_args = InitArgsBuilder::new()
        .version(JNIVersion::V1_8)
        .build()
        .expect("Failed to build JVM arguments");

    let jvm = JavaVM::new(jvm_args).expect("Failed to start JVM");

    jvm.attach_current_thread(|env| {
        let current_dir = env::current_dir().unwrap();

        // --- Sandbox A ---
        println!("[Rust] Initializing Sandbox A...");
        let jar_a = format!("file:{}/sandbox/sandbox_a.jar", current_dir.display());
        let loader_a = create_isolated_loader(env, &jar_a);

        // --- Sandbox B ---
        println!("[Rust] Initializing Sandbox B...");
        let jar_b = format!("file:{}/sandbox/sandbox_b.jar", current_dir.display());
        let loader_b = create_isolated_loader(env, &jar_b);

        // --- Verify isolation ---
        println!("[Rust] Calling SandboxTest.hello() from Sandbox A...");
        invoke_hello(env, &loader_a);

        println!("[Rust] Calling SandboxTest.hello() from Sandbox B...");
        invoke_hello(env, &loader_b);

        println!("[OK] If you see A and B above, sandbox isolation is working!");
        Ok::<(), jni::errors::Error>(())
    }).expect("Failed to attach thread");

    println!("=== Test finished ===");
}

fn create_isolated_loader<'local>(env: &mut jni::Env<'local>, jar_path: &str) -> JObject<'local> {
    // 1. Create java.net.URL
    let url_class = env.find_class(jni_str!("java/net/URL")).unwrap();
    let url_str = env.new_string(jar_path).unwrap();
    let url_obj = env.new_object(
        url_class,
        jni_sig!((java.lang.String) -> void),
        &[JValue::Object(&url_str)],
    ).unwrap();

    // 2. Create java.net.URL[]
    let url_array: JObjectArray<JObject> = JObjectArray::<JObject>::new(env, 1, JObject::null()).unwrap();
    url_array.set_element(env, 0, &url_obj).unwrap();

    // 3. Create URLClassLoader(URL[], null) -> parent is null for strict isolation
    let url_class_loader_class = env.find_class(jni_str!("java/net/URLClassLoader")).unwrap();
    env.new_object(
        url_class_loader_class,
        jni_sig!((java.net.URL[], java.lang.ClassLoader) -> void),
        &[JValue::Object(&url_array), JValue::Object(&JObject::null())],
    ).unwrap()
}

fn invoke_hello<'local>(env: &mut jni::Env<'local>, loader: &JObject<'local>) {
    let class_name = env.new_string("SandboxTest").unwrap();
    let class_obj = env.call_method(
        loader,
        jni_str!("loadClass"),
        jni_sig!((java.lang.String) -> java.lang.Class),
        &[JValue::Object(&class_name)],
    ).unwrap().l().unwrap();

    let loaded_class: JClass = unsafe { JClass::from_raw(env, class_obj.into_raw()) };

    env.call_static_method(
        &loaded_class,
        jni_str!("hello"),
        jni_sig!(() -> void),
        &[],
    ).unwrap();
}
