# JVM Mod Combiner: 踩坑与解决记录 (PROBLEM.md)

本文档记录了在使用 Rust 微内核启动不同版本的 Minecraft 加载器时，所遇到的核心技术难点、报错原因以及最终的解决/绕过方案。

---

## 一、Fabric 1.21.4 启动问题 (Mod Loader 早期)

### 1. 缺 ASM 与 sponge-mixin 依赖
*   **现象**：启动 `KnotClient` 时抛出 `ClassNotFoundException`。
*   **原因**：Rust 侧手动构建的 Classpath 缺少 Fabric 依赖的底层字节码操作库。
*   **解决**：手动补全 `asm` / `asm-tree` / `asm-analysis` / `asm-commons` / `asm-util`（当时用的是 9.9.1），并从本地 Minecraft `libraries` 复制 `sponge-mixin.jar` 进入沙盒。

### 2. 报错 "No game providers present on the class path"
*   **现象**：Fabric 初始化失败，找不到游戏主类。
*   **原因**：没有告诉 Fabric 游戏所在的 JAR 在哪，且 Fabric 的类路径隔离机制挡住了游戏主类。
*   **解决**：在启动参数（或系统属性）中强行注入：
    *   `-Dfabric.gameJarPath=/path/to/minecraft-client.jar`
    *   `-Dfabric.debug.disableClassPathIsolation=true`
    *   `-Dfabric.gameVersion=1.21.4`

### 3. Classpath 全量复制导致版本冲突
*   **现象**：`NoSuchMethodError` 或 `ClassNotFoundException`，比如 Authlib、DataFixerUpper 或 LWJGL 版本不匹配。
*   **原因**：为了省事，直接把整个 `libraries` 目录塞进了 Classpath，导致不同版本的库互相覆盖。
*   **解决**：编写 `resolve_classpath()` 逻辑，直接解析 `1.21.4.json`，根据当前系统架构（macOS / arm64）的 rules 进行筛选，精准定位所需 JAR。

### 4. LWJGL natives 缺失
*   **现象**：报错 `Failed to locate library: liblwjgl.dylib` 或 `MemoryUtil` 初始化失败。
*   **原因**：natives 包没有被正确解压或找不到。
*   **解决**：单独下载并解压 LWJGL 3.3.3 的 macOS-arm64 natives 到指定目录，并通过 `-Djava.library.path` 显式指定。

---

## 二、NeoForge 1.21.1 启动问题 (ModLauncher 11)

### 1. `ModLauncher null starting` 与 NullPointerException
*   **现象**：BootstrapLauncher 启动时，`TypesafeMap.putIfAbsent` 抛出 NPE。
*   **原因**：ModLauncher 作为 JPMS 模块加载时，Manifest 中的版本号被忽略，`getImplementationVersion()` 返回 null。
*   **解决**：放弃修改原始 JAR 的 Manifest（因为模块化加载根本不吃这套），转为手动构建正确的 `--module-path` 和 `-DlegacyClassPath`，确保 BootstrapLauncher 正常识别 ModLauncher。

### 2. `ServiceConfigurationError`: Unable to get public no-arg constructor
*   **现象**：早期窗口 (`EarlyProgressVisualization`) 崩溃。
*   **解决**：在游戏目录 `config/fml.toml` 中关闭早期窗口：`earlyWindowControl = false`。

### 3. `NoSuchElementException` 与 `Missing classes`
*   **现象**：子进程启动时直接崩溃，或者报缺失基础类。
*   **原因**：服务端模板 `unix_args.txt` 不包含客户端渲染所需的库（如 ICU4J、text2speech 等），且 Java 9+ 的模块导出限制导致反射失败。
*   **解决**：
    *   利用 Python 脚本扫描并将 `ICU4J`、`text2speech`、LWJGL 强行追加到 `legacyClassPath` 中。
    *   添加启动参数 `--add-opens` 和 `--add-exports`，解放 `java.base` 下的相关模块。
    *   加入 `-XstartOnFirstThread` 解决 macOS 主线程创建 GLFW 窗口的限制。

---

## 三、Forge 1.16.5 启动问题 (ModLauncher 8)

*(注：此版本最终因底层死锁未能跑通，以下为尝试过程与原因分析)*

