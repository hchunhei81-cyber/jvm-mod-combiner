# JVM Mod Combiner — Development Roadmap (TODO.md)

> This roadmap tracks the project's verified achievements, current engineering tasks, and long-term development goals.
>
> The project is currently an experimental prototype. Its immediate objective is to establish reliable cross-loader communication before attempting broader mod compatibility.

---

## 🟢 Completed

### 1. Minecraft Loader Launching

- [x] **Fabric 1.21.4**
  - Successfully launched Fabric Loader through the Rust-based launcher.
  - Main menu rendered successfully.
  - Sound engine and textures initialized.
  - Verified on macOS ARM64 with Temurin JDK 21.

- [x] **NeoForge 1.21.1**
  - Successfully launched NeoForge through a Rust-managed subprocess.
  - Bypassed official launcher restrictions and resolved the major BootstrapLauncher initialization issues.
  - Main menu rendered successfully.
  - Verified that installed mods can be detected.

- [x] **Forge 1.21.4 Experimental Boot**
  - Successfully reached the Forge early loading screen during experiments.
  - ModLauncher initialization and dependency-path issues were investigated and bypassed sufficiently to demonstrate experimental boot capability.
  - Full gameplay stability and complete launcher integration remain separate verification targets.

### 2. Rust JNI and ClassLoader Isolation

- [x] Initialize an embedded JVM through Rust and JNI.
- [x] Invoke Java static methods and retrieve return values from Rust.
- [x] Dynamically load external JAR files through `URLClassLoader`.
- [x] Verify that isolated ClassLoaders can load classes with identical names without sharing class definitions.
- [x] Establish the Rust launcher as the process-level orchestration layer.

### 3. Java Agent and Runtime Instrumentation

- [x] Inject a Java Agent through JVM startup arguments.
- [x] Execute the Agent's `premain()` entry point.
- [x] Use `Instrumentation` and `ClassFileTransformer` to observe classes loaded by Fabric's `KnotClassLoader`.
- [x] Verify runtime interception of Minecraft-related classes.

### 4. Rust DataBus — IPC Prototype

- [x] Implement a TCP-based DataBus in Rust.
- [x] Bind the listener synchronously before starting the accept thread.
- [x] Register clients using sandbox identities (`fabric` and `neoforge`).
- [x] Route JSON messages according to their destination.
- [x] Verify Fabric ↔ NeoForge message routing with `hello` / `ack` messages.
- [x] Support independently launched sandboxes without starting duplicate DataBus listeners.
- [x] Verify that both Java Agents can connect to the Rust DataBus.

### 5. Cross-Sandbox Player State Synchronization

- [x] Discover the Fabric Minecraft instance through runtime reflection.
- [x] Read the Fabric player's position and rotation.
- [x] Resolve the relevant obfuscated Minecraft methods for the tested runtime.
- [x] Send player movement data through the Rust TCP DataBus.
- [x] Receive and parse movement messages in the NeoForge sandbox.
- [x] Discover the NeoForge Minecraft singleton independently of incoming movement messages.
- [x] Detect NeoForge `ClientLevel` and `LocalPlayer` readiness.
- [x] Inject a mirror `RemotePlayer` into NeoForge's client world.
- [x] Update the mirror entity using received Fabric movement data.

**Current status:** The initial one-way Fabric-to-NeoForge player-state pipeline and RemotePlayer injection have been verified as proof-of-concept functionality. Heartbeat timeout cleanup and mirror recreation have also received initial runtime verification.

Fabric is the sole authoritative player-state publisher. NeoForge operates as a receiver-only client. This prototype does not establish complete gameplay equivalence or general cross-loader compatibility.

---

## 🟡 High Priority

### 6. Enforce One-Way Player-State Authority

**Goal:** Enforce Fabric as the sole authoritative player-state publisher and keep NeoForge receiver-only.

