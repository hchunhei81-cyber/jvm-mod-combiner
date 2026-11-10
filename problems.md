# JVM Mod Combiner — Problems & Solutions

> This document records the major technical problems encountered during the development of JVM Mod Combiner, including their symptoms, root causes, attempted solutions, workarounds, and current status.
>
> The project involves launching and investigating different Minecraft mod loaders through a Rust-based JVM orchestration layer.
>
> Some problems have been resolved, some have only been worked around, and others remain under investigation. Each section distinguishes verified results from unconfirmed explanations.

**Project status:** Experimental prototype. Fabric, Forge, and NeoForge startup experiments have achieved significant milestones, but complete cross-loader interoperability is not yet implemented.

---

# 1. Fabric 1.21.4 Startup Problems

## 1.1 Missing ASM and Sponge Mixin Dependencies

<badge color="success">Resolved</badge>

**Symptoms**

Fabric initialization failed with `ClassNotFoundException` while attempting to load `KnotClient` and its supporting classes.

**Root cause**

The manually constructed Rust classpath did not contain all the low-level bytecode manipulation libraries required by Fabric.

Important dependencies included:

- ASM
- ASM Tree
- ASM Analysis
- ASM Commons
- ASM Util
- Sponge Mixin

**Solution**

Extended the classpath construction logic to include the required ASM components and Sponge Mixin.

The dependencies were located in the local Minecraft libraries directory and copied or referenced from the sandbox environment.

ASM 9.9.1 was used in one of the dependency configurations during this investigation. The exact version must remain compatible with the selected loader.

**Verification**

Fabric successfully passed the earlier missing-class stage and proceeded to the KnotClient initialization pipeline.

---

## 1.2 `No game providers present on the class path`

<badge color="success">Resolved</badge>

**Symptoms**

Fabric failed to initialize because it could not locate a valid Minecraft game provider.

Typical error:

```text
No game providers present on the class path
```

**Root cause**

The manually launched environment did not correctly identify the Minecraft game JAR.

Additionally, Fabric's classpath isolation mechanism prevented the game classes from being discovered through the expected classpath configuration.

**Solution**

Explicitly supplied the game JAR location and adjusted Fabric's classpath isolation configuration.

Relevant JVM properties:

```text
-Dfabric.gameJarPath=/path/to/minecraft-client.jar
-Dfabric.debug.disableClassPathIsolation=true
-Dfabric.gameVersion=1.21.4
```

The actual game JAR path must be resolved dynamically from the local Minecraft installation.

**Verification**

Fabric recognized the Minecraft game provider and proceeded to launch the client.

---

## 1.3 Classpath Conflicts Caused by Copying the Entire Libraries Directory

<badge color="success">Improved</badge>

**Symptoms**

Errors included:

```text
NoSuchMethodError
ClassNotFoundException
```

Some failures involved libraries such as Authlib, DataFixerUpper, and LWJGL.

**Root cause**

Initially, copying the entire Minecraft `libraries` directory into the classpath introduced multiple versions of the same dependency.

A JAR being present does not necessarily mean it is the correct version for the current Minecraft installation.

**Solution**

Implemented a Rust-side `resolve_classpath()` routine that reads the Minecraft version manifest.

The resolver:

1. Parses the Minecraft version JSON.
2. Reads individual library declarations.
3. Evaluates operating-system rules.
4. Filters libraries according to the current platform and architecture.
5. Resolves the required JAR paths.
6. Includes the appropriate native classifiers.

For the Apple Silicon test environment, relevant rules include:

```text
macOS
arm64
natives-macos-arm64
natives-macos
```

**Verification**

The launcher successfully resolved the required dependencies from Minecraft 1.21.4 metadata.

One recorded dependency-resolution run resolved 32 of 77 declared libraries before subsequent classpath construction.

Further work is still needed to make dependency resolution fully portable and comprehensive.

---

## 1.4 LWJGL Native Library Loading Failure

<badge color="success">Resolved in tested macOS configuration</badge>

**Symptoms**

Typical errors:

```text
Failed to locate library: liblwjgl.dylib
```

or:

```text
MemoryUtil initialization failed
```

**Root cause**

The LWJGL Java libraries were available, but their platform-specific native binaries were not correctly extracted or located.

**Solution**

Downloaded and extracted the appropriate LWJGL macOS ARM64 native libraries into a dedicated directory.

Configured:

```text
-Djava.library.path=/path/to/natives
```

**Verification**

The Minecraft client successfully initialized its graphics and sound subsystems in the tested environment.

---

## 1.5 Missing Netty Dependency at Runtime

<badge color="warning">Requires further verification</badge>

**Symptoms**

A Fabric launch reached the KnotClient initialization stage but subsequently encountered:

```text
NoClassDefFoundError:
io/netty/util/ResourceLeakDetector$Level
```

**Root cause**

The immediate failure indicated that the runtime could not resolve the required Netty class.

Although Guava had been downloaded and verified, Guava was unrelated to the missing Netty class.

This demonstrated that successfully resolving one dependency does not guarantee that the complete runtime dependency graph is available.

**Investigation**

The following checks were performed:

- Verified the downloaded Guava JAR.
- Confirmed that the expected Guava class existed inside the archive.
- Inspected the manually assembled classpath.
- Identified the missing Netty class as a separate dependency-resolution issue.

**Current status**

The Netty classpath issue requires verification against the exact Minecraft version metadata and actual runtime classpath.

The fix must ensure that the correct Netty JAR, rather than an arbitrary version, is selected.

---

# 2. NeoForge 1.21.1 Startup Problems — ModLauncher 11

This section records the earlier NeoForge 1.21.1 investigation. The current Rust launcher source is configured for NeoForge 21.4.161 / Minecraft 1.21.4; that newer configuration is a separate verification target.

## 2.1 `ModLauncher null starting` and NullPointerException

<badge color="success">Resolved / bypassed</badge>

**Symptoms**

BootstrapLauncher failed during ModLauncher initialization.

A representative failure involved:

```text
TypesafeMap.putIfAbsent
NullPointerException
```

The startup logs also showed:

