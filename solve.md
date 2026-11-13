# JVM Mod Combiner: Architectural Challenges & Proposed Solutions

> This document complements [Problems.md](problems.md).
>
> `Problems.md` records the technical problems encountered while building the current prototype and the solutions or workarounds implemented so far.
>
> `SOLVE.md` looks ahead. It identifies the major architectural challenges that must be addressed before JVM Mod Combiner can evolve from an experimental prototype into a practical cross-loader platform.
>
> The solutions described here are engineering proposals, not completed features. They will be evaluated and revised as implementation progresses.

---

## 1. The Double-Stuttering Problem: Lockstep Synchronization

### 1.1 The Core Problem

One early architectural idea was to let Rust act as a global clock, forcing the Fabric and Forge/NeoForge sandboxes to execute their game ticks in synchronization.

However, JVM garbage collection introduces an unavoidable source of unpredictable pauses.

Consider the following scenario:

- Fabric experiences a 30 ms stop-the-world (STW) garbage collection pause.
- NeoForge is ready to continue its next tick.
- A strict lockstep scheduler forces NeoForge to wait for Fabric.
- Both clients experience the resulting delay, even though only one JVM originally stalled.

This creates unnecessary synchronization overhead and can amplify frame-time instability.

The problem becomes more serious when either sandbox experiences chunk loading, resource reloads, large entity updates, or expensive mod operations.

### 1.2 Proposed Solution: Asymmetric Decoupling and Local State Freezing

**Abandon global lockstep synchronization.**

Rust should operate as an asynchronous coordination layer rather than a global game clock.

The proposed design has three mechanisms:

**1. Independent Tick Execution**

- Fabric and NeoForge maintain independent game loops.
- Neither sandbox waits for the other to finish a tick.
- Rust forwards state updates asynchronously.
- Each receiver processes incoming updates when its own game thread is ready.

**2. Ghost Freeze**

If one sandbox temporarily stops producing updates, the other sandbox should not freeze its entire game.

Instead, only the affected mirrored entity should enter a temporary frozen state.

For example, if Fabric experiences a GC pause:

- NeoForge continues rendering and processing its own world.
- The Fabric mirror temporarily stops moving.
- The mirror resumes interpolation when new updates arrive.

This isolates synchronization delays to the affected entity rather than the entire client.

**3. Heartbeat and Timeout Detection**

The Rust DataBus should monitor connection health and message timestamps.

A possible initial policy:

| Condition | Proposed behavior |
|---|---|
| No update for 100 ms | Mark the mirror as potentially delayed |
| No update for 200 ms | Enter a stale or frozen state |
| Connection lost | Mark the remote player offline |
| Connection restored | Resume synchronization and request a fresh state |

These values are initial engineering targets, not fixed requirements.

The system should distinguish between a temporarily delayed client and a disconnected client.

---

## 2. Distributed Game Logic: The Rollback and Consistency Problem

### 2.1 The Core Problem

Minecraft mod behavior is not necessarily deterministic.

Two sandboxes may run different loader versions, different mod versions, different configurations, or even different world states.

Consider a hypothetical combat scenario:

- A player attacks a mob in the Fabric sandbox.
- Fabric immediately predicts that the attack succeeds.
- NeoForge runs a mod that gives the mob a chance to activate an additional defensive ability.
- The authoritative result differs from Fabric's prediction.

If the system attempts to reconcile the two simulations afterward, the player may observe:

- Health decreasing and then increasing.
- Duplicate damage effects.
- Conflicting animations.
- Incorrect knockback.
- Different entity states between clients.

With many independent mods, these conflicts can become extremely difficult to resolve.

### 2.2 Proposed Solution: Authoritative Simulation and Visual Delegation

**Establish a single authoritative source for each synchronized gameplay domain.**

The project should not assume that both sandboxes can independently simulate the same gameplay event and reliably produce identical results.

The proposed architecture separates gameplay authority from visual representation.

**1. Authoritative Source**

One sandbox is responsible for calculating the final result of a particular synchronized operation.

For example:

- NeoForge may own combat calculations.
- Fabric may own a particular mod-specific interaction.
- Rust coordinates communication but does not independently calculate Minecraft gameplay.