### 1. 官方 Installer 的 `jarsplitter` 哈希死锁
*   **现象**：Installer 运行到 `jarsplitter` 时报 `Expected: xxx` / `Actual: xxx` 哈希不匹配，导致安装失败。
*   **原因**：macOS 的 zlib 实现（zlib-ng）与 Forge 早期 Installer 依赖的经典 zlib 产生的压缩输出有差异，导致哈希校验永远无法通过。
*   **尝试解决**：换用 Java 8u312（无效）；换用 Forge 36.2.42（无效）。
*   **最终绕过**：在 Windows 虚拟机中运行 Installer 成功生成了 `srg.jar`、`extra.jar` 等文件，并将其拷贝到 Mac 使用。

### 2. 缺失 `fmlloader` 与 `FMLClientLaunchHandler`
*   **现象**：`Cannot find launch target fmlclient`。
*   **原因**：Forge 1.16.5 的启动类被官方藏得很深。它没有作为独立的 Maven 依赖发布，导致全网寻找 `fmlloader-1.16.5-36.2.42.jar` 均返回 404。同时，`universal.jar` 和 `client.jar` 中也没有这个类。
*   **尝试解决**：用 Prism 隐藏目录里的 1.17+ 版本“偷天换日”。
*   **最终失败原因**：高版本 ModLauncher 接口无法识别 1.17 的 FML 服务，抛出 `Invalid Services found fml`。

---

## 四、跨平台杂项与底层工具类问题

### 1. JNI 0.22 巨变
*   **现象**：大量 API 编译报错。
*   **解决**：使用 `InitArgsBuilder` + `JNIVersion::V1_8` 启动 JVM；必须开启 `invocation` feature；字符串和签名必须使用 `jni_str!` 和 `jni_sig!` 宏；`JObjectArray` 的构造参数变更需要传入 `JObject::null()`。

### 2. Java 版本兼容与模块化限制
*   **现象**：高版本 Java（17/21）报 `InaccessibleObjectException`（如 `sun.security.util`、`java.util.jar`）。
*   **解决**：在启动脚本中动态注入大量的 `--add-opens` 和 `--add-exports`。例如：
    `--add-exports java.base/sun.security.util=ALL-UNNAMED`
    `--add-opens java.base/java.util.jar=ALL-UNNAMED`

### 3. 全量扫描导致的高版本类污染
*   **现象**：报 `UnsupportedClassVersionError`（class file version 60.0 / 61.0 / 69.0）。
*   **原因**：本地 `libraries` 目录里有多个版本的加载器（如 ModLauncher 10、11，NeoForge 的 FML 等），Rust 或脚本在扫描时误把 Java 16+ 编译的类加载进了 Java 8 的进程。
*   **解决**：写了强力的 Python 脚本，读取 JAR 包内的 class 文件魔数，过滤掉 major version > 52 的所有文件。

---

## 五、跨版本玩家镜像与坐标同步问题（Fabric 1.21.4 / NeoForge 1.21.1）

### 1. NeoForge 未能及时获取 Minecraft 实例

- **现象**：DataBus 已收到 Fabric 发送的 `player_move` 消息，但处理坐标时，`nfMcInstance` 或 `nfMcClass` 仍为 `null`。
- **原因**：Minecraft 实例初始化与坐标消息处理之间存在时序问题；原有代码只延迟 3 秒获取实例，且没有验证 `getInstance()` 的返回值是否为 `null`。
- **解决措施**：增加实例状态诊断日志；将实例获取改为最多重试 180 次、每次间隔 1 秒，并仅在获取到非空实例后报告成功。

### 2. Fabric 与 NeoForge 的镜像玩家位置及世界环境不一致

- **现象**：Fabric 端正常显示玩家及森林环境，NeoForge 端也成功显示 `FabricMirror` 镜像玩家，但镜像玩家出现在地下封闭空间。
- **原因分析**：两个客户端使用不同的 Minecraft 版本（1.21.4 与 1.21.1），可能存在世界状态、区块加载或维度差异；具体原因尚未确认。
- **解决措施**：确认 DataBus 能持续转发 Fabric 的真实坐标，并在 NeoForge 中显示镜像实体。后续需要对比两端玩家的实际坐标、所在维度及区块加载状态，再修复位置与世界环境不一致的问题。

### 3. Minecraft 窗口失去焦点后自动进入 ESC 菜单

- **现象**：切换到浏览器或其他应用时，Fabric 与 NeoForge 客户端都会自动进入 ESC 暂停菜单，影响双客户端同时运行。
- **排查与处理**：检查 Minecraft 的窗口失焦自动暂停行为，尝试通过 `F3 + P` 切换失焦自动暂停设置，并检查客户端各自的 `options.txt` 配置。
- **后续验证**：确认两个客户端在切换应用后仍能保持运行，并检查 DataBus 坐标同步是否持续进行。