```text
ModLauncher null starting
```

**Root cause**

The initial investigation focused on ModLauncher version metadata and manifest attributes.

When ModLauncher is loaded as a JPMS module, its runtime metadata lookup does not necessarily behave like ordinary classpath-based JAR loading.

Consequently, `getImplementationVersion()` can return `null` if the expected metadata is unavailable through the actual loading path.

The exact failure also depends on the selected ModLauncher and BootstrapLauncher versions.

**Solution / workaround**

Rather than relying on rewriting the original JAR manifest, the launcher was reworked to construct the expected module and legacy classpaths.

Important components included:

- `--module-path`
- `-DlegacyClassPath`
- BootstrapLauncher dependencies
- SecureJarHandler dependencies
- Correct JVM argument ordering

This approach was more reliable than attempting to solve the problem through manifest modification alone.

**Verification**

NeoForge 1.21.1 successfully progressed through the earlier ModLauncher initialization failure and reached the Minecraft client startup process.

---

## 2.2 `ServiceConfigurationError` and Early Window Initialization

<badge color="success">Workaround verified</badge>

**Symptoms**

The early loading window crashed with an error resembling:

```text
ServiceConfigurationError:
Unable to get public no-arg constructor
```

The failure involved:

```text
EarlyProgressVisualization
```

**Root cause**

The early window service initialization path was incompatible with the tested manually constructed launch environment.

The exact interaction between service discovery and the early window implementation was not fully isolated.

**Workaround**

Modified:

```text
config/fml.toml
```

Set:

```toml
earlyWindowControl = false
```

**Verification**

Disabling early window control allowed the startup process to bypass the specific failure.

This is a workaround, not a general solution for every early-window initialization problem.

---

## 2.3 `NoSuchElementException` and Missing Client Classes

<badge color="success">Resolved in tested configuration</badge>

**Symptoms**

The NeoForge client process failed with missing-class errors or exceptions during startup.

**Root cause**

The supplied `unix_args.txt` was not sufficient for the manually constructed client environment.

Some client-side dependencies were missing from the effective legacy classpath.

Examples investigated included:

- ICU4J
- Text-to-speech libraries
- LWJGL components

Java's module encapsulation also restricted some reflective access paths.

**Solution**

Used Python scripts to inspect the installed libraries and append the missing dependencies to the effective legacy classpath.

Additional JVM module-access options were introduced where required.

Example:

```text
--add-opens
--add-exports
```

For macOS, the following option was also required:

```text
-XstartOnFirstThread
```

This addresses the platform-specific requirement for creating the GLFW window on the first application thread.

**Verification**

NeoForge 1.21.1 successfully reached the Minecraft client environment in the tested configuration.

---

# 3. Forge 1.16.5 Startup Problems — ModLauncher 8

> This version was investigated but was not successfully brought to a working client state. The following records describe the attempted solutions and unresolved causes.

## 3.1 Official Installer JAR Splitter Hash Mismatch

<badge color="danger">Unresolved for native macOS installation</badge>

**Symptoms**

The official installer failed during its JAR splitting stage.

Representative output:

```text
Expected: xxx
Actual: xxx
```

The hash verification failure prevented the installer from completing.

**Investigation**

Several possibilities were considered, including differences in compression behavior between the macOS environment and the installer dependencies.

The suspected interaction with the system zlib implementation was not conclusively established.

**Attempts**

- Switched to Java 8u312.
- Tested Forge 36.2.42.
- Repeated the installation process.

These attempts did not resolve the failure.

**Workaround**

Ran the official installer in a Windows virtual machine.

The generated files, including:

```text
srg.jar
extra.jar
```

were copied back to macOS.

This allowed the investigation to proceed without relying on the failing native macOS installation step.

---

## 3.2 Missing `fmlloader` and `FMLClientLaunchHandler`

<badge color="danger">Unresolved</badge>

**Symptoms**

The launch process failed with:

```text
Cannot find launch target fmlclient
```

**Root cause**

Forge 1.16.5 uses an older launch architecture.

The required FML client launch classes were not available through the initially assembled dependency paths.

Attempts to locate a standalone artifact resembling:

```text
fmlloader-1.16.5-36.2.42.jar
```

were unsuccessful.

The inspected `universal.jar` and `client.jar` did not provide the required launch class.

**Attempted workaround**

Tried substituting a newer FML implementation found in a local Prism Launcher installation.

**Final failure**

The newer ModLauncher environment could not correctly recognize the older FML service implementation.

The process eventually failed with:

```text
Invalid Services found fml
```

**Conclusion**

Forge 1.16.5 requires a version-specific launch pipeline. Substituting newer FML or ModLauncher components is not a reliable compatibility strategy.

---

# 4. Forge 1.21.4 Startup Problems — ModLauncher 10.2.4

> **Updated status:** Forge 1.21.4 has now achieved experimental startup success in the project. The issues below document the earlier failures and the investigation that led to the working configuration.

## 4.1 ModLauncher JAR Discovery Failure

<badge color="success">Overcome in experimental startup</badge>

**Symptoms**

The launcher reported:

```text
ERROR: ModLauncher JAR not found
```

**Root cause**

The Rust launcher expected a particular JAR location and filename, but the local installation contained differently named or duplicated ModLauncher artifacts.

**Investigation**

Inspected the actual library directory and verified the expected artifact paths.

The launcher path-construction logic was adjusted to match the installed dependency layout.

**Result**

The ModLauncher JAR discovery problem was overcome in the later experimental startup configuration.

---

## 4.2 Missing Dependencies on the Module Path

<badge color="success">Overcome in experimental startup</badge>

**Symptoms**

The JVM reported missing module-path dependencies involving components such as:

- SecureModules
- ForgeSPI
- FMLLoader
- Bootstrap API
- ASM

**Root cause**

A JAR existing on disk does not mean that the JVM can resolve it as a JPMS module.

Classpath and module-path entries serve different purposes.

The initial configuration did not consistently reproduce the module layout expected by the Forge bootstrap process.

**Investigation**

Used tools such as:

```bash
jar --describe-module --file dependency.jar
```

