# JVM Mod Combiner

> 一个用 Rust 写的微内核，试图让 Fabric 与 Forge/NeoForge 模组
> **在同一个 JVM 里共存**，不依赖 Sinytra Connector。

[English README](README.md)

## 为什么要做这个

Minecraft 的模组生态是割裂的：**Fabric** 与 **Forge/NeoForge** 拥有互不兼容的
加载器、Mixin 约定和事件系统。现有的跨加载器方案（尤其是 Sinytra Connector）
通过在字节码层面把一个 ABI 翻译成另一个来让两者共存。这种做法天生脆弱：任何
一方更新，翻译层都可能崩溃。

这个项目走另一条路：**不做翻译。把两个加载器各自隔离在独立的沙盒里，只让它
们在数据层交汇。**

## 架构

Rust 微内核
（JVM 生命周期、沙盒编排、数据总线）
|
+----------+----------+
| |
+-----v-----+ +-----v-----+
| 沙盒 A | | 沙盒 B |
| Fabric | | Forge |
| Loader | | Loader |
| + Fabric | | + Forge |
| 模组 | | 模组 |
+-----------+ +-----------+

三个设计支柱：

1. **微内核。** 融合层是唯一持有 JVM 的组件。它不依赖 Fabric，也不依赖 Forge。
2. **运行时沙盒隔离。** 每个加载器运行在自己独立的 `URLClassLoader` 里，模组
   类之间不共享父加载器。同名类、不同加载器、互不冲突。
3. **零拷贝数据总线（规划中）。** 跨沙盒通信通过 Rust 持有的内存总线完成。
   模组代码层完全不见面，只在最终的“数据层”交汇。

## 项目状态

**早期原型，还不能用来玩 Minecraft。**

已在 macOS + Temurin 21（arm64）上验证可用的部分：

- [x] Rust 进程可以通过 JNI 启动内嵌 JVM
- [x] Rust 可以调用 Java 静态方法并读回返回值
- [x] Rust 可以通过 `URLClassLoader` 动态加载外部 `.jar`
- [x] 两个隔离的 `URLClassLoader` 实例可以加载同名类并产生不同行为
- [x] **Rust JNI 成功启动 Minecraft 1.21.4 + Fabric Loader —— 主菜单成功渲染，
      音效引擎初始化，纹理加载完成，游戏以正常帧率运行**
- [x] **Rust 成功通过子进程启动 Minecraft 1.21.1 + NeoForge 21.1.191 —— 主菜单成功渲染，绕过了官方启动器限制与 ModLauncher 的 JPMS 模块化冲突**
- [x] 从 `mc_libraries/` 动态组装类路径（253 个条目），通过
      `fabric.gameJarPath` 和关闭类路径隔离让 Fabric 认出游戏
- [x] **双沙盒并行启动成功**：在同一台机器上通过 Rust 子进程同时拉起 Fabric 1.21.4 与 NeoForge 1.21.1，互不干扰，均成功渲染主菜单。
- [x] **零拷贝数据总线（IPC 原型）**：在 Rust 微内核中实现了基于 TCP 的消息总线，成功接收来自 Fabric 与 NeoForge 两个隔离沙盒的跨进程通讯。
- [x] **跨沙盒消息路由**：Rust 总线按沙盒名路由 JSON 消息，Fabric 与 NeoForge 完成 `hello → ack` 双向闭环，并返回 `routed` 回执。
- [x] **运行时字节码拦截（Instrumentation）**：Rust 通过 JNI `InitArgsBuilder` 动态注入 Java Agent。`ClassFileTransformer` 成功拦截了由 Fabric `KnotClassLoader` 加载的类（如 `net.minecraft.client.main.Main`），证明了系统具备无需依赖 Mixin 即可进行自动化打补丁的底层能力。
- [x] **跨沙盒运行时状态同步（Proof of Concept）**：Fabric 沙盒通过反射绕过混淆提取玩家实时坐标，通过 Rust TCP 总线广播。NeoForge 沙盒成功接收并打印同步状态。这证明了跨物理隔离 JVM 进行实时数据层交汇的可行性。

进行中：

- [ ] 把真实的 Forge 1.21.4 加载器装进沙盒 B（注：Forge 1.16.5 因远古 Installer 的 zlib 哈希死锁，目前暂时卡死）
- [ ] 解决真实加载器的 `parent = null` 问题（它们需要 `java.lang.*` 等）
- [ ] 处理两个沙盒之间的 Mixin 冲突
- [ ] 零拷贝数据总线
- [ ] 动态依赖解析（解析 version manifest，而不是全量复制 `libraries`）
- [ ] 跨平台支持（Windows / Linux / macOS）

## 概念验证与实机演示

以下是微内核成功拉起双加载器并完成跨沙盒通信的视觉证明。

### 1. Fabric Loader 启动 (MC 1.21.4)
Rust 内嵌 JNI JVM 成功启动 Fabric，主菜单渲染完成，音效引擎与纹理加载正常。
![Rust JNI 启动 Minecraft 1.21.4 + Fabric](image.png)

