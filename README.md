# JVM Mod Fusion

> A Rust-based micro-kernel that aims to run Fabric and Forge/NeoForge mods **side by side in the same JVM**, without Sinytra Connector.

[中文文档](README.zh.md)

## Why

The Minecraft modding ecosystem is split: **Fabric** and **Forge/NeoForge** have incompatible loaders, mixin conventions, and event systems. Existing solutions (like Sinytra Connector) work by translating one loader's ABI into the other at the bytecode level, which is fragile and breaks whenever either loader changes.

This project takes a different approach: **don't translate anything. Isolate both loaders in their own sandbox, and let them talk only at the data layer.**

## Architecture
┌──────────────────────────────┐
│ Rust Micro-Kernel (this) │
│ - JVM lifecycle │
│ - Sandbox orchestration │
│ - Zero-copy data bus (WIP) │
└──────────────┬───────────────┘
│
┌─────────────────────┴─────────────────────┐
│ │
┌──────────▼──────────┐ ┌──────────▼──────────┐
│ Sandbox A │ │ Sandbox B │
│ Fabric Loader │ │ Forge Loader │
│ + Fabric mods │ │ + Forge mods │
└─────────────────────┘ └─────────────────────┘


Three design pillars:

1. **Micro-Kernel** — the fusion layer is the only thing that owns the JVM. It does not depend on Fabric or Forge.
2. **Runtime Sandbox Isolation** — each loader runs in its own `URLClassLoader` with no shared parent for mod classes. Same class name, different loader, no conflict.
3. **Zero-Copy Data Bus (planned)** — cross-sandbox communication happens through a Rust-owned memory bus. Mods never see each other's code; only the resulting game data is shared.

## Status

**Early prototype. Not usable for playing Minecraft yet.**

What is actually working (verified on macOS + Temurin 25):

- [x] Rust process can bootstrap an embedded JVM via JNI
- [x] Rust can call static Java methods and read back return values
- [x] Rust can dynamically load external `.jar` files through `URLClassLoader`
- [x] Two isolated `URLClassLoader` instances can load classes with the same name and produce different behaviors
- [x] **Rust JNI successfully boots Minecraft 1.21.4 with Fabric Loader — the main menu renders, sound engine initializes, textures load, and the game runs at full frame rate**
- [x] Dynamic classpath assembly from `mc_libraries/` (253 entries) with `fabric.gameJarPath` and classpath isolation disabled


What is **not** working yet:

- [ ] Loading a real Forge/NeoForge Loader jar into Sandbox B
- [ ] Resolving the `parent = null` problem for real loaders (they need `java.lang.*` etc.)
- [ ] Handling Mixin conflicts between the two sandboxes
- [ ] Zero-copy data bus
- [ ] Dynamic dependency resolution (version manifests, not full `libraries` copy)
- [ ] Cross-platform support (Windows / Linux / macOS)

![Proof: Rust JNI booting Minecraft 1.21.4 with Fabric](image.png)

## Quick Start

### Prerequisites

- Rust (stable, 1.75+)
- A JDK (tested with Temurin 25, JDK 8+ should work)

### Build and Run

git clone https://github.com/hchunhei81-cyber/jvm-mod-fusion.git
cd jvm-mod-fusion

# Point JAVA_HOME to your JDK
export JAVA_HOME=/Library/Java/JavaVirtualMachines/temurin-25.jdk/Contents/Home

cargo run
If everything works, you will see:

=== Rust JVM Fusion Kernel: Sandbox Isolation Test ===
[Sandbox A] I am from Sandbox A!
[Sandbox B] I am from Sandbox B!
[OK] If you see A and B above, sandbox isolation is working!

## Tech Stack

Rust — kernel, JVM orchestration
jni crate (0.22.4) — FFI bridge to the JVM
Java / JVM — where the loaders and mods actually live

## Why Not Sinytra Connector?
Sinytra Connector translates Fabric mods to Forge at the bytecode level. It works surprisingly well, but:

It is inherently fragile — every loader update may break it.
It requires deep hooks into both loaders, so it lags behind upstream.
It does not isolate the loaders; they share the same class space.


This project tries a fundamentally different bet: treat the two loaders as two separate worlds, and connect them only at the rendering/data layer.

It may fail. The Mixin conflict problem alone is brutal. But if it works, it survives loader updates by design.

## Roadmap

Load a real Fabric Loader jar into Sandbox A.
Load a real Forge Loader jar into Sandbox B (requires a hybrid parent classloader).
Handle Mixin conflicts — likely through a Rust-side bytecode merger or a priority-based allowlist.
Zero-copy data bus for cross-sandbox communication.
Cross-platform (Windows / Linux / macOS).
Contributing

This is a personal research project for now. If it moves past the prototype stage, contribution guidelines will be added.

# License
BSD 3-Clause. See [LICENCE](LICENCE).