and:

```bash
javap -classpath dependency.jar -p some.package.SomeClass
```

Inspected the module descriptors, dependency paths, and duplicate entries.

**Result**

The module-path construction was revised, and the Forge 1.21.4 startup experiment subsequently progressed successfully.

---

## 4.3 Bootstrap Service Discovery Failure

<badge color="success">Overcome in experimental startup; implementation details remain version-specific</badge>

**Symptoms**

The expected service descriptor was initially missing:

```text
META-INF/services/net.minecraftforge.bootstrap.api.BootstrapEntryPoint
```

**Root cause**

The initial investigation found a mismatch between the expected service interface, implementation classes, and service registration mechanism.

However, the missing descriptor alone did not establish the complete root cause.

**Attempts**

- Inspected the JAR module descriptors.
- Inspected service registration files.
- Tested injecting a service descriptor into a processed ModLauncher JAR.

**Result**

The bootstrap configuration was revised sufficiently for the later Forge startup experiment.

Manual service descriptor injection should not be treated as a universal solution; the actual service provider mechanism must match the Forge version.

---

## 4.4 Bootstrap JAR Injection Did Not Initially Solve Startup

<badge color="success">Overcome in later configuration</badge>

**Symptoms**

Even after the bootstrap JAR was injected at the beginning of the classpath, the launcher continued to report errors resembling:

```text
ModLauncher JAR not found
forge-client JAR missing
```

**Root cause**

Successful classpath insertion does not guarantee:

- Correct file discovery.
- Correct module resolution.
- Correct service loading.
- Correct bootstrap entry-point selection.

These are separate stages of the Java launch process.

**Investigation**

Adjusted classpath ordering, removed duplicate dependency entries, and revised Rust-side JVM argument construction.

**Current status**

Forge 1.21.4 has experimentally reached the early loading stage.

Further testing is required to establish full client startup and stable gameplay.

---

## 4.5 Duplicate ModLauncher Modules and `UnionFileSystemProvider`

<badge color="warning">Requires further verification</badge>

**Symptoms**

The Forge investigation encountered duplicate ModLauncher-related artifacts and module-resolution conflicts.

One reported conflict involved:

```text
UnionFileSystemProvider
```

**Root cause**

Multiple dependency versions or duplicate module entries can cause the JVM to resolve conflicting implementations or encounter module-layer initialization failures.

The presence of similarly named JARs in the library directory made it important to distinguish between the actual selected artifact and unused alternatives.

**Investigation**

- Inspected ModLauncher 10.2.4.
- Checked the launcher constructor and bytecode with `javap`.
- Inspected module-path construction.
- Checked for duplicate ModLauncher entries.
- Compared the expected module layout against the local Forge installation.

**Current status**

The later Forge 1.21.4 startup experiment succeeded, but the dependency-selection logic should still be hardened against duplicate modules.

---

# 5. Rust, JNI, and JVM Integration Problems

## 5.1 JNI 0.22 API Changes

<badge color="success">Resolved</badge>

**Symptoms**

The Rust JNI integration produced numerous compilation errors after moving to JNI crate version 0.22.

**Root cause**

The JNI crate API changed substantially compared with earlier versions.

Several APIs, string conversion patterns, and object construction methods no longer matched the previous implementation.

**Solution**

Updated the Rust integration to use the new API patterns.

Important components include:

```rust
InitArgsBuilder
JNIVersion::V1_8
jni_str!
jni_sig!
```

The required `invocation` feature was also enabled.

For example, `JObjectArray` construction requires the appropriate object initialization arguments, including:

```rust
JObject::null()
```

where applicable.

**Verification**

Rust successfully launches an embedded JVM and invokes Java methods through JNI.

---

## 5.2 Java Module Encapsulation and `InaccessibleObjectException`

<badge color="success">Workaround implemented</badge>

**Symptoms**

Java 17 and Java 21 environments produced errors such as:

```text
InaccessibleObjectException
```

Some failures involved:

```text
sun.security.util
java.util.jar
```

**Root cause**

The Java Platform Module System restricts reflective access to internal packages.

Older libraries and launchers may assume unrestricted access to these APIs.

**Solution / workaround**

Added the required module-access arguments to the JVM launch configuration.

Examples:

```text
--add-exports java.base/sun.security.util=ALL-UNNAMED
--add-opens java.base/java.util.jar=ALL-UNNAMED
```

Additional packages must be opened only when required by the selected loader and dependency versions.

**Remaining concern**

Excessive use of `--add-opens` and `--add-exports` can conceal deeper compatibility problems. The long-term objective is to minimize these flags and use version-appropriate launch configurations.

---

## 5.3 `UnsupportedClassVersionError` from Library Scanning

<badge color="success">Filtering workaround implemented</badge>

**Symptoms**

The JVM reported:

```text
UnsupportedClassVersionError
```

Examples of encountered class-file major versions included:

```text
60.0
61.0
69.0
```

**Root cause**

The local Minecraft libraries directory contained multiple generations of Java dependencies and mod-loader components.

Broad directory scanning could accidentally include classes compiled for a newer Java runtime.

For example:

| Class-file major version | Java version |
|---|---|
| 52 | Java 8 |
| 60 | Java 16 |
| 61 | Java 17 |
| 65 | Java 21 |
| 69 | Java 25 |

The exact compatibility requirement depends on the Minecraft and loader versions being launched.

**Solution / workaround**

Implemented Python-based JAR inspection that reads class-file headers and filters out incompatible classes where the legacy environment requires an older Java version.

**Remaining concern**

Filtering class files is not a substitute for dependency resolution.

The preferred approach is to select the correct version of each dependency before constructing the classpath.

---

## 5.4 JVM Argument Ordering

<badge color="success">Resolved in tested launch configurations</badge>

**Symptoms**

Some JVM properties and module arguments were not interpreted as expected during the launch process.

**Root cause**

JVM options, system properties, classpath arguments, and application arguments must appear in the correct positions.

For example, JVM options placed after the main class are interpreted as application arguments rather than JVM configuration.

**Solution**