- [x] Establish Fabric as the sole authoritative player-state source.
- [x] Remove the active `sampleNeoForgePlayer(player)` call from the edited NeoForge monitor source.
- [ ] Rebuild and package the updated Java Agent.
- [ ] Enforce sender authorization in the Rust DataBus using registered TCP connection identity.
- [ ] Reject `player_move` messages originating from the registered NeoForge connection.
- [ ] Preserve explicitly authorized reverse-direction message types, such as interaction requests and diagnostics.
- [ ] Verify Fabric-to-NeoForge player-state delivery using the rebuilt Agent.
- [ ] Verify that rejected NeoForge `player_move` messages do not disconnect the client.
- [ ] Test reconnection and fresh player-state transmission.
- [ ] Add protocol versioning, timestamps, sequence numbers, and session generations.
- [ ] Reject stale or out-of-session player-state updates.
- [ ] Verify that unauthorized messages cannot bypass the sender authorization policy.

### 7. Stabilize Remote Player Mirroring

- [ ] Replace temporary entity-ID allocation with a collision-safe strategy.
- [ ] Maintain a stable mapping between remote player UUIDs and mirror entities.
- [ ] Remove mirror entities when their source player disconnects.
- [ ] Handle world changes and dimension transitions.
- [ ] Prevent duplicate mirror entities after reconnection.
- [ ] Add interpolation and smoothing to reduce visible movement jitter.
- [ ] Handle missing chunks and unloaded entity regions.
- [ ] Verify entity lifecycle behavior during world reloads.

### 8. Refactor the Rust Launcher

- [ ] Rewrite remaining Python-based launcher operations in Rust.
  - Version metadata parsing.
  - Dependency discovery.
  - JAR validation.
  - Version and architecture filtering.
  - Dependency download and cache management.

- [ ] Refactor `launch_neoforge_subprocess()`.
  - Remove dependencies on external launch scripts where practical.
  - Construct `--module-path` dynamically.
  - Construct `-DlegacyClassPath` dynamically.
  - Generate JVM arguments from version metadata.
  - Keep platform-specific arguments separate from shared launch logic.

- [ ] Improve process management.
  - Track child JVM processes.
  - Detect unexpected process termination.
  - Shut down both sandboxes cleanly.
  - Preserve useful logs when one sandbox crashes.

- [ ] Organize build artifacts and repository structure.
  - Remove unnecessary generated files.
  - Improve `.gitignore`.
  - Separate source code, runtime assets, and test artifacts.
  - Keep documentation consistent with the actual implementation.

---

## 🔵 Medium Priority

### 9. DataBus Protocol Evolution

The TCP DataBus is already implemented. The next step is to evolve it from a basic JSON router into a reliable synchronization transport.

- [ ] Define a formal message protocol.
- [ ] Add protocol version fields.
- [ ] Add message IDs and sequence numbers.
- [ ] Add timestamps and connection/session identifiers.
- [ ] Validate message schemas before forwarding.
- [ ] Handle malformed messages safely.
- [ ] Add rate limiting and message-size limits.
- [ ] Add heartbeat timeout detection.
- [ ] Add structured diagnostics and traffic statistics.

### 10. Synchronize More Than Player Coordinates

**Goal:** Move from player mirroring toward shared gameplay state.

- [ ] Entity discovery and entity identity mapping.
- [ ] Entity position, rotation, velocity, and basic state synchronization.
- [ ] Entity spawn and removal events.
- [ ] Basic health and damage-state synchronization.
- [ ] Selected inventory and equipment state.
- [ ] Block-state synchronization.
- [ ] Container and GUI interaction requests.
- [ ] Dimension and world metadata synchronization.

**Important:** These features must be introduced incrementally. Synchronizing an entity's visual state does not automatically synchronize its AI, physics, inventory, or mod-specific behavior.

### 11. Mixin and Runtime Compatibility

- [ ] Investigate conflicts caused by different Mixin environments.
- [ ] Establish clear ownership of classes modified by multiple loaders.
- [ ] Develop a transformation compatibility registry.
- [ ] Detect incompatible transformations before applying them.
- [ ] Investigate priority-based transformation policies.
- [ ] Evaluate ASM-based bytecode transformation and merging as a long-term research direction.

**Design constraint:** Do not assume that two independently transformed versions of the same Minecraft class can be safely merged. Transformation order, injection points, target method signatures, and loader lifecycle must be considered.

### 12. Dependency Resolution

- [ ] Implement a complete Minecraft version manifest parser.
- [ ] Resolve dependencies from the version JSON instead of copying the entire `libraries/` directory.
- [ ] Respect OS and architecture rules.
- [ ] Handle native libraries separately from ordinary JAR dependencies.
- [ ] Verify downloaded files using official hashes where available.
- [ ] Cache dependencies by version and checksum.
- [ ] Detect conflicting dependency versions between loaders.

