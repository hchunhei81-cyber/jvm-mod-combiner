# JVM Mod Fusion

> A Rust-written microkernel that aims to let Fabric and Forge/NeoForge mods
> **coexist inside one JVM**, without depending on Sinytra Connector.

[中文 README](README.zh.md)

## Why

Minecraft's modding ecosystem is split: **Fabric** and **Forge/NeoForge** ship
incompatible loaders, incompatible Mixin conventions, and incompatible event
systems. Existing cross-loader solutions (notably Sinytra Connector) work by
translating one ABI into the other at the bytecode level. That approach is
inherently fragile: every upstream loader release can break the translation
layer.

This project takes a different route. **No translation. Isolate each loader in
its own sandbox, and let them meet only at the data layer.**

## Architecture
Rust Microkernel
(JVM lifecycle, sandbox orchestration, data bus)
|
+--------------+--------------+
| |
+-----v-----+ +-----v-----+
| Sandbox A | | Sandbox B |
| Fabric | | Forge |
| Loader | | Loader |
| + mods | | + mods |
+-----------+ +-----------+


Three design pillars:

1. **Microkernel.** The fusion layer is the only component that owns a JVM. It
   depends on neither Fabric nor Forge.
2. **Runtime sandbox isolation.** Each loader runs inside its own isolated
   `URLClassLoader`. Mod classes never share a parent loader. Same-named classes
   under different loaders do not collide.
3. **Zero-copy data bus (planned).** Cross-sandbox communication goes through a
   memory bus owned by Rust. Mod code on either side never meets; the two
   sandboxes only intersect at the data layer.

## Project status

**Early prototype. Not yet playable.**

Verified on macOS + Temurin 21 (arm64):

- [x] Rust process launches an embedded JVM via JNI
- [x] Rust calls Java static methods and reads back return values
- [x] Rust loads external `.jar` files dynamically through `URLClassLoader`
- [x] Two isolated `URLClassLoader` instances load the same class name and
      produce different behavior
- [x] **Rust JNI boots Minecraft 1.21.4 + Fabric Loader — the main menu renders,
      the sound engine initializes, textures load, and the game runs at a normal
      frame rate**
- [x] Classpath is dynamically assembled from `mc_libraries/` (253 entries);
      Fabric recognizes the game via `fabric.gameJarPath` and classpath isolation
      being disabled

In progress:

- [ ] Load a real Forge 1.21.4 loader into Sandbox B
- [ ] Resolve the `parent = null` problem for real loaders (they need
      `java.lang.*` and friends)
- [ ] Handle Mixin conflicts between the two sandboxes
- [ ] Zero-copy data bus
- [ ] Dynamic dependency resolution (parse the version manifest instead of
      copying the whole `libraries/` tree)
- [ ] Cross-platform support (Windows / Linux / macOS)

![Proof: Rust JNI launches Minecraft 1.21.4 + Fabric](image.png)

## Quick start

### Requirements

- Rust (stable, 1.75+)
- A JDK. Tested with Temurin 21; JDK 8+ should also work.

### Build and run


git clone https://github.com/hchunhei81-cyber/jvm-mod-fusion.git
cd jvm-mod-fusion

# Point JAVA_HOME at your JDK
export JAVA_HOME=/Library/Java/JavaVirtualMachines/temurin-21.jdk/Contents/Home

cargo run

If everything is wired correctly, you should see:

=== Rust JVM Fusion Kernel: Sandbox Isolation Test ===
[Sandbox A] I am from Sandbox A!
[Sandbox B] I am from Sandbox B!
[OK] If you see A and B above, sandbox isolation is working!

# Tech stack

Rust — kernel, JVM orchestration
jni crate (0.22.4) — FFI bridge to the JVM
Java / JVM — where loaders and mods actually live
Why not Sinytra Connector

Sinytra Connector translates Fabric mods into something Forge can run at the
bytecode level. It does that surprisingly well, but:

It is fragile by construction: every loader update can break it.
It has to hook deeply into both loaders, so it always lags upstream.
It does not isolate the two loaders; they share one class space.
This project makes a different bet: treat the two loaders as two separate
worlds, and connect them only at the render/data layer.

It may fail. The Mixin conflict problem alone is hard. But if it works, it does
not break when a loader updates, by design.

# Known challenges

Forge 1.21+ relies on the Java Module System (--module-path,
BootstrapLauncher, securejarhandler), which must be assembled to match the
official installer exactly.
ModLauncher 10 requires well-formed JAR manifest metadata; anything that
rewrites manifests silently breaks version discovery.
Mixin conflicts across two loaders in one JVM.
ClassLoader hierarchy for real loaders (parent = null is not viable in
practice).

# Roadmap
Load a real Fabric Loader jar into Sandbox A.
Load a real Forge Loader jar into Sandbox B (requires a hybrid parent loader).
Resolve Mixin conflicts — either bytecode merging on the Rust side, or a
priority-based allowlist.
Zero-copy data bus for cross-sandbox communication.
Cross-platform (Windows / Linux / macOS).
Contributing

Currently a personal research project. Contribution guidelines will be added if
it reaches a usable prototype stage.

# License
BSD 3-Clause. See [LICENCE](LICENCE).