Revised the Rust launch command construction to keep JVM options before the application entry point.

This includes:

- `-D` system properties.
- `-X` runtime options.
- `--add-opens`.
- `--add-exports`.
- `--module-path`.
- Classpath arguments.

**Verification**

The tested Fabric and NeoForge launch configurations progressed beyond the earlier argument-ordering failures.

---

# 6. Rust DataBus and IPC Problems

## 6.1 DataBus Connection Refused

<badge color="success">Resolved</badge>

**Symptoms**

Java Agents reported:

```text
Connection refused
```

when attempting to connect to:

```text
127.0.0.1:25565
```

**Root cause**

The Java Agent attempted to connect before the Rust DataBus had successfully bound its TCP listener.

The original asynchronous startup arrangement made the listener startup race harder to detect.

In Fusion mode, multiple child processes also needed to share a single DataBus rather than independently attempting to bind the same address.

**Solution**

Changed the Rust DataBus startup function:

```rust
fn start_data_bus() -> std::io::Result<()>
```

The listener is now bound synchronously before the accept thread is spawned.

The startup error is propagated to the caller.

Fusion mode starts one shared DataBus in the parent process and sets:

```text
JVM_COMBINER_BUS_EXTERNAL=1
```

in both child environments.

Standalone Fabric and NeoForge modes start their own DataBus unless this environment variable is present.

**Verification**

Standalone NeoForge successfully produced logs resembling:

```text
[DataBus] 路由总线已启动，监听 127.0.0.1:25565
[DataBus] 127.0.0.1:59996 注册为 'neoforge'
[BusAgent:neoforge] Connected to DataBus
```

Both Fabric and NeoForge have also successfully registered with the same Fusion DataBus in a concurrent launch.

---

## 6.2 Client Registration and Message Routing

<badge color="success">Verified prototype</badge>

**Symptoms**

Both clients could connect to TCP, but messages needed a reliable destination and source identity.

**Solution**

Implemented named client registration and JSON message routing.

The clients register using messages such as:

```json
{"register":"fabric"}
```

and:

```json
{"register":"neoforge"}
```

Messages use a destination field:

```json
{
  "to": "neoforge",
  "type": "player_move",
  "x": 3.0,
  "y": 64.0,
  "z": 8.5
}
```

The Rust DataBus forwards messages to the registered destination and attaches the sender identity as `from`.

**Verification**

The following communication pattern has been verified:

```text
Fabric -> DataBus -> NeoForge
NeoForge -> DataBus -> Fabric
```

The `hello` / `ack` routing test succeeded.

**Important distinction**

The current implementation is TCP-based IPC.

It is not a shared-memory bus and does not provide true zero-copy transfer semantics.

---

## 6.3 DataBus Disconnection and Reconnection

<badge color="warning">Requires additional stress testing</badge>

**Symptoms**

A sandbox may lose its TCP connection while the other sandbox continues running.

Potential consequences include:

- Lost movement messages.
- Stale player state.
- Delayed synchronization after reconnection.
- A sender believing that an old state has already been transmitted.

**Existing implementation**

The Java Agent attempts to reconnect when its socket connection fails.

The Rust DataBus maintains separate client connections and writer channels.

**Remaining work**

- Explicit connection-state tracking.
- Reconnection backoff.
- State resynchronization after reconnect.
- Heartbeat monitoring.
- Handling duplicate or stale messages.
- Detecting disconnected destination clients.

A successful TCP write should not be treated as proof that the remote game has applied the state.

---

## 6.4 Concurrent TCP Writes

<badge color="success">Improved</badge>

**Symptoms**

Multiple threads may attempt to write messages to the same client connection.

Without synchronization, JSON messages could become interleaved or corrupted.

**Solution**

The Rust DataBus uses a separate writer thread and an `mpsc` channel for each connection.

Conceptually:

```text
Client Handler
      |
      v
  mpsc Sender
      |
      v
  Writer Thread
      |
      v
  TCP Stream
```

This keeps actual socket writes serialized for each connection.

The Java Agent also synchronizes its writes around the shared `PrintWriter`.

**Remaining concern**

Connection replacement and shutdown behavior should continue to be tested under concurrent reconnect conditions.

## 6.5 One-Way State Authorization and Registration Identity

<badge color="warning">Policy implemented in Rust source; runtime verification pending</badge>

**Requirement**

Fabric is the sole publisher of authoritative `player_move` and monster-state messages. A NeoForge client must not publish these message types, and a rejected message must not be forwarded or disconnect the sender.

**Current implementation**

`start_data_bus()` in `src/main.rs` assigns a connection ID, requires registration as the first message, and derives the routed `from` value from the registered connection name rather than a message's payload. Before forwarding, it rejects non-`fabric` messages of type:

```text
player_move
monster_spawn
monster_update
monster_remove
```

The sender receives a `state_authority_violation` JSON response, and the handler continues reading that connection. This behavior is implemented in source; an end-to-end runtime test is still needed to verify rejection, no forwarding, and continued client connectivity.

**Important limitation**

The DataBus binds to `127.0.0.1`, but registration names are currently self-asserted. The server does not authenticate the connecting process or restrict names to a fixed allowlist. Therefore, the registered name is a connection-scoped routing identity, not proof that the process is Fabric or NeoForge. A local client can claim a name, and a later registration can replace the active route for that name.

**Next steps**

- Add validation for allowed registration names and decide how local client identity should be trusted.
- Test unauthorized sends from a registered NeoForge client and assert that the connection remains usable.
- Test that protected messages are never delivered to another peer.
- Preserve only explicitly defined reverse-direction message types.

# 7. Java Agent and Runtime Instrumentation Problems

Java sources currently reside in `bus_test/`; there is no `bus-agent/` source directory in the repository.

## 7.1 Java Agent Injection and Class Transformation

<badge color="success">Verified</badge>

**Objective**

Inject runtime instrumentation without requiring direct Mixin integration for every diagnostic or synchronization task.

**Implementation**

The Rust launcher supplies a Java Agent through JVM startup arguments:

```text
-javaagent:/path/to/bus-agent.jar
```