Authority should be configurable by domain rather than permanently assigned to one loader.

**2. Request-Based Interaction**

Instead of immediately applying an authoritative gameplay change, the non-authoritative sandbox sends a request.

Example protocol:

```json
{
  "type": "attack_request",
  "request_id": 1024,
  "source": "fabric",
  "target_entity": 12345
}
```

The authoritative sandbox validates the request, performs the operation, and returns the result.

```json
{
  "type": "attack_result",
  "request_id": 1024,
  "accepted": true,
  "damage": 6.0
}
```

The receiving sandbox then updates its visual representation.

**3. Visual Prediction Without Authoritative Mutation**

The non-authoritative sandbox may play a swing animation immediately, but it should not independently apply authoritative health or inventory changes.

This reduces visible rollback and duplicate-operation problems.

**4. Lazy Confirmation**

For operations where a small delay is acceptable, wait for the authoritative result before committing the synchronized state.

This is preferable to implementing a complicated rollback system across two independently running mod environments.

However, not every interaction can tolerate this delay. The design must distinguish latency-sensitive visual feedback from authoritative gameplay changes.

---

## 3. Non-Standard Mod Behavior: The Compatibility Boundary Problem

### 3.1 The Core Problem

Java instrumentation and bytecode transformation provide powerful ways to inspect and modify runtime behavior.

However, not every mod uses standard Minecraft APIs.

Large technical mods, including projects such as Create and Mekanism, may introduce:

- Custom internal data structures.
- Specialized scheduling systems.
- Custom thread pools.
- Direct access to internal implementation details.
- Optimized block and entity lookup mechanisms.
- Native libraries loaded through JNI.

A `ClassFileTransformer` cannot automatically understand every custom implementation.

Furthermore, native code executes outside ordinary Java bytecode instrumentation.

Even if a transformer successfully intercepts a Minecraft class, that does not guarantee that every mod operation will pass through the intercepted method.

### 3.2 Proposed Solution: API-Level Integration and Proxy Fallbacks

**Do not depend entirely on automatic bytecode injection.**

Use multiple compatibility layers.

**1. Standard API Interception**

Where possible, intercept established interoperability boundaries.

Examples include:

- Forge / NeoForge Capabilities.
- Fabric Transfer API.
- Fabric API Lookup mechanisms.
- Standard block entity and entity interaction interfaces.
- Inventory and energy transfer interfaces.

These APIs provide useful integration points, but they do not cover every mod or every internal operation.

The implementation must support loader-specific adapters rather than assume that one API covers both environments.

**2. Explicit Compatibility Adapters**

For important mods, implement dedicated adapters.

An adapter should translate a mod's exposed state into a loader-independent representation.

For example:

```text
Create Adapter
      |
      v
Normalized Machine State
      |
      v
Rust DataBus
      |
      v
NeoForge / Fabric Receiver
```

This is more reliable than attempting to infer arbitrary internal mod behavior.

**3. Shadow Block and Proxy Entity Fallback**

For unsupported machines, synchronize only their visual representation.

The remote sandbox creates a shadow representation containing:

- Block position.
- Block state.
- Orientation.
- Selected visual properties.
- Optional animation state.

When a player interacts with the shadow object, the request is forwarded to the sandbox that owns the real machine.

The authoritative sandbox performs the operation and returns the result.

This creates a practical fallback for complex machines that cannot yet be fully synchronized.

**4. Native Library Compatibility**

Native libraries require a separate compatibility strategy.

Possible techniques include:

- JNI boundary instrumentation.
- Explicit native API wrappers.
- OS-level library interposition where supported.
- IPC adapters for cooperating native components.

`LD_PRELOAD` on Linux, `DYLD_INSERT_LIBRARIES` on macOS, and Windows API interception techniques can help with specific native interfaces.

However, they cannot guarantee interception of all native operations. Platform security restrictions, code signing, static linking, and direct system calls can limit their effectiveness.

Therefore, native interception should remain an optional advanced integration layer, not a core architectural dependency.

---

## 4. Cross-Platform Window Integration: The GPU Sharing Problem

### 4.1 The Core Problem

Running two Minecraft clients independently is relatively straightforward compared with combining their rendered output into a unified interface.

A possible approach is to share GPU resources or textures between processes.