### 2. NeoForge 启动 (MC 1.21.1)
Rust 通过子进程成功启动 NeoForge，绕过了官方启动器限制与 ModLauncher 的 JPMS 模块化冲突。
![Rust 启动 NeoForge](image2.png)

### 3. 双沙盒并行启动
Fabric 1.21.4 与 NeoForge 1.21.1 在同一台机器上通过独立子进程同时运行，互不干扰。
![双沙盒并行启动证明](image3.png)

### 4. 零拷贝数据总线路由 (IPC)
Rust 微内核成功接收来自两个沙盒的注册请求。日志显示 Fabric 与 NeoForge 之间完成了 `hello → ack` 的双向消息闭环路由，彻底摆脱了对共享 ClassLoader 的依赖。
![跨沙盒 IPC 路由](image4.png)

### 5. 运行时字节码拦截（Instrumentation）
通过 JNI 动态注入 Java Agent，利用 Instrumentation 成功拦截特定 ClassLoader 加载的字节码。这不需要依赖 Minecraft 的 Mixin，也不需要编译期依赖混淆的 Jar。
```text
[BusAgent:fabric] premain
[BusAgent:fabric] MC-CLIENT: net/minecraft/client/main/Main loader=KnotClassLoader
[BusAgent:fabric] MC-CLIENT: net/minecraft/client/ClientBrandRetriever loader=KnotClassLoader
```

### 6.跨沙盒运行时状态同步
Fabric 沙盒通过反射绕过混淆提取玩家实时坐标
![Fabric 沙盒通过反射绕过混淆提取玩家实时坐标](image5.png)


## 快速开始

### 依赖

- Rust（stable，1.75+）
- 一个 JDK。实测 Temurin 21；JDK 8+ 应该也行。

### 编译与运行

git clone https://github.com/hchunhei81-cyber/jvm-mod-fusion.git
cd jvm-mod-fusion

# 指向你的 JDK
export JAVA_HOME=/Library/Java/JavaVirtualMachines/temurin-21.jdk/Contents/Home

cargo run

如果一切正常，你会看到：

=== Rust JVM Fusion Kernel: Sandbox Isolation Test ===
[Sandbox A] I am from Sandbox A!
[Sandbox B] I am from Sandbox B!
[OK] If you see A and B above, sandbox isolation is working!

## 技术栈

Rust —— 内核、JVM 编排
jni crate（0.22.4） —— 与 JVM 的 FFI 桥
Java / JVM —— 加载器和模组真正住的地方

## 为什么不用 Sinytra Connector

Sinytra Connector 在字节码层面把 Fabric 模组翻译成 Forge 能运行的形式。它做得
相当不错，但是：

它天生脆弱：每次加载器更新都可能打破它。
它必须深入勾进两个加载器，所以总是落后于上游。
它不隔离两个加载器，它们共享同一个类空间。
本项目尝试一个完全不同的赌注：把两个加载器当作两个独立的世界，只在渲染/
数据层连接它们。

它可能会失败。单是 Mixin 冲突问题就已经够硬核。但如果它成功，从设计上就不会
因为加载器更新而崩溃。

## 已知难点

- [x] **通过实现自动化 JAR 净化、精确的 MANIFEST 属性注入、正确的 `jopt-simple` 与 `securejarhandler` 依赖路径，以及严格的 JVM 虚拟机参数排序（将 `-D` 参数置于主类前方），成功解决了 ModLauncher 的初始化与 `NullPointerException` (NPE) 崩溃问题。**
Forge 1.21+ 依赖 Java Module System（--module-path、BootstrapLauncher、
securejarhandler），必须按官方安装器的形态精确组装。
ModLauncher 10 要求 JAR manifest 元数据完整；任何重写 manifest 的行为都会
静默破坏版本发现。
两个加载器在同一个 JVM 内的 Mixin 冲突。
真实加载器的 ClassLoader 层级问题（parent = null 在实践中不可行）。

## 路线图

把真实的 Fabric Loader jar 加载进沙盒 A。
把真实的 Forge Loader jar 加载进沙盒 B（需要混合父加载器）。
处理 Mixin 冲突 —— 可能是 Rust 侧字节码合并，或者基于优先级的白名单。
零拷贝数据总线，用于跨沙盒通信。
跨平台（Windows / Linux / macOS）。

## 贡献

目前是个人研究项目。如果它能走过原型阶段，会补上贡献指南。

## 许可证

BSD 3-Clause，详见 [LICENCE](LICENCE)。
> **给使用者的一个小请求**：
> 如果你正在使用、测试或借鉴本项目，**强烈建议**你在 GitHub 的 [Discussions](https://github.com/hchunhei81-cyber/jvm-mod-fusion/discussions) 中的 **“Showcase / 运行反馈”** 分类下发一条帖子，并附上你的运行截图、操作系统和模组环境。
> 这不仅能给项目带来极大的鼓励，也能帮助我收集不同环境下的报错信息（尤其是 Windows 和 Linux 下的奇难杂症），以便更好、更快地进行修复与迭代。