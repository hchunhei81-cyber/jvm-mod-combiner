# JVM Mod Fusion

> 一个用 Rust 写的微内核，试图让 Fabric 与 Forge/NeoForge 模组**在同一个 JVM 里共存**，不依赖 Sinytra Connector。

[English README](README.md)

## 为什么要做这个

Minecraft 的模组生态是割裂的：**Fabric** 与 **Forge/NeoForge** 拥有互不兼容的加载器、Mixin 约定和事件系统。现有的方案（比如 Sinytra Connector）通过字节码层面的 ABI 翻译让两者共存，这很脆弱——只要任何一方更新，翻译层就可能崩溃。

这个项目走另一条路：**不做任何翻译。把两个加载器各自隔离在独立的沙盒里，只让它们在数据层交互。**

## 架构
┌──────────────────────────────┐
│ Rust 微内核（本项目） │
│ - JVM 生命周期管理 │
│ - 沙盒编排 │
│ - 零拷贝数据总线（规划中） │
└──────────────┬───────────────┘
│
┌─────────────────────┴─────────────────────┐
│ │
┌──────────▼──────────┐ ┌──────────▼──────────┐
│ 沙盒 A │ │ 沙盒 B │
│ Fabric Loader │ │ Forge Loader │
│ + Fabric 模组 │ │ + Forge 模组 │
└─────────────────────┘ └─────────────────────┘


三个设计支柱：

1. **微内核**：融合层是唯一持有 JVM 的组件。它不依赖 Fabric，也不依赖 Forge。
2. **运行时沙盒隔离**：每个加载器运行在自己独立的 `URLClassLoader` 里，模组类之间不共享父加载器。同名类、不同加载器、互不冲突。
3. **零拷贝数据总线（规划中）**：跨沙盒的通信通过 Rust 持有的内存总线完成。模组代码层完全不见面，只在最终的“数据层”交汇。

## 项目状态

**早期原型，还不能用来玩 Minecraft。**

已在 macOS + Temurin 25 上验证可用的部分：

- [x] Rust 进程可以通过 JNI 启动一个内嵌 JVM
- [x] Rust 可以调用 Java 静态方法并读回返回值
- [x] Rust 可以通过 `URLClassLoader` 动态加载外部 `.jar`
- [x] **两个隔离的 `URLClassLoader` 实例可以加载同名类（`SandboxTest`）并产生不同行为**

**还未**实现的部分：

- [ ] 把真实的 Fabric Loader jar 装进沙盒 A
- [ ] 把真实的 Forge/NeoForge Loader jar 装进沙盒 B
- [ ] 解决真实加载器的 `parent = null` 问题（它们需要 `java.lang.*` 等）
- [ ] 处理两个沙盒之间的 Mixin 冲突
- [ ] 零拷贝数据总线
- [ ] 跨平台支持（Windows / Linux / macOS）

## 快速开始

### 依赖

- Rust（stable，1.75+）
- 一个 JDK（实测 Temurin 25，JDK 8+ 应该也行）

### 编译与运行


git clone https://github.com/hchunhei81-cyber/jvm-mod-fusion.git
cd jvm-mod-fusion

# 指向你的 JDK
export JAVA_HOME=/Library/Java/JavaVirtualMachines/temurin-25.jdk/Contents/Home

cargo run

如果一切正常，你会看到：

=== Rust JVM Fusion Kernel: Sandbox Isolation Test ===
[Sandbox A] I am from Sandbox A!
[Sandbox B] I am from Sandbox B!
[OK] If you see A and B above, sandbox isolation is working!

## 技术栈
Rust — 内核、JVM 编排
jni crate (0.22.4) — 与 JVM 的 FFI 桥
Java / JVM — 加载器和模组真正住的地方

## 为什么不用 Sinytra Connector
Sinytra Connector 在字节码层面把 Fabric 模组翻译给 Forge。它做得相当不错，但是：

它天生脆弱——每次加载器更新都可能把它打破。
它必须深入勾进两个加载器，所以总是落后于上游。
它不隔离两个加载器，它们共享同一个类空间。
本项目尝试一个完全不同的赌注：把两个加载器当作两个独立的世界，只在渲染/数据层连接它们。

它可能会失败。单是 Mixin 冲突问题就已经够硬核了。但如果它成功，从设计上就不会因为加载器更新而崩溃。

## 路线图

把真实的 Fabric Loader jar 加载进沙盒 A
把真实的 Forge Loader jar 加载进沙盒 B（需要混合父加载器）
处理 Mixin 冲突 — 可能是 Rust 侧字节码合并，或者基于优先级的白名单
零拷贝数据总线，用于跨沙盒通信
跨平台（Windows / Linux / macOS）

## 贡献
目前是个人研究项目。如果它能走过原型阶段，会补上贡献指南。

## 许可证
BSD 3-Clause，详见 [LICENCE](LICENCE)。