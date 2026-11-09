# JVM Mod Combiner

> **由于即将参加考试，本项目将暂停开发数周。**
>
> 考试结束后将继续推进 JVM Mod Combiner 的开发与实验。

> 一个使用 Rust 和 Java 开发的跨 Loader 互操作研究项目。目标是通过隔离的运行环境和跨沙盒通信，让 Fabric 与 Forge/NeoForge 能够协同运行，而不依赖 Sinytra Connector。

[English README](README.md) · [技术问题清单](problems.md) · [GitHub Discussions](https://github.com/hchunhei81-cyber/jvm-mod-fusion/discussions)

---

## 项目背景

Minecraft 的模组生态被多个 Mod Loader 分割。

Fabric 与 Forge/NeoForge 在以下方面存在差异：

- Loader 初始化流程。
- ClassLoader 和 Classpath 结构。
- Mixin 集成机制。
- 事件系统与生命周期管理。
- 依赖解析机制。

现有跨 Loader 兼容方案，例如 [Sinytra Connector](https://github.com/Sinytra/Connector)，主要通过兼容层和转换机制，让不同生态中的模组能够协同工作。

JVM Mod Combiner 探索另一条技术路线：

**不直接把一个 Loader 的内部运行环境转换成另一个 Loader 的环境，而是尝试隔离各自的运行环境，并通过数据接口进行通信。**

长期目标是降低不同 Loader 之间的直接依赖，使跨 Loader 互操作不必深度绑定某一个 Loader 的内部实现。

这目前仍然是一个实验性研究方向，并不是成熟的模组兼容替代方案。

## 架构设计

### 目标架构

```text
                    Rust Microkernel
              (JVM 生命周期 / 沙盒编排)
                            |
                   跨沙盒数据接口
                            |
               +------------+------------+
               |                         |
         +-----v------+            +-----v------+
         | Sandbox A  |            | Sandbox B  |
         | Fabric     |            | Forge /    |
         | Loader     |            | NeoForge   |
         | + Mods     |            | + Mods     |
         +------------+            +------------+
```

项目目前围绕三个设计原则展开。

### 1. Microkernel（微内核）

Rust 负责：

- JVM 生命周期管理。
- Loader 沙盒编排。
- 依赖准备与 Classpath 构建。
- 跨沙盒通信基础设施。

尽可能避免让 Rust 内核直接依赖 Fabric 或 Forge/NeoForge 的内部实现。

### 2. Runtime Isolation（运行时隔离）

不同 Loader 应当拥有独立的类加载边界。

目前已经验证：

- 两个独立的 `URLClassLoader` 可以加载同名类。
- 两个 ClassLoader 加载的类可以表现出不同的行为。

但这不代表已经实现了两个真实 Minecraft Loader 在同一个 JVM 中完全隔离运行。

目前真实 Loader 的双端并行启动采用**独立 JVM 进程**。

### 3. Data-layer Interoperability（数据层互操作）

不同沙盒之间不应直接依赖对方的 Loader 内部对象，而应通过结构化数据交换游戏状态。

目前使用 Rust 实现基于 TCP 的 DataBus。

共享内存与真正的 Zero-copy 数据传输仍然属于未来目标，并未完成。

---

## 项目进度

<badge color="warning">早期原型 · 尚未达到统一可玩状态</badge>

### 已验证的功能

主要测试环境为 macOS + Apple Silicon + Temurin JDK 21。

- [x] Rust 通过 JNI 启动嵌入式 JVM。
- [x] Rust 调用 Java 静态方法并获取返回值。
- [x] Rust 通过 `URLClassLoader` 动态加载外部 JAR。
- [x] 两个独立 `URLClassLoader` 加载同名类，并产生不同的行为。
- [x] Rust JNI 成功启动 Minecraft 1.21.4 + Fabric Loader，主菜单、纹理和音频系统正常初始化。
- [x] Rust 通过独立 JVM 进程启动 Minecraft 1.21.1 + NeoForge 21.1.191。
- [x] Rust 成功启动 Forge 1.21.4（54.1.22）并进入早期加载界面。
- [x] 根据本地 Minecraft 安装信息动态组装依赖与 Classpath。
- [x] 在当前测试配置下，通过 `fabric.gameJarPath` 让 Fabric 正确识别 Minecraft Game JAR。
- [x] Fabric 1.21.4 与 NeoForge 1.21.1 可以在同一台机器上通过独立 JVM 进程同时运行。
- [x] Rust TCP DataBus 已经实现。
- [x] Fabric 与 NeoForge Java Agent 可以使用不同身份向 DataBus 注册。
- [x] 已经验证 `hello` / `ack` JSON 消息的跨沙盒路由。
- [x] Rust 可以通过 JVM 启动参数注入 Java Agent。
- [x] Java Agent 的 `ClassFileTransformer` 成功观察到由 Fabric `KnotClassLoader` 加载的类。
- [x] Fabric 可以通过反射获取本地玩家的坐标与移动状态。
- [x] Fabric 玩家状态消息已经通过 Rust DataBus 发送到 NeoForge。
- [x] NeoForge Java Agent 可以找到 `Minecraft` 单例、`ClientLevel` 和 `LocalPlayer`。
- [x] 已经实现 NeoForge `RemotePlayer` 镜像注入的 PoC 路径。

### 正在开发

- [ ] NeoForge 本地玩家状态发送器。
- [ ] Fabric 接收端及远程玩家镜像实现。
- [ ] 双向玩家状态同步。
- [ ] 更完善的玩家身份和实体生命周期管理。
- [ ] 不同 Minecraft 版本的反射与映射解析。
- [ ] 网络延迟、断线和状态一致性处理。

### 尚未实现

- [ ] 真正的 Zero-copy / Shared-memory DataBus。
- [ ] 在同一个 JVM 中完整隔离运行真实 Fabric 与 Forge/NeoForge Loader。
- [ ] 通用模组互操作。
- [ ] 自动 Mixin 冲突解决。
- [ ] 完整的跨 Loader 事件转换。
- [ ] Windows 和 Linux 的生产级兼容性验证。

**注意：** 当前的玩家同步只是概念验证。它并不代表已经实现完整的模组兼容，也不代表不同 Minecraft 版本之间拥有完全一致的游戏行为。

---

## PoC 与验证截图

以下截图分别展示项目不同阶段的实验结果，并不代表整个项目已经达到生产可用状态。

### 1. Fabric Loader 启动 — Minecraft 1.21.4

Rust 通过 JNI 启动 Fabric，成功渲染 Minecraft 主菜单。

![Rust JNI 启动 Minecraft 1.21.4 + Fabric](image.png)

### 2. NeoForge 启动 — Minecraft 1.21.1

Rust 通过独立 JVM 进程启动 NeoForge。

![Rust 启动 NeoForge](image2.png)

### 3. 双沙盒并行启动

Fabric 1.21.4 与 NeoForge 1.21.1 在同一台机器上通过不同 JVM 进程并行运行。

![Dual Sandbox Launch Proof](image3.png)

### 4. DataBus 路由 — IPC 原型

Rust 微内核接收两个沙盒的注册请求，并通过 TCP 路由层转发消息。

![Cross-Sandbox IPC Routing](image4.png)

### 5. Runtime Bytecode Instrumentation

Rust 在 JVM 启动参数中注入 Java Agent。

Java Agent 使用 `ClassFileTransformer` 观察 Fabric `KnotClassLoader` 加载的类。

示例日志：

```text
[BusAgent:fabric] premain
[BusAgent:fabric] MC-CLIENT: net/minecraft/client/main/Main loader=KnotClassLoader
[BusAgent:fabric] MC-CLIENT: net/minecraft/client/ClientBrandRetriever loader=KnotClassLoader
```

### 6. Fabric 玩家状态提取

Fabric 沙盒通过反射获取本地玩家的实时状态，包括坐标和旋转信息。

![Fabric 玩家状态提取](image5.png)

### 7. Fabric → NeoForge 状态同步

Fabric 通过 Rust TCP DataBus 发送玩家状态。

NeoForge 成功接收并解析同步消息。

![NeoForge 接收玩家状态](image6.png)

### 8. 跨沙盒实体镜像

通过反射在 NeoForge 的 `ClientLevel` 中注入 PoC `RemotePlayer`。

镜像实体可以根据 Fabric 发送的状态进行更新。

![Cross-Sandbox Entity Mirroring](image7.png)

> 不同 Minecraft 版本和世界种子可能产生不同的地形。玩家坐标一致并不代表世界生成结果相同。

### 9. Forge 1.21.4 启动

Rust 成功启动 Forge 1.21.4（54.1.22），并进入早期加载界面。

![Rust 启动 Forge 1.21.4](image8.png)

---

## 快速开始

### 环境要求

- Rust stable 工具链（1.75+）。
- 推荐 JDK 21。
- 本地 Minecraft 安装及对应 Loader 文件。
- 当前主要验证环境为 macOS，Windows 和 Linux 尚需进一步测试。

### 克隆仓库

```bash
git clone https://github.com/hchunhei81-cyber/jvm-mod-fusion.git
cd jvm-mod-fusion
```

### 配置 Java

macOS + Temurin 21 示例：

```bash
export JAVA_HOME=$(/usr/libexec/java_home -v 21)
```

### 编译与运行

```bash
cargo build
cargo run
```

默认模式运行基础 JVM 和 ClassLoader 隔离测试。

预期输出：

```text
=== Rust JVM Fusion Kernel: Sandbox Isolation Test ===
[Sandbox A] I am from Sandbox A!
[Sandbox B] I am from Sandbox B!
[OK] If you see A and B above, sandbox isolation is working!
```

---

## 运行模式

所有模式目前都处于实验阶段，可能出现崩溃或版本兼容问题。

### Fabric

```bash
cargo run -- --mode fabric
```

启动配置好的 Fabric 环境。

### Forge

```bash
cargo run -- --mode forge
```

启动配置好的 Forge 环境。

### NeoForge

```bash
cargo run -- --mode neoforge
```

启动配置好的 NeoForge 环境。

### Fusion

```bash
cargo run -- --mode fusion
```

尝试并行启动 Fabric 与 NeoForge，并通过 Rust DataBus 建立跨沙盒通信。

> Fusion 模式目前不稳定。两个真实 Minecraft 环境使用独立 JVM 进程运行，尚未成为一个统一的可玩 Minecraft 客户端。

### DataBus 测试

```bash
cargo run -- --mode bus-test
```

启动独立 TCP 路由服务：

```text
127.0.0.1:25565
```

---

## 已知问题

当前技术问题与调查记录请查看 [Problems.md](problems.md)。

主要挑战包括：

- 真实 Loader 的 ClassLoader 隔离。
- JPMS 与 ModLauncher 启动要求。
- Mixin 兼容性和字节码转换顺序。
- Minecraft 版本映射差异。
- 玩家身份和实体生命周期同步。
- 网络重连与状态一致性。

---

## 为什么不使用 Sinytra Connector？

[Sinytra Connector](https://github.com/Sinytra/Connector) 是一个成熟的兼容性项目，目标是让 Fabric 模组能够在 Forge/NeoForge 环境中运行。

JVM Mod Combiner 探索不同的架构：

不直接转换一个 Loader 的内部环境，而是尝试隔离不同 Loader 的运行环境，并通过受控的数据接口连接它们。

这种方案可能带来的优势：

- 减少对 Loader 内部实现的直接依赖。
- 独立管理不同 Loader 的生命周期。
- 建立明确的通信边界。
- 更灵活地实现游戏状态同步。

但隔离也带来了新的挑战。跨 Loader 的实体同步、事件语义、世界状态、渲染和模组行为，远远比传输玩家坐标复杂。

本项目研究的是这些问题，而不是宣称已经解决它们。

---

## 开发路线图

### Phase 1 — Runtime Foundation

- [x] Rust / JNI 集成。
- [x] 基础 `URLClassLoader` 隔离。
- [x] Fabric 启动。
- [x] Forge / NeoForge 启动实验。

### Phase 2 — Sandbox Communication

- [x] Rust TCP DataBus。
- [x] 客户端身份注册。
- [x] JSON 消息路由。
- [x] Java Agent 集成。

### Phase 3 — Game-state Synchronization

- [x] Fabric 玩家状态提取。
- [x] Fabric → NeoForge 消息传递。
- [x] NeoForge 玩家发现。
- [x] NeoForge 远程玩家注入原型。
- [ ] NeoForge 玩家状态发送器。
- [ ] Fabric 接收端与镜像注入。
- [ ] 双向状态同步。

### Phase 4 — Interoperability

- [ ] 玩家身份与生命周期协议。
- [ ] 实体同步。
- [ ] 物品栏与交互同步。
- [ ] 事件与能力系统桥接。
- [ ] Mixin 冲突解决策略。

### Phase 5 — Architecture and Portability

- [ ] Shared-memory / Zero-copy DataBus。
- [ ] 同一 JVM 中的真实 Loader 隔离。
- [ ] 动态依赖解析。
- [ ] Windows / Linux 验证。
- [ ] 性能和稳定性测试。

---

## 参与贡献

目前这是一个个人研究项目。

欢迎提交 Bug、测试结果和技术讨论。

如果你正在测试本项目，欢迎在 [GitHub Discussions](https://github.com/hchunhei81-cyber/jvm-mod-fusion/discussions) 的 **Showcase / Test Results** 分类发布测试结果。

建议提供：

- 操作系统和 CPU 架构。
- JDK 发行版与版本。
- Minecraft 版本。
- Loader 名称及版本。
- 相关模组列表。
- 运行日志和截图。
- 可复现问题的操作步骤。

尤其欢迎 Windows 和 Linux 用户提供测试结果，以帮助后续跨平台开发。

## License

BSD 3-Clause。详见 [LICENSE](LICENSE)。