The Agent uses:

```java
public static void premain(
    String agentArgs,
    Instrumentation instrumentation
)
```

A `ClassFileTransformer` observes classes loaded by the Fabric runtime.

**Verification**

Example output:

```text
[BusAgent:fabric] premain
[BusAgent:fabric] MC-CLIENT: net/minecraft/client/main/Main loader=KnotClassLoader
[BusAgent:fabric] MC-CLIENT: net/minecraft/client/ClientBrandRetriever loader=KnotClassLoader
```

This establishes that the Agent can be initialized and can observe classes loaded by Fabric's KnotClassLoader.

It does not mean that arbitrary runtime class modification or Mixin conflict resolution has already been implemented.

---

## 7.2 Fabric Player Monitor NullPointerException

<badge color="success">Resolved</badge>

**Symptoms**

The Fabric monitoring thread repeatedly produced `NullPointerException` errors.

The failure originated around:

```text
BusAgent.monitorFabricPlayer
```

**Root cause**

The monitor attempted to inspect the Minecraft instance before Fabric had fully initialized its game instance.

The relevant reference was still:

```java
fabricMcInstance == null
```

**Solution**

Added null protection and a wait/retry path.

The monitor now waits for the Minecraft instance before attempting player discovery.

**Verification**

The monitor produced clean initialization output:

```text
[BusAgent:fabric][INIT] SUCCESS: flk
```

The earlier repeated NPE spam was eliminated.

---

## 7.3 NeoForge Player Discovery Initialization Race

<badge color="success">Resolved for player discovery</badge>

**Symptoms**

The DataBus could receive Fabric `player_move` messages while NeoForge still had:

```java
nfMcInstance == null
```

or:

```java
nfMcClass == null
```

The original receive-side initialization path depended on incoming synchronization messages.

If a message arrived before the Minecraft client was ready, the synchronization handler could not safely access the game state.

**Root cause**

NeoForge's Minecraft lifecycle and the DataBus connection lifecycle were independent.

The original implementation initialized the NeoForge Minecraft reference only when the receive handler needed it.

This created a timing dependency between remote messages and local client initialization.

**Solution**

Added an independent NeoForge monitoring thread.

The monitor repeatedly:

1. Searches for the loaded `net.minecraft.client.Minecraft` class.
2. Calls the static `getInstance()` method.
3. Waits for a non-null Minecraft instance.
4. Schedules readiness checks through `Minecraft.execute()`.
5. Checks `ClientLevel`.
6. Checks `LocalPlayer`.
7. Logs readiness transitions.

The monitor also uses an atomic flag to avoid scheduling duplicate probes.

**Verification**

The following messages were observed:

```text
[BusAgent:neoforge][MONITOR] thread started
[BusAgent:neoforge][INIT] Minecraft instance found
[BusAgent:neoforge][INIT] ClientLevel ready
[BusAgent:neoforge][LIVE] LocalPlayer ready
```

This verifies that NeoForge player discovery works in the tested client session.

It does not yet verify bidirectional state synchronization.

---

## 7.4 Runtime Reflection and Minecraft Mapping Differences

<badge color="warning">Version-specific implementation</badge>

**Symptoms**

Minecraft methods are not always accessible through the same method names across different runtime environments.

Directly using development names can fail when the runtime uses obfuscated names.

**Fabric implementation**

The tested Fabric 1.21.4 environment required reflective access to methods identified through local mappings and runtime inspection.

The current implementation uses method names including:

```text
dA
dC
dG
dL
dN
cj
```

These correspond to the player position, rotation, and sprint-state accessors in the tested runtime.

**NeoForge implementation**

NeoForge uses a different runtime setup.

The NeoForge sender is designed to resolve methods independently rather than reusing Fabric's method cache.

The expected Mojang-style method names include:

```java
getX()
getY()
getZ()
getYRot()
getXRot()
isSprinting()
```

**Remaining work**

- Resolve method mappings dynamically.
- Verify mappings against each exact Minecraft version.
- Avoid hardcoded names where practical.
- Detect missing or incompatible methods without repeatedly flooding the logs.
- Support future Minecraft versions.

---

# 8. Cross-Version Player Mirroring and State Synchronization

Tested version combination:

```text
Fabric     Minecraft 1.21.4
NeoForge   Minecraft 1.21.1
```

## 8.1 NeoForge Remote Player Initialization

<badge color="success">Prototype implemented</badge>

**Objective**

Create a remote player entity in the NeoForge sandbox to represent the Fabric local player.

**Implementation**

The NeoForge Agent uses reflection to access Minecraft runtime classes and construct a `RemotePlayer`.

The mirror is given a recognizable name:

```text
FabricMirror
```

Its UUID is derived from a stable identifier associated with the Fabric mirror.

A high candidate entity ID is used to reduce the risk of colliding with ordinary game entities.

**Verification**

The NeoForge client successfully displayed the injected mirror player in the tested environment.

This proves that the NeoForge runtime can accept a remotely controlled player entity through the current reflection-based prototype.

**Remaining work**

- Stable UUID management.
- Entity ID collision handling.
- Duplicate entity prevention.
- Respawn and disconnect handling.
- Dimension transitions.
- Player removal when the source disconnects.
- Compatibility with different Minecraft versions.

---

## 8.2 Fabric and NeoForge Mirror Appearing in Different Environments

<badge color="warning">Under investigation</badge>

**Symptoms**

Fabric displayed the local player in a forest environment.

NeoForge also displayed the `FabricMirror` entity, but the mirror appeared in an underground enclosed area.

The two clients did not appear to show the same surrounding environment.

**Possible causes**

The clients were running different Minecraft versions:

```text
Fabric:   1.21.4
NeoForge: 1.21.1
```

Potential explanations include:

- Different terrain-generation behavior.
- Different world seeds.
- Different dimension state.
- Different chunk availability.
- Different client world initialization timing.
- Different coordinates or coordinate interpretation.
- Differences in the mirror entity's initialization and positioning.

The exact cause has not been confirmed.

**Current investigation**

The DataBus successfully forwarded Fabric's player coordinates, and NeoForge successfully displayed the mirror entity.

The next step is to compare both clients':

- X, Y, and Z coordinates.
- Dimension identifiers.
- World seeds.
- Loaded chunk positions.
- Local-player position.
- Mirror-player position.

**Remaining work**

Establish whether the discrepancy originates from world generation, world state, chunk loading, or entity synchronization.

A matching coordinate alone does not guarantee identical terrain or world state across different Minecraft versions.

---

## 8.3 Minecraft Automatically Pauses When the Window Loses Focus

<badge color="warning">Requires final dual-client verification</badge>

**Symptoms**

When switching to a browser or another application, one or both Minecraft clients automatically entered the ESC pause menu.

This interfered with testing two clients simultaneously.

**Investigation**

Checked Minecraft's automatic pause behavior when the game window loses focus.

Tested the relevant keyboard setting:

```text
F3 + P
```

Also inspected the individual client configurations:

```text
options.txt
```

**Remaining work**

Verify that both clients continue running after focus changes.

In particular, confirm that:

- The world simulation continues.
- The game thread continues processing scheduled tasks.
- Player-state sampling continues.
- DataBus messages continue to be transmitted.
- Mirror entities continue to update.

Disabling automatic pause is not by itself proof that every synchronization task remains active.

---

## 8.4 Fabric-to-NeoForge State Synchronization

<badge color="success">Proof of concept verified</badge>

**Implementation**

The Fabric Agent:

1. Locates the Fabric Minecraft instance.
2. Discovers the local player.
3. Extracts coordinates, rotation, and sprint state.
4. Constructs a JSON `player_move` message.
5. Sends it to the Rust DataBus.

The DataBus routes the message to NeoForge.

The NeoForge Agent receives and parses the message and updates the mirror entity through the Minecraft game thread.

**Message example**

```json
{
  "to": "neoforge",
  "type": "player_move",
  "x": 3.0,
  "y": 64.0,
  "z": 8.5,
  "yaw": 90.0,
  "pitch": 0.0,
  "sprint": false
}
```

**Verified capabilities**

- Fabric player-state extraction.
- TCP message transmission.
- Rust DataBus routing.
- NeoForge message reception.
- NeoForge JSON parsing.
- NeoForge mirror entity update.

**Remaining work**

The current implementation is a proof of concept. It requires more testing for movement smoothness, network latency, entity lifecycle, and version compatibility.

---
## 8.5 NeoForge-to-Fabric Synchronization - Historical Design

<badge color="warning">Superseded by the one-way authority architecture</badge>

**Historical objective**

The original prototype considered a reverse player-state synchronization path:

```text
NeoForge LocalPlayer
       |
       v
NeoForge Java Agent
       |
       v
Rust DataBus
       |
       v
Fabric Java Agent
```
The original design attempted to make both sandboxes publish their local-player movement state and mirror the remote player.

### Why this design was retired

Bidirectional authoritative player-state synchronization creates two competing sources of truth.

Different Minecraft versions, independent game loops, and different world states introduce conflicts that require additional ownership, conflict-resolution, and feedback-loop prevention mechanisms.

The current prototype does not need two authoritative player-state publishers.

### Current implementation status
- The NeoForge Agent monitor currently consumes `player_move` and does not sample/publish its local player state.
- NeoForge is intended to operate as a receiver-only client.
- The Rust DataBus source now rejects non-Fabric registered clients attempting to publish `player_move` or monster state messages.
- This source-level rejection has not yet been documented as runtime-tested; verify that rejected sends are not routed and do not close the client connection.
- Registration names remain self-asserted; the one-way policy trusts the first `register` message and is not process authentication.

### Current architecture policy

Fabric is the sole authoritative publisher of player movement state.

NeoForge receives Fabric player_move messages and updates its remote-player mirror. NeoForge must not publish authoritative player_move messages back to Fabric.

Future reverse-direction messages may include explicitly defined interaction requests, acknowledgements, diagnostics, or capability reports. Each requires its own protocol definition and authorization rules.

**Remaining work**

- Rebuild and package the current Java Agent source from `bus_test/`.
- Verify that NeoForge no longer publishes local-player movement state.
- Runtime-test Rust-side rejection based on the registered TCP connection name.
- Add registration-name validation/authentication and prevent unauthorized route replacement.
- Verify that permitted reverse-direction message types remain supported.

The historical bidirectional design is retained for architectural context.

## 8.6 Hostile-Mob Synchronization Is an Unverified Working-Tree Prototype

<badge color="warning">Java source implementation added; build/runtime verification pending</badge>

The current working-tree `bus_test/MonsterSync.java` runs from the Fabric monitor on the client thread. It samples visible, living hostile `Monster` entities at a maximum of 5 Hz, tracks them by UUID, and emits `monster_spawn` for newly observed mobs and `monster_update` when position, rotation, type, or health changes. Pending states carry sequence numbers, are retried with backoff, and are confirmed by a `monster_ack`. The Rust DataBus permits protected monster-state messages only from the registered `fabric` name.

The current working-tree `BusAgent.java` also parses `monster_spawn` and `monster_update` on NeoForge, creates or updates local entity mirrors, clears those mirrors when it observes a world change, and sends an ACK after applying a message. The Fabric side accepts those ACKs. This is an implementation in the uncommitted working tree, not proof that the packaged Agent or live clients run it successfully. The checked-in `main` history does not yet include these Java changes.

No `monster_remove` message is published yet. Mobs missing from the Fabric render iteration are deliberately not treated as removed, since absence from that iteration is not proof of despawn. The current Java Agent JAR must be rebuilt from source, and end-to-end behavior—including retries, ACKs, world transitions, and reconnect recovery—still needs runtime testing.

**Remaining work**

- Rebuild/package the current Java Agent source and verify the packaged classes match it.
- Define reliable despawn and world-change semantics.
- Publish removal events without inferring despawn from render-list absence.
- Runtime-test entity creation/update, ACK/retry ordering, reconnect snapshots, and cleanup before calling this synchronized.

# 9. ClassLoader Isolation and Mixin Compatibility