However, graphics interoperability differs significantly across operating systems.

| Platform | Relevant graphics technologies |
|---|---|
| macOS | Metal, OpenGL compatibility, IOSurface |
| Windows | Direct3D 11/12, DXGI, shared resources |
| Linux | Vulkan, DMA-BUF, X11, Wayland |

Cross-process GPU resource sharing introduces several risks:

- Driver incompatibility.
- Synchronization errors.
- Resource lifetime problems.
- GPU memory leaks.
- Device resets and driver timeouts.
- Window manager restrictions.

A solution that works on one machine may fail on another GPU or operating system.

### 4.2 Proposed Solution: OS-Level Window Composition First

**Do not make GPU-level texture sharing a prerequisite for the initial usable version.**

Start with ordinary operating-system window composition.

**Current prototype status:** The Rust Window Manager and Java bridge register
Fabric, Forge, and NeoForge JVM processes; tests cover registration,
reconnection, rejection, and PID-based association. macOS window discovery and
Accessibility inspection are implemented, and Rust source now includes
transactional position/size changes. A real operation partially applied and
could not confirm rollback, so safe live recovery remains unresolved. Window
embedding/composition and Windows/Linux discovery are not implemented.

**1. Independent Client Windows**

Each sandbox keeps its own rendering context and window.

Rust manages their lifecycle and coordinates their startup and shutdown.

**2. Window Arrangement and Integration**

Investigate platform-native window APIs:

- macOS: Cocoa / AppKit.
- Windows: Win32.
- Linux: X11 and Wayland-compatible integration.

Potential presentation modes include:

- Side-by-side windows.
- Borderless windows.
- Parent-child window arrangements where supported.
- A unified launcher interface controlling both clients.

Not every window manager supports arbitrary cross-process child-window embedding. This must be tested independently on each platform.

**3. Native Compositor Integration**

Let the operating system handle ordinary window composition.

This avoids requiring the project to implement its own cross-process GPU compositor during the early development stages.

**4. Optional Advanced Rendering Integration**

After the basic window-management layer is stable, investigate shared textures or GPU composition as an optional optimization.

The project should retain a functional fallback when advanced graphics interoperability is unavailable.

---

## 5. Cross-Version State Synchronization: The Semantic Compatibility Problem

### 5.1 The Core Problem

The current prototype has demonstrated communication between separate Minecraft environments, including:

- Fabric 1.21.4.
- NeoForge 1.21.x.
- Independent JVM processes.
- Java Agent instrumentation.
- Rust TCP message routing.
- Runtime player discovery.
- Fabric-to-NeoForge player-state transfer.
- Remote player injection into NeoForge.

However, successfully transferring coordinates does not mean that two Minecraft versions share the same game state.

Different versions may have different:

- Entity implementations.
- Method names and mappings.
- Entity metadata.
- Attribute systems.
- Registries.
- Block states.
- Dimensions.
- Chunk data.
- Network protocol assumptions.

A coordinate such as `(3.0, 64.0, 8.0)` is easy to serialize. An arbitrary modded entity with custom attributes and behavior is considerably harder.

### 5.2 Proposed Solution: A Version-Independent State Protocol

**Never make Rust depend directly on Minecraft's internal Java classes.**

Instead, introduce a normalized protocol between the sandboxes.

For example:

```json
{
  "protocol_version": 1,
  "type": "player_state",
  "source": "fabric",
  "sequence": 100,
  "timestamp": 1730000000000,
  "dimension": "minecraft:overworld",
  "position": {
    "x": 3.0,
    "y": 64.0,
    "z": 8.0
  },
  "rotation": {
    "yaw": 90.0,
    "pitch": 0.0
  },
  "movement": {
    "sprinting": true
  }
}
```

The exact protocol will evolve.

Its key design requirements are:

**1. Versioned Protocol**

Every message should identify its protocol version.

This allows future agents to negotiate supported message formats.

**2. Loader-Specific Adapters**

Fabric and NeoForge agents should translate their respective internal game objects into the same normalized representation.

```text
Fabric LocalPlayer
       |
       v
Fabric Adapter
       |
       v
Normalized State Protocol
       |
       v
Rust DataBus
       |
       v
NeoForge Adapter
       |
       v
NeoForge Remote Entity
```