---

## 🟣 Low Priority / Long-Term Research

### 13. Cross-Platform Support

- [ ] **macOS**
  - Continue validating Apple Silicon.
  - Improve native library discovery.
  - Handle macOS first-thread requirements.

- [ ] **Windows**
  - Support `java.exe` discovery.
  - Support `%APPDATA%` Minecraft directories.
  - Handle Windows native libraries and process management.

- [ ] **Linux**
  - Support standard Minecraft directory layouts.
  - Handle Linux LWJGL natives.
  - Investigate X11 and Wayland behavior.

- [ ] Standardize path handling and JVM argument generation across platforms.

### 14. Rendering and Window Integration

- [ ] Investigate whether two Minecraft windows can be managed as one user-facing experience.
- [ ] Research native window embedding and overlay approaches.
- [ ] Evaluate platform-specific window composition.
- [ ] Investigate shared rendering resources only after the process and gameplay synchronization layers are stable.

**Priority:** Reliable gameplay synchronization comes before cross-process GPU sharing.

### 15. Advanced Mod Compatibility

- [ ] Investigate Fabric API and NeoForge API interoperability.
- [ ] Design a cross-loader capability abstraction.
- [ ] Research inventory, energy, fluid, and block-entity state adapters.
- [ ] Investigate proxy representations for unsupported modded blocks and machines.
- [ ] Explore GUI request forwarding for remote sandbox objects.
- [ ] Evaluate native-library interoperability limitations.

### 16. Legacy Version Support

- [ ] Re-evaluate Forge 1.16.5 support.
- [ ] Test the dependencies extracted from the Windows virtual machine.
- [ ] Determine whether the old Forge launch pipeline can be reproduced reliably on macOS.
- [ ] If the maintenance cost is too high, document 1.16.5 as unsupported rather than compromising the stability of newer versions.

---
## Development Order

The intended implementation order is:

1. Rebuild and package the receiver-only Java Agent.
2. Implement Rust DataBus sender authorization using registered connection identity.
3. Verify Fabric-to-NeoForge player-state delivery.
4. Verify NeoForge `player_move` rejection while preserving explicitly authorized reverse-direction messages.
5. Verify rejected messages do not disconnect the NeoForge client.
6. Test heartbeat timeout, mirror removal, and mirror recreation.
7. Stabilize entity identity, lifecycle management, and failure recovery.
8. Add protocol versioning, timestamps, sequence numbers, and session generations.
9. Expand synchronization to additional entities and basic gameplay state.
10. Refactor dependency resolution and NeoForge launch orchestration.
11. Investigate Mixin compatibility and cross-loader API adapters.
12. Expand platform support and research advanced mod compatibility.

---

## Engineering Principles

The project will follow these principles during development:

- **Isolation first:** Loader internals should remain isolated whenever possible.
- **Data before behavior:** Synchronize explicit state before attempting to synchronize arbitrary mod logic.
- **Asynchronous communication:** One sandbox must not block its rendering or game loop while waiting for the other.
- **Game-thread safety:** Minecraft world and entity operations must execute on the appropriate client game thread.
- **Incremental verification:** Each new synchronization feature must be tested independently before expanding the protocol.
- **No premature optimization:** TCP is sufficient for the current prototype. Shared memory and zero-copy transport should only be considered after profiling identifies a real bottleneck.
- **Honest compatibility claims:** A successful loader launch or entity mirror does not prove that arbitrary mods can coexist correctly.

---

## Current Development Focus

<badge color="success">Verified foundation</badge>

Rust launcher, Fabric and NeoForge startup, Java Agent injection, TCP DataBus, and the initial Fabric-to-NeoForge player-mirroring pipeline.

<badge color="warning">Next milestone</badge>

Rebuild the receiver-only Java Agent, enforce one-way sender authorization in the Rust DataBus, and verify reliable Fabric-to-NeoForge player-state synchronization.

The next milestone also includes testing unauthorized reverse `player_move` rejection, reconnect behavior, and RemotePlayer lifecycle recovery.

> The immediate objective is not to make every Fabric mod work with every NeoForge mod. It is to build a reliable, measurable foundation on which broader interoperability can be developed.