<badge color="warning">Major unresolved research area</badge>

## 9.1 `parent = null` Is Not a Complete Isolation Strategy

**Problem**

The original architecture considered using:

```java
new URLClassLoader(urls, null)
```

to isolate different loader environments.

However, a completely isolated parentless ClassLoader cannot automatically access the Java platform classes and other shared runtime dependencies required by real loaders.

**Challenges**

Real Minecraft loaders depend on:

- `java.lang.*`
- `java.util.*`
- Reflection APIs.
- Java networking.
- Native library interfaces.
- Runtime services.
- Shared platform interfaces.

**Research direction**

Investigate a controlled parent ClassLoader or hybrid delegation model.

The objective is to isolate loader-specific classes without breaking access to required Java platform classes.

---

## 9.2 Mixin Conflicts Across Independent Loader Environments

<badge color="warning">Not solved</badge>

**Problem**

Fabric and Forge/NeoForge use different Mixin integration and transformation pipelines.

Running both environments in a shared JVM introduces questions about:

- Transformation ordering.
- Duplicate target transformations.
- Class definition ownership.
- Injection conflicts.
- Static state shared across class loaders.
- Transformation visibility between isolated environments.

**Potential approaches**

- Independent transformation environments.
- Explicit transformation ownership.
- Bytecode inspection and conflict detection.
- Priority-based transformation rules.
- Controlled compatibility interfaces.

No general-purpose Mixin conflict solution has been implemented yet.

---

## 9.3 Shared Minecraft Classes and Native Runtime State

<badge color="warning">Not solved</badge>

**Problem**

ClassLoader isolation is difficult when two environments depend on classes representing similar or identical Minecraft runtime concepts.

Even if two ClassLoaders can load the same class name independently, the resulting Java types are not interchangeable.

This creates problems for:

- Entity references.
- Minecraft client instances.
- Static singleton fields.
- Event objects.
- Rendering state.
- Native resources.
- Shared game data.

**Research direction**

Keep loader-specific objects inside their own runtime boundaries and exchange serialized data through explicit interfaces.

Avoid passing arbitrary Java objects directly between isolated environments.

---

# 10. Performance and Synchronization Challenges

## 10.1 Different Minecraft Tick Rates

<badge color="warning">Not fully investigated</badge>

**Problem**

Different Minecraft clients may not process game ticks at identical rates.

Possible causes include:

- Different frame rates.
- Different CPU scheduling.
- Different JVM garbage collection behavior.
- Different world loading times.
- Different client pause states.

A simple position update is therefore not guaranteed to arrive at the same logical tick on both clients.

**Future work**

Investigate:

- Timestamped messages.
- Sequence numbers.
- Interpolation.
- Extrapolation.
- Stale-state detection.
- Bounded update queues.

---

## 10.2 Lockstep Synchronization and Double Stuttering

<badge color="warning">Design concern</badge>

**Problem**

A strict lockstep architecture could make both clients wait for each other's game-state progress.

This risks producing a double-stuttering problem:

1. One client waits for a synchronization message.
2. The other client is delayed by its own frame or tick processing.
3. Both clients experience additional latency.
4. The synchronization delay propagates between the two environments.

**Design direction**

The initial implementation should avoid making the Rust kernel a global game-tick authority.

Instead, investigate asynchronous state replication with:

- Local simulation.
- Timestamped updates.
- Bounded buffering.
- Interpolation.
- Explicit consistency rules.

The DataBus should transport state rather than control every game tick.

---

## 10.3 No-Cheat and Server-Authoritative Interaction

<badge color="warning">Future research</badge>

**Problem**

Some players and server environments impose strict no-cheat requirements.

Directly injecting remote entities or modifying client-side state may not be acceptable in such environments.

Player movement synchronization alone does not solve:

- Server-side authority.
- Collision validation.
- Inventory consistency.
- Attack registration.
- Interaction ordering.
- Anti-cheat compatibility.

**Design direction**

Prioritize version compatibility and entity-state replication before implementing left-click and right-click synchronization.

Future interaction synchronization should distinguish between:

- Client-side visual state.
- Server-authoritative game actions.
- Validated interaction events.
- Replicated results.

The project should not assume that client-side entity injection is equivalent to a legitimate server-side player.

---

# 11. Dependency Resolution and Cross-Platform Support

## 11.1 Static Dependency Paths

<badge color="warning">Partially implemented</badge>

**Problem**

Some experimental launch configurations still rely on manually inspected dependency paths or copied JAR files.

This makes the launcher sensitive to:

- Minecraft version changes.
- Loader version changes.
- Library updates.
- Different installation layouts.

**Current implementation**

The Fabric dependency resolver reads Minecraft version metadata and applies platform rules.

Some Forge and NeoForge configurations still require more specialized module-path handling.

**Remaining work**

Implement a unified dependency-resolution layer that understands:

- Minecraft version manifests.
- Fabric loader dependencies.
- Forge and NeoForge installer metadata.
- JPMS module dependencies.
- Platform-specific native libraries.
- Dependency checksums.

---

## 11.2 macOS-Specific Launch Behavior

<badge color="warning">Partially handled</badge>

**Problem**

Some launch requirements are specific to macOS.

Examples include:

```text
-XstartOnFirstThread
```

and the selection of ARM64 native libraries.

**Current implementation**

The launcher includes platform-specific dependency filtering and native library paths for the tested macOS environment.

**Remaining work**

Validate the same launch pipeline on:

- macOS Intel.
- macOS Apple Silicon.
- Windows x86-64.
- Linux x86-64.
- Linux ARM64, if supported by the selected Minecraft and loader versions.

---

# 12. Summary of Current Problem Status

| Area | Current status |
|---|---|
| Rust JNI integration | <badge color="success">Verified</badge> |
| Basic URLClassLoader isolation | <badge color="success">Verified</badge> |
| Fabric 1.21.4 startup | <badge color="success">Verified</badge> |
| NeoForge 1.21.1 startup | <badge color="success">Verified</badge> |
| Forge 1.21.4 startup | <badge color="success">Experimental success</badge> |
| Forge 1.16.5 startup | <badge color="danger">Unresolved</badge> |
| Rust TCP DataBus | <badge color="success">Verified</badge> |
| DataBus connection startup race | <badge color="success">Resolved</badge> |
| Named client registration and routing | <badge color="success">Verified</badge> |
| Source-level one-way state message rejection | <badge color="warning">Implemented; runtime test pending</badge> |
| Authenticated sandbox registration | <badge color="warning">Not implemented</badge> |
| Java Agent injection | <badge color="success">Verified</badge> |
| Fabric player discovery | <badge color="success">Verified</badge> |
| NeoForge player discovery | <badge color="success">Verified</badge> |
| Fabric-to-NeoForge state transfer | <badge color="success">PoC verified</badge> |
| NeoForge RemotePlayer injection | <badge color="success">PoC verified</badge> |
| Fabric hostile-mob sampling, spawn/update, and ACK/retry | <badge color="warning">Implemented in current working-tree source; runtime verification pending</badge> |
| NeoForge hostile-mob receive/mirroring | <badge color="warning">Implemented in current working-tree source; runtime verification pending</badge> |
| Hostile-mob ACK/retry delivery | <badge color="warning">Implemented in current working-tree source; runtime verification pending</badge> |
| Bidirectional player mirroring | <badge color="warning">Not a current goal</badge> |
| Cross-version world consistency | <badge color="warning">Under investigation</badge> |
| Same-JVM real-loader isolation | <badge color="warning">Not solved</badge> |
| General Mixin conflict resolution | <badge color="warning">Not solved</badge> |
| True zero-copy DataBus | <badge color="warning">Not implemented</badge> |
| General mod interoperability | <badge color="warning">Not implemented</badge> |
| Cross-platform verification | <badge color="warning">Incomplete</badge> |

---

# 13. Recommended Development Priorities

The following order minimizes the risk of expanding the architecture before the current prototype is stable.

### Priority 1 — Enforce and Stabilize One-Way Player-State Authority

**Goal:** Enforce Fabric as the sole authoritative player-state publisher and keep NeoForge receiver-only.

- [x] Establish Fabric as the sole authoritative player-state source.
- [x] Remove the active NeoForge player-state sampling call from the edited monitor source.
- [x] Implement source-level rejection of protected player/monster state message types from non-Fabric registrations.
- [ ] Rebuild and package the current Java Agent source from `bus_test/`.
- [ ] Validate or authenticate DataBus registration names; current names are client-supplied.
- [ ] Runtime-test that rejected state messages are not routed and do not disconnect the sender.
- [ ] Verify Fabric-to-NeoForge player-state delivery with the packaged Agent.
- [ ] Test reconnect behavior and fresh state transmission.
- [ ] Add protocol versioning, sequence numbers, timestamps, and session generations.
- [ ] Reject stale or out-of-session player-state updates.

### Priority 2 — Complete the Hostile-Mob Synchronization Prototype

- [x] Sample hostile Fabric mobs and emit initial/change messages from Java source.
- [x] Add current working-tree NeoForge spawn/update receive, entity-mirror, world-change cleanup, and ACK/retry source.
- [ ] Implement and test removal detection and `monster_remove` publication.
- [ ] Rebuild/package the Java Agent and runtime-test the end-to-end path before describing mob synchronization as a feature.

### Priority 3 — Improve Entity Lifecycle Management

- [ ] Stable remote-player UUID.
- [ ] Entity ID collision prevention.
- [ ] Spawn and despawn handling.
- [ ] Dimension-change handling.
- [ ] Disconnect cleanup.

### Priority 4 — Improve Synchronization Quality

- [ ] Timestamped messages.
- [ ] Sequence numbers.
- [ ] Stale-state detection.
- [ ] Interpolation.
- [ ] Network latency measurement.
- [ ] Tick-rate independence.

### Priority 5 — Expand Cross-Version Compatibility

- [ ] Verify method mappings dynamically.
- [ ] Test identical Minecraft versions on both sides.
- [ ] Test different world seeds.
- [ ] Compare dimension and chunk state.
- [ ] Test multiple loader versions.

### Priority 6 — Return to the Original Microkernel Architecture

- [ ] Develop a controlled ClassLoader delegation model.
- [ ] Investigate real-loader isolation inside one JVM.
- [ ] Define Mixin transformation boundaries.
- [ ] Develop a stable shared data interface.
- [ ] Investigate shared-memory transport.

## Development Order

The intended implementation order is:

1. Rebuild and package the current Java Agent from `bus_test/`.
2. Constrain the self-asserted DataBus registration identity.
3. Runtime-test protected-message rejection, no forwarding, and sender continuity.
4. Verify Fabric-to-NeoForge player-state delivery with the packaged Agent.
5. Test heartbeat timeout, mirror removal, reconnect, and fresh-state recovery.
6. Rebuild/runtime-test the NeoForge hostile-mob receiver and ACK/retry path; implement reliable removal.
7. Improve entity lifecycle handling and failure recovery.
8. Add protocol versioning, timestamps, sequence numbers, and session generations.
9. Expand entity and gameplay synchronization features.
10. Continue launcher refactoring and cross-version compatibility testing.

## Current Development Focus

The current focus is validating the Rust source-level one-way state policy at runtime, constraining registration identity, and rebuilding/testing the working-tree Java Agent changes for player and hostile-mob mirror lifecycle.

**Next milestone:**

Rebuild/package the Java Agent and verify reliable Fabric-to-NeoForge player and hostile-mob synchronization while testing that unauthorized state messages are rejected without forwarding or disconnecting the sender.
---

## Final Note

JVM Mod Combiner is an experimental systems project.

A successful Minecraft launch, a successful TCP message transfer, and a successful remote-player injection are separate engineering milestones. None of them alone establishes complete cross-loader compatibility.

The project deliberately records unsuccessful experiments and incomplete solutions because understanding the failure modes is essential to designing a stable runtime architecture.

The immediate objective is to make the existing two-process prototype reliable before attempting more ambitious same-JVM isolation and general mod interoperability.