**3. Sequence Numbers and Timestamps**

Each sender should attach sequence numbers and timestamps.

The receiver can then reject outdated packets and identify missing updates.

**4. Coordinate and Dimension Validation**

The system must not assume that matching coordinates imply matching worlds.

Before attempting full world synchronization, it should validate:

- Dimension identifiers.
- World identity.
- Coordinate conventions.
- Entity availability.
- Chunk loading state.

**5. Graceful Capability Negotiation**

If a receiver does not support a particular state field, it should ignore that field or use a defined fallback instead of crashing.

---

## 6. DataBus Reliability: Message Ordering, Backpressure, and Recovery

### 6.1 The Core Problem

The current Rust DataBus is a TCP-based IPC prototype.

TCP provides reliable, ordered byte-stream delivery within a connection, but it does not automatically provide application-level guarantees.

Potential future problems include:

- A disconnected client missing important state updates.
- Old position updates being processed after newer ones.
- A slow receiver accumulating queued messages.
- Reconnection without a fresh state snapshot.
- Multiple message types competing for processing time.
- Unbounded memory growth under heavy traffic.

These problems become increasingly important when synchronization expands beyond player coordinates.

### 6.2 Proposed Solution: An Authority-Aware Messaging Layer

The DataBus should evolve from a simple JSON router into a protocol-aware
messaging service. Routing alone is insufficient: the bus must also enforce
which sandbox is allowed to publish each authoritative state category.

**1. Explicit Authority Model**

The initial player-state protocol has one authoritative source:

- Fabric is the authoritative publisher of local-player movement state.
- NeoForge is a receiver-only consumer of authoritative `player_move` messages.
- NeoForge may maintain a visual or gameplay-side mirror, but that mirror
  must never become a second source of authoritative player movement.
- Reverse-direction messages may be introduced for explicitly defined
  requests, diagnostics, capability reports, and acknowledgements.

This is an architectural rule, not merely a convention followed by the
Java Agents.

**2. Sender Identity and Message Authorization**

The Rust DataBus must derive the sender identity from the registered TCP
client connection. It must not trust a sender name supplied only inside
the JSON payload.

Before routing a message, the DataBus should validate:

- The registered sender identity.
- The message type.
- The sender's permission to publish that message type.
- The destination and broadcast policy.
- Required fields and value ranges.
- Protocol version and message size.

For the initial policy:

| Registered sender | Message type | Policy |
|---|---|---|
| Fabric | `player_move` | Allow |
| NeoForge | `player_move` | Reject |
| NeoForge | Explicit request or diagnostic | Allow only when defined |
| Fabric | Explicit response or diagnostic | Allow only when defined |

A rejected message must not be routed to any recipient. The rejection should
be observable through a structured error or diagnostic log.

The current Rust source implements this policy for `player_move`,
`monster_spawn`, `monster_update`, and `monster_remove`, returning a
`state_authority_violation` response to non-Fabric registered names before
routing. This is connection-scoped source filtering, not authenticated process
identity: the client supplies its registration name. Validate or authenticate
registration before relying on the name as a security boundary, and runtime-test
rejection, no-forwarding, and connection continuity.

**3. Message Classification**

Separate messages into categories:

| Message category | Example | Proposed handling |
|---|---|---|
| Authoritative state | Fabric `player_move` | Source-authorized, latest-state priority |
| Requests | Interaction request | Explicit request/response semantics |
| Discrete events | Spawn or remove event | Preserve event identity |
| Initialization | Entity registration | Reliable delivery |
| Heartbeat | Client health | Periodic monitoring |
| World metadata | Dimension information | Snapshot-based updates |
| Diagnostics | Runtime status | Non-authoritative reporting |

Message type and sender permission must be validated together. A message
category must not implicitly grant every sandbox permission to publish it.

**4. Ordering, Backpressure, and Stale-State Rejection**

Do not allow a slow receiver to accumulate an unlimited number of outdated
movement packets.

For high-frequency authoritative state:

- Attach a monotonically increasing sequence number per source session.
- Include a timestamp and session or connection generation.
- Reject updates older than the latest accepted state.
- Prefer the newest queued movement state when intermediate states are obsolete.
- Keep discrete events separate so they are not silently discarded.

Sequence numbers from a previous connection generation must not be treated
as newer than state from the current session.

**5. Reconnection and Resynchronization**

After reconnecting:

- Register the sandbox identity again.
- Establish a new connection generation.
- Exchange protocol versions and capabilities.
- Wait for a fresh authoritative Fabric state snapshot.
- Recreate or recover the NeoForge mirror.
- Resume incremental state updates only after the initial snapshot is accepted.

A reconnect must not cause NeoForge to publish its local player as a substitute
authority when Fabric is unavailable.

**6. Transport Evolution**

Keep TCP for the current prototype.

Only consider shared-memory IPC, memory-mapped queues, or other lower-latency
transports after profiling demonstrates that TCP is a meaningful bottleneck.

The current TCP JSON implementation is an IPC prototype, not a zero-copy
transport.

---

## 7. Development Strategy: What We Will Actually Implement

The architecture above describes long-term challenges. Implementation should
proceed incrementally, preserving the one-way authority model before expanding
the protocol.

### Phase 1 — Enforce and Stabilize the Current One-Way Prototype

**Priority: Highest**

- [x] Rust launches Fabric independently.
- [x] Rust launches NeoForge independently.
- [x] Rust launches both sandboxes concurrently.
- [x] Rust DataBus accepts named registrations.
- [x] Fabric-to-NeoForge messages are routed.
- [x] Fabric player coordinates are extracted through runtime reflection.
- [x] NeoForge discovers its Minecraft instance.
- [x] NeoForge discovers its `ClientLevel`.
- [x] NeoForge discovers its `LocalPlayer`.
- [x] NeoForge can receive Fabric state and create a `RemotePlayer` mirror
      in the prototype.
- [x] The NeoForge monitor no longer actively samples its local player for
      authoritative movement transmission in the edited Java source.
- [x] Rust DataBus source rejects non-Fabric registered clients publishing
      `player_move` or monster state messages.
- [x] Committed Java source parses NeoForge `monster_spawn` and
      `monster_update`, creates/updates mirrors, and clears mob mirrors on a
      detected world change.
- [x] Committed Java source adds sequence-based monster ACK/retry.
- [ ] Rebuild and package the Java Agent containing the receiver-only change.
- [ ] Validate or authenticate registration names; current names are supplied
      by the connecting client and are not authenticated process identities.
- [ ] Runtime-test that unauthorized state messages are rejected, never
      routed, and do not disconnect the sending client.
- [ ] Verify that Fabric `player_move` continues to reach NeoForge using the
      packaged Agent.
- [ ] Runtime-test monster mirror creation/update, ACK/retry ordering, and
      world-change behavior using the packaged Agent.
- [ ] Define and implement reliable monster removal publication.
- [ ] Verify reconnect and fresh-state recovery.
- [ ] Verify that both clients remain responsive during synchronization.

The Rust source now enforces the one-way state policy at the DataBus boundary.
Committed Java Agent source extends the prototype with hostile-mob
receive/mirroring and ACK/retry, but the packaged Agent and live behavior are
not yet runtime-verified. The immediate task is to rebuild the Java Agent,
constrain the self-asserted registration identity, and verify rejection and
normal delivery at runtime. `monster_remove` is not implemented, so hostile-mob
lifecycle synchronization remains incomplete.

### Phase 2 — Establish a Versioned Synchronization Protocol

- [ ] Introduce protocol versioning.
- [ ] Add sequence numbers and timestamps.
- [ ] Add connection or session generations.
- [ ] Normalize player state.
- [ ] Add dimension and world identity information.
- [ ] Implement stale-packet rejection.
- [ ] Add heartbeat and timeout handling.
- [ ] Implement reconnect and full-state resynchronization.
- [ ] Separate continuous state updates from discrete events.
- [ ] Define explicit permissions for each message type.

### Phase 3 — Stabilize Remote Player Lifecycle

- [ ] Establish stable remote-player identity.
- [ ] Prevent entity ID collisions.
- [ ] Handle spawn and despawn events.
- [ ] Handle dimension and world transitions.
- [ ] Make removal failures retryable.
- [ ] Prevent duplicate mirrors after reconnection.
- [ ] Add interpolation and smoothing.
- [ ] Handle missing chunks and unloaded entity regions.
- [ ] Test repeated world unload and reload cycles.

### Phase 4 — Expand Beyond Player Coordinates

- [ ] Entity discovery and entity identity mapping.
- [ ] Entity spawn and despawn events.
- [ ] Rotation, velocity, and movement synchronization.
- [ ] Health and selected attributes.
- [ ] Basic inventory representation.
- [ ] Block state synchronization.
- [ ] Dimension and world compatibility checks.
- [ ] Mod-specific compatibility adapters.

Each feature must define its authoritative source, permitted message directions,
consistency requirements, and failure behavior before implementation.

### Phase 5 — Improve Compatibility and Portability

- [x] Add process registration, PID-based window association, macOS window
      discovery/Accessibility inspection, and source-level position/size
      transactions.
- [ ] Validate discovery and macOS permission handling with live Minecraft
      windows.
- [ ] Review identity checks and diagnostics; resolve and live-validate the
      partial-mutation/failed-rollback path before relying on window control.
- [ ] Define and implement window arrangement/embedding only after selecting
      the intended user experience.
- [ ] Establish loader-specific adapter interfaces.
- [ ] Implement capability-based synchronization.
- [ ] Add shadow representations for unsupported blocks and entities.
- [ ] Introduce message prioritization and backpressure.
- [ ] Profile CPU, memory, and network overhead.
- [ ] Test behavior under GC pauses and temporary disconnections.
- [ ] Build automated compatibility tests for supported Minecraft versions.
- [ ] Verify Windows and Linux support.
- [ ] Improve automated Java and Minecraft environment detection.

## 8. Architectural Principles

The following principles will guide future engineering decisions.

1. **Isolation before integration.** Keep loader internals separate wherever possible.
2. **Asynchronous communication before lockstep.** One sandbox must not control the tick rate of another.
3. **Explicit authority before reconciliation.** Fabric is the sole authoritative publisher of player movement state; NeoForge consumes that state as a receiver. Reverse-direction requests must not become a second authoritative state stream.
4. **Normalized data before internal object sharing.** Minecraft classes must not become the cross-loader protocol.
5. **Adapters before universal bytecode manipulation.** Prefer stable integration boundaries when available.
6. **Graceful degradation before hard failure.** Unsupported features should fall back to visual proxies or disabled synchronization.
7. **Measured optimization before complexity.** Do not introduce shared-memory or GPU-level mechanisms without profiling evidence.
8. **Incremental verification before feature expansion.** Every synchronization layer must have independently reproducible tests.

---

## 9. Final Direction

JVM Mod Combiner is not intended to make Fabric and Forge/NeoForge internally
identical.

The goal is to allow independently running mod-loader environments to exchange
selected game state and interact through explicitly defined synchronization
interfaces.

The initial player synchronization architecture is intentionally asymmetric:

- Fabric is the sole authoritative source of player movement state.
- NeoForge receives Fabric state and maintains a remote-player mirror.
- Rust coordinates communication and enforces sender authorization.
- Loader-specific Java Agents translate local runtime data into normalized
  protocol messages.
- Reverse-direction messages may carry explicitly defined requests,
  acknowledgements, diagnostics, or capability reports.
- Reverse-direction messages must not create a second authoritative
  `player_move` stream.

The authority policy must be enforced at multiple layers:

1. The NeoForge Java Agent must not actively sample and publish its local
   player as authoritative movement state.
2. The Rust DataBus must reject unauthorized `player_move` messages based on
   the registered connection identity.
3. The receiver must validate protocol version, session generation, sequence
   number, and state freshness before applying updates.

A Java Agent convention alone is not a security or correctness boundary.
The DataBus must enforce the protocol even if a client sends an invalid
message.

A fully compatible cross-loader Minecraft environment is a much larger
objective than launching two clients or mirroring player coordinates.

The practical path is to prove each layer separately: enforce one-way player
authority, stabilize mirror lifecycle and recovery, then expand toward entities,
blocks, inventories, and selected modded systems.

**The immediate objective is a stable, measurable, Fabric-authoritative
player-state pipeline with Rust-enforced message permissions—not bidirectional
player-state synchronization.**
