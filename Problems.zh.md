# JVM Mod Combiner — 问题与解决方案

> 本文记录了 JVM Mod Combiner 开发过程中遇到的主要技术问题，包括其表现、根本原因、尝试过的解决方案、临时绕过方法以及当前状态。
>
> 本项目通过基于 Rust 的 JVM 编排层启动并研究不同的 Minecraft 模组加载器。
>
> 部分问题已经解决，部分问题仅通过临时方案绕过，另一些问题仍在调查中。每个章节都会区分已验证的结果与尚未确认的解释。

**项目状态：** 实验性原型。Fabric、Forge 和 NeoForge 的启动实验已取得重要进展，但尚未实现完整的跨 Loader 互操作。已提交的 Java Agent 源码包含敌对生物镜像和 ACK/重试实现，但仍需打包并运行时验证。Rust Window Manager、Java Bridge 和只读 macOS 窗口发现也已加入；真实 Minecraft 窗口验证以及窗口操作尚未完成。

---

# 1. Fabric 1.21.4 启动问题

## 1.1 缺少 ASM 和 Sponge Mixin 依赖

<badge color="success">已解决</badge>

**症状**

Fabric 初始化失败；尝试加载 `KnotClient` 及其支持类时出现 `ClassNotFoundException`。

**根本原因**

手动构造的 Rust 类路径未包含 Fabric 所需的全部底层字节码操作库。

重要依赖包括：

- ASM
- ASM Tree
- ASM Analysis
- ASM Commons
- ASM Util
- Sponge Mixin

**解决方案**

扩展类路径构建逻辑，加入所需的 ASM 组件和 Sponge Mixin。

这些依赖位于本地 Minecraft 库目录中，并从沙盒环境复制或引用。

调查期间，其中一组依赖配置使用了 ASM 9.9.1。具体版本必须与所选 Loader 兼容。

**验证**

Fabric 成功越过此前的缺少类阶段，并继续进入 KnotClient 初始化流程。

---

## 1.2 `No game providers present on the class path`

<badge color="success">已解决</badge>

**症状**

Fabric 无法找到有效的 Minecraft 游戏提供程序，因此初始化失败。

典型错误：

```text
No game providers present on the class path
```

**根本原因**

手动启动的环境未能正确识别 Minecraft 游戏 JAR。

此外，Fabric 的类路径隔离机制阻止了游戏类通过预期的类路径配置被发现。

**解决方案**

显式指定游戏 JAR 的位置，并调整 Fabric 的类路径隔离配置。

相关 JVM 属性：

```text
-Dfabric.gameJarPath=/path/to/minecraft-client.jar
-Dfabric.debug.disableClassPathIsolation=true
-Dfabric.gameVersion=1.21.4
```

实际游戏 JAR 路径必须从本地 Minecraft 安装中动态解析。

**验证**

Fabric 识别到 Minecraft 游戏提供程序，并继续启动客户端。

---

## 1.3 复制整个 libraries 目录导致的类路径冲突

<badge color="success">已改善</badge>

**症状**

错误包括：

```text
NoSuchMethodError
ClassNotFoundException
```

部分故障涉及 Authlib、DataFixerUpper 和 LWJGL 等库。

**根本原因**

最初将整个 Minecraft `libraries` 目录复制到类路径中，导致同一依赖存在多个版本。

JAR 文件存在，并不代表它就是当前 Minecraft 安装所需的正确版本。

**解决方案**

实现 Rust 端的 `resolve_classpath()` 例程，用于读取 Minecraft 版本清单。

解析器会：

1. 解析 Minecraft 版本 JSON。
2. 读取各个库的声明。
3. 评估操作系统规则。
4. 根据当前平台和架构筛选库。
5. 解析所需的 JAR 路径。
6. 纳入适当的原生库分类器。

对于 Apple Silicon 测试环境，相关规则包括：

```text
macOS
arm64
natives-macos-arm64
natives-macos
```

**验证**

启动器成功根据 Minecraft 1.21.4 元数据解析所需依赖。

一次记录的依赖解析运行在后续类路径构建之前，从声明的 77 个库中解析出 32 个。

仍需进一步完善依赖解析，使其具备完整的可移植性和覆盖范围。

---

## 1.4 LWJGL 原生库加载失败

<badge color="success">已在测试的 macOS 配置中解决</badge>

**症状**

典型错误：

```text
Failed to locate library: liblwjgl.dylib
```

或者：

```text
MemoryUtil initialization failed
```

**根本原因**

LWJGL Java 库已就绪，但其平台专用原生二进制文件未被正确提取或定位。

**解决方案**

下载并提取适用于 macOS ARM64 的 LWJGL 原生库到专用目录。

配置：

```text
-Djava.library.path=/path/to/natives
```

**验证**

在测试环境中，Minecraft 客户端成功初始化图形和声音子系统。

---

## 1.5 运行时缺少 Netty 依赖

<badge color="warning">需要进一步验证</badge>

**症状**

Fabric 启动到 KnotClient 初始化阶段后，随后遇到：

```text
NoClassDefFoundError:
io/netty/util/ResourceLeakDetector$Level
```

**根本原因**

直接故障表明运行时无法解析所需的 Netty 类。

虽然 Guava 已下载并验证，但 Guava 与缺失的 Netty 类无关。

这说明成功解析一个依赖，并不代表完整的运行时依赖图都已就绪。

**调查**

已执行以下检查：

- 验证下载的 Guava JAR。
- 确认归档中存在预期的 Guava 类。
- 检查手动组装的类路径。
- 确认缺失的 Netty 类属于另一个独立的依赖解析问题。

**当前状态**

需要根据确切的 Minecraft 版本元数据和实际运行时类路径，验证 Netty 类路径问题。

修复必须确保选择的是正确的 Netty JAR，而不是任意版本。

---

# 2. NeoForge 1.21.1 启动问题 — ModLauncher 11

## 2.1 `ModLauncher null starting` 和 NullPointerException

<badge color="success">已解决／已绕过</badge>

**症状**

BootstrapLauncher 在 ModLauncher 初始化期间失败。

一个具有代表性的故障涉及：

```text
TypesafeMap.putIfAbsent
NullPointerException
```

启动日志还显示：

```text
ModLauncher null starting
```

**根本原因**

最初的调查聚焦于 ModLauncher 版本元数据和清单属性。

当 ModLauncher 作为 JPMS 模块加载时，其运行时元数据查询不一定与普通的基于类路径的 JAR 加载行为相同。

因此，如果实际加载路径无法访问预期元数据，`getImplementationVersion()` 可能返回 `null`。

具体故障也取决于所选的 ModLauncher 和 BootstrapLauncher 版本。

**解决方案／临时方案**

启动器不再依赖重写原始 JAR 清单，而是调整为构造预期的模块路径和传统类路径。

重要组件包括：

- `--module-path`
- `-DlegacyClassPath`
- BootstrapLauncher 依赖
- SecureJarHandler 依赖
- 正确的 JVM 参数顺序

与仅尝试修改清单相比，这种方法更可靠。

**验证**

NeoForge 1.21.1 成功越过此前的 ModLauncher 初始化故障，并进入 Minecraft 客户端启动流程。

---

## 2.2 `ServiceConfigurationError` 和早期窗口初始化

<badge color="success">临时方案已验证</badge>

**症状**

早期加载窗口崩溃，错误类似于：

```text
ServiceConfigurationError:
Unable to get public no-arg constructor
```

故障涉及：

```text
EarlyProgressVisualization
```

**根本原因**

早期窗口服务初始化路径与经过测试的手动构建启动环境不兼容。

服务发现与早期窗口实现之间的具体交互尚未完全隔离确认。

**临时方案**

修改：

```text
config/fml.toml
```

设置：

```toml
earlyWindowControl = false
```

**验证**

禁用早期窗口控制后，启动流程可以绕过这一特定故障。

这只是临时方案，并非适用于所有早期窗口初始化问题的通用解决方案。

---

## 2.3 `NoSuchElementException` 和缺少客户端类

<badge color="success">已在测试配置中解决</badge>

**症状**

NeoForge 客户端进程在启动期间因缺少类或异常而失败。

**根本原因**

对于手动构建的客户端环境，所提供的 `unix_args.txt` 并不足够。

有效的传统类路径中缺少一些客户端依赖。

调查的示例包括：

- ICU4J
- 文本转语音库
- LWJGL 组件

Java 模块封装也限制了一些反射访问路径。

**解决方案**

使用 Python 脚本检查已安装的库，并将缺少的依赖追加到有效的传统类路径中。

必要时添加额外的 JVM 模块访问选项。

例如：

```text
--add-opens
--add-exports
```

对于 macOS，还需要以下选项：

```text
-XstartOnFirstThread
```

此选项用于满足平台特定要求：必须在第一个应用程序线程上创建 GLFW 窗口。

**验证**

在测试配置中，NeoForge 1.21.1 成功进入 Minecraft 客户端环境。

---

# 3. Forge 1.16.5 启动问题 — ModLauncher 8

> 已对该版本进行调查，但尚未成功启动至可正常运行的客户端状态。以下内容记录了尝试过的解决方案及未解决的原因。

## 3.1 官方安装程序 JAR 拆分器哈希不匹配

<badge color="danger">macOS 原生安装尚未解决</badge>

**症状**

官方安装程序在拆分 JAR 阶段失败。

具有代表性的输出：

```text
Expected: xxx
Actual: xxx
```

哈希验证失败导致安装程序无法完成。

**调查**

考虑过多种可能性，包括 macOS 环境与安装程序依赖之间的压缩行为差异。

怀疑与系统 zlib 实现有关，但尚未得到确凿证实。

**尝试**

- 切换到 Java 8u312。
- 测试 Forge 36.2.42。
- 重复安装流程。

这些尝试均未解决该问题。

**临时方案**

在 Windows 虚拟机中运行官方安装程序。

将生成的文件（包括：

```text
srg.jar
extra.jar
```

）复制回 macOS。

这样无需依赖失败的 macOS 原生安装步骤，即可继续调查。

---

## 3.2 缺少 `fmlloader` 和 `FMLClientLaunchHandler`

<badge color="danger">尚未解决</badge>

**症状**

启动过程失败，并显示：

```text
Cannot find launch target fmlclient
```

**根本原因**

Forge 1.16.5 使用较旧的启动架构。

最初组装的依赖路径中没有所需的 FML 客户端启动类。

尝试寻找类似以下名称的独立构件未果：

```text
fmlloader-1.16.5-36.2.42.jar
```

检查过的 `universal.jar` 和 `client.jar` 均未提供所需的启动类。

**尝试过的临时方案**

尝试用本地 Prism Launcher 安装中找到的较新 FML 实现替代。

**最终故障**

较新的 ModLauncher 环境无法正确识别较旧的 FML 服务实现。

进程最终失败，并显示：

```text
Invalid Services found fml
```

**结论**

Forge 1.16.5 需要针对该版本的启动流程。替换为较新的 FML 或 ModLauncher 组件并不是可靠的兼容策略。

---

# 4. Forge 1.21.4 启动问题 — ModLauncher 10.2.4

> **状态更新：** Forge 1.21.4 已在本项目中实现实验性启动成功。以下问题记录了早期故障，以及最终形成可用配置的调查过程。

## 4.1 未能发现 ModLauncher JAR

<badge color="success">已在实验性启动中克服</badge>

**症状**

启动器报告：

```text
ERROR: ModLauncher JAR not found
```

**根本原因**

Rust 启动器预期某个特定的 JAR 位置和文件名，但本地安装中存在名称不同或重复的 ModLauncher 构件。

**调查**

检查实际的库目录，并验证预期的构件路径。

调整启动器的路径构造逻辑，使其符合已安装依赖的布局。

**结果**

在后续的实验性启动配置中，已克服 ModLauncher JAR 发现问题。

---

## 4.2 模块路径上缺少依赖

<badge color="success">已在实验性启动中克服</badge>

**症状**

JVM 报告模块路径依赖缺失，涉及的组件包括：

- SecureModules
- ForgeSPI
- FMLLoader
- Bootstrap API
- ASM

**根本原因**

JAR 文件存在于磁盘上，并不意味着 JVM 能将其解析为 JPMS 模块。

类路径条目和模块路径条目用途不同。

最初的配置未能始终如一地复现 Forge 引导流程所期望的模块布局。

**调查**

使用了以下工具：

```bash
jar --describe-module --file dependency.jar
```

以及：

```bash
javap -classpath dependency.jar -p some.package.SomeClass
```

检查模块描述符、依赖路径和重复条目。

**结果**

修订了模块路径构造；之后 Forge 1.21.4 启动实验得以继续成功推进。

---

## 4.3 引导服务发现失败

<badge color="success">已在实验性启动中克服；实现细节仍因版本而异</badge>

**症状**

最初找不到预期的服务描述文件：

```text
META-INF/services/net.minecraftforge.bootstrap.api.BootstrapEntryPoint
```

**根本原因**

最初的调查发现，预期的服务接口、实现类和服务注册机制之间存在不匹配。

不过，仅凭缺少描述文件不足以确定完整根本原因。

**尝试**

- 检查 JAR 模块描述符。
- 检查服务注册文件。
- 测试向经过处理的 ModLauncher JAR 注入服务描述文件。

**结果**

调整了引导配置，使后续 Forge 启动实验能够继续推进。

不应将手动注入服务描述文件视为通用解决方案；实际服务提供程序机制必须与 Forge 版本相匹配。

---

## 4.4 注入引导 JAR 最初未能解决启动问题

<badge color="success">已在后续配置中克服</badge>

**症状**

即使将引导 JAR 注入类路径开头，启动器仍持续报告类似以下错误：

```text
ModLauncher JAR not found
forge-client JAR missing
```

**根本原因**

成功插入类路径并不保证以下事项都正确：

- 文件发现。
- 模块解析。
- 服务加载。
- 引导入口点选择。

这些都是 Java 启动流程中彼此独立的阶段。

**调查**

调整类路径顺序、删除重复依赖条目，并修订 Rust 端的 JVM 参数构造。

**当前状态**

Forge 1.21.4 已通过实验性方式进入早期加载阶段。

仍需进一步测试，才能确认客户端完整启动及游戏运行的稳定性。

---

## 4.5 ModLauncher 模块重复和 `UnionFileSystemProvider`

<badge color="warning">需要进一步验证</badge>

**症状**

Forge 调查中遇到了重复的 ModLauncher 相关构件和模块解析冲突。

报告的一个冲突涉及：

```text
UnionFileSystemProvider
```

**根本原因**

多个依赖版本或重复的模块条目可能导致 JVM 解析到相互冲突的实现，或遇到模块层初始化失败。

库目录中存在多个名称相似的 JAR，因此需要区分实际选用的构件和未使用的替代项。

**调查**

- 检查 ModLauncher 10.2.4。
- 使用 `javap` 检查启动器构造函数和字节码。
- 检查模块路径构造。
- 检查是否存在重复的 ModLauncher 条目。
- 将预期模块布局与本地 Forge 安装进行比较。

**当前状态**

后续 Forge 1.21.4 启动实验已成功，但仍应加强依赖选择逻辑，以防止重复模块。

---

# 5. Rust、JNI 和 JVM 集成问题

## 5.1 JNI 0.22 API 变更

<badge color="success">已解决</badge>

**症状**

升级到 JNI crate 0.22 后，Rust JNI 集成产生了大量编译错误。

**根本原因**

与早期版本相比，JNI crate API 有了显著变化。

一些 API、字符串转换方式和对象构造方法不再与先前的实现相匹配。

**解决方案**

更新 Rust 集成，采用新的 API 模式。

重要组件包括：

```rust
InitArgsBuilder
JNIVersion::V1_8
jni_str!
jni_sig!
```

还启用了所需的 `invocation` 特性。

例如，构造 `JObjectArray` 时需要适当的对象初始化参数，包括：

```rust
JObject::null()
```

（在适用的情况下）。

**验证**

Rust 已能成功启动嵌入式 JVM，并通过 JNI 调用 Java 方法。

---

## 5.2 Java 模块封装和 `InaccessibleObjectException`

<badge color="success">已实现临时方案</badge>

**症状**

Java 17 和 Java 21 环境出现如下错误：

```text
InaccessibleObjectException
```

部分故障涉及：

```text
sun.security.util
java.util.jar
```

**根本原因**

Java 平台模块系统限制了对内部程序包的反射访问。

较旧的库和启动器可能默认假设可以不受限制地访问这些 API。

**解决方案／临时方案**

向 JVM 启动配置添加所需的模块访问参数。

例如：

```text
--add-exports java.base/sun.security.util=ALL-UNNAMED
--add-opens java.base/java.util.jar=ALL-UNNAMED
```

只有在所选 Loader 和依赖版本确有需要时，才应开放其他程序包。

**尚存问题**

过度使用 `--add-opens` 和 `--add-exports` 可能掩盖更深层的兼容性问题。长期目标是尽量减少这些标志，并采用适合对应版本的启动配置。

---

## 5.3 扫描库时出现 `UnsupportedClassVersionError`

<badge color="success">已实现过滤临时方案</badge>

**症状**

JVM 报告：

```text
UnsupportedClassVersionError
```

遇到的类文件主版本号示例包括：

```text
60.0
61.0
69.0
```

**根本原因**

本地 Minecraft 库目录包含多个世代的 Java 依赖和模组加载器组件。

宽泛的目录扫描可能会意外纳入由较新 Java 运行时编译的类。

例如：

| 类文件主版本号 | Java 版本 |
|---|---|
| 52 | Java 8 |
| 60 | Java 16 |
| 61 | Java 17 |
| 65 | Java 21 |
| 69 | Java 25 |

具体兼容性要求取决于所启动的 Minecraft 和 Loader 版本。

**解决方案／临时方案**

实现基于 Python 的 JAR 检查，读取类文件头，并在传统环境需要较旧 Java 版本时过滤掉不兼容的类。

**尚存问题**

过滤类文件不能替代依赖解析。

首选方法是在构建类路径之前，为每项依赖选择正确版本。

---

## 5.4 JVM 参数顺序

<badge color="success">已在测试启动配置中解决</badge>

**症状**

启动过程中，某些 JVM 属性和模块参数未能按预期解析。

**根本原因**

JVM 选项、系统属性、类路径参数和应用程序参数必须放在正确的位置。

例如，放在主类之后的 JVM 选项会被视为应用程序参数，而不是 JVM 配置。

**解决方案**

修订 Rust 启动命令的构造方式，将 JVM 选项放在应用程序入口点之前。

包括：

- `-D` 系统属性。
- `-X` 运行时选项。
- `--add-opens`。
- `--add-exports`。
- `--module-path`。
- 类路径参数。

**验证**

在测试的 Fabric 和 NeoForge 启动配置中，进程已能越过此前的参数顺序故障。

---

# 6. Rust DataBus 和 IPC 问题

## 6.1 DataBus 连接被拒绝

<badge color="success">已解决</badge>

**症状**

Java Agent 尝试连接以下地址时报告：

```text
Connection refused
```

目标地址为：

```text
127.0.0.1:25565
```

**根本原因**

Rust DataBus 尚未成功绑定 TCP 监听器时，Java Agent 就开始尝试连接。

原先的异步启动安排使监听器启动竞态更难发现。

在 Fusion 模式下，多个子进程还需要共享同一个 DataBus，而不是各自尝试绑定同一地址。

**解决方案**

修改 Rust DataBus 启动函数：

```rust
fn start_data_bus() -> std::io::Result<()>
```

现在会先同步绑定监听器，然后再启动 accept 线程。

启动错误会向调用方传播。

Fusion 模式由父进程启动一个共享 DataBus，并在两个子进程的环境中设置：

```text
JVM_COMBINER_BUS_EXTERNAL=1
```

除非存在此环境变量，否则独立的 Fabric 和 NeoForge 模式会各自启动 DataBus。

**验证**

独立的 NeoForge 成功生成类似以下日志：

```text
[DataBus] 路由总线已启动，监听 127.0.0.1:25565
[DataBus] 127.0.0.1:59996 注册为 'neoforge'
[BusAgent:neoforge] Connected to DataBus
```

Fabric 和 NeoForge 也已在并发启动时成功注册到同一个 Fusion DataBus。

---

## 6.2 客户端注册和消息路由

<badge color="success">原型已验证</badge>

**症状**

两个客户端都能通过 TCP 连接，但消息需要可靠的目标地址和来源身份。

**解决方案**

实现具名客户端注册和 JSON 消息路由。

客户端使用类似以下消息注册：

```json
{"register":"fabric"}
```

以及：

```json
{"register":"neoforge"}
```

消息使用目标字段：

```json
{
  "to": "neoforge",
  "type": "player_move",
  "x": 3.0,
  "y": 64.0,
  "z": 8.5
}
```

Rust DataBus 会将消息转发给已注册的目标，并附加发送方身份作为 `from`。

**验证**

已验证以下通信路径：

```text
Fabric -> DataBus -> NeoForge
NeoForge -> DataBus -> Fabric
```

`hello`／`ack` 路由测试已成功。

**重要区别**

当前实现是基于 TCP 的 IPC。

它不是共享内存总线，也不提供真正的零拷贝传输语义。

---

## 6.3 DataBus 断开和重新连接

<badge color="warning">需要进行更多压力测试</badge>

**症状**

某个沙盒可能丢失 TCP 连接，而另一个沙盒仍继续运行。

潜在后果包括：

- 移动消息丢失。
- 玩家状态过时。
- 重新连接后同步延迟。
- 发送方误以为旧状态已经传输。

**现有实现**

Java Agent 在套接字连接失败时会尝试重新连接。

Rust DataBus 为每个客户端维护独立连接和写入器通道。

**剩余工作**

- 明确跟踪连接状态。
- 实现带退避的重新连接。
- 重新连接后重新同步状态。
- 心跳监测。
- 处理重复或过时的消息。
- 检测目标客户端是否已断开。

TCP 写入成功不应被视为远端游戏已应用该状态的证明。

---

## 6.4 并发 TCP 写入

<badge color="success">已改善</badge>

**症状**

多个线程可能尝试向同一个客户端连接写入消息。

若没有同步措施，JSON 消息可能交错或损坏。

**解决方案**

Rust DataBus 为每个连接使用独立的写入线程和 `mpsc` 通道。

概念结构：

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

这样可以为每个连接串行执行实际套接字写入。

Java Agent 也会通过同步共享的 `PrintWriter` 来保护写入操作。

**尚存问题**

仍应在并发重新连接的情况下持续测试连接替换和关闭行为。

---

# 7. Java Agent 和运行时插桩问题

## 7.1 Java Agent 注入和类转换

<badge color="success">已验证</badge>

**目标**

注入运行时插桩，以便执行诊断或同步任务，而不必为每项任务都直接集成 Mixin。

**实现**

Rust 启动器通过 JVM 启动参数提供 Java Agent：

```text
-javaagent:/path/to/bus-agent.jar
```

Agent 使用：

```java
public static void premain(
    String agentArgs,
    Instrumentation instrumentation
)
```

`ClassFileTransformer` 会观察 Fabric 运行时加载的类。

**验证**

示例输出：

```text
[BusAgent:fabric] premain
[BusAgent:fabric] MC-CLIENT: net/minecraft/client/main/Main loader=KnotClassLoader
[BusAgent:fabric] MC-CLIENT: net/minecraft/client/ClientBrandRetriever loader=KnotClassLoader
```

这表明 Agent 能够初始化，并观察由 Fabric 的 KnotClassLoader 加载的类。

这并不意味着任意运行时类修改或 Mixin 冲突解决功能已经实现。

---

## 7.2 Fabric 玩家监视器 NullPointerException

<badge color="success">已解决</badge>

**症状**

Fabric 监视线程反复产生 `NullPointerException` 错误。

故障发生在：

```text
BusAgent.monitorFabricPlayer
```

附近。

**根本原因**

Fabric 尚未完全初始化游戏实例时，监视器就尝试检查 Minecraft 实例。

相关引用当时仍为：

```java
fabricMcInstance == null
```

**解决方案**

添加空值保护和等待／重试路径。

监视器现在会等待 Minecraft 实例就绪后再尝试查找玩家。

**验证**

监视器输出了正常的初始化信息：

```text
[BusAgent:fabric][INIT] SUCCESS: flk
```

此前反复刷屏的 NPE 已消除。

---

## 7.3 NeoForge 玩家发现初始化竞态

<badge color="success">玩家发现功能已解决</badge>

**症状**

DataBus 收到 Fabric 的 `player_move` 消息时，NeoForge 仍可能处于以下状态：

```java
nfMcInstance == null
```

或：

```java
nfMcClass == null
```

原先接收端的初始化流程依赖收到同步消息。

如果消息在 Minecraft 客户端就绪前到达，同步处理程序就无法安全地访问游戏状态。

**根本原因**

NeoForge 的 Minecraft 生命周期与 DataBus 连接生命周期相互独立。

原实现只有在接收处理程序需要时才初始化 NeoForge Minecraft 引用。

因此，本地客户端初始化与远端消息之间形成了时序依赖。

**解决方案**

添加独立的 NeoForge 监视线程。

监视线程会反复执行以下操作：

1. 查找已加载的 `net.minecraft.client.Minecraft` 类。
2. 调用静态 `getInstance()` 方法。
3. 等待 Minecraft 实例变为非空。
4. 通过 `Minecraft.execute()` 安排就绪状态检查。
5. 检查 `ClientLevel`。
6. 检查 `LocalPlayer`。
7. 记录就绪状态的变化。

监视器还使用原子标记，避免重复安排探测任务。

**验证**

观察到了以下消息：

```text
[BusAgent:neoforge][MONITOR] thread started
[BusAgent:neoforge][INIT] Minecraft instance found
[BusAgent:neoforge][INIT] ClientLevel ready
[BusAgent:neoforge][LIVE] LocalPlayer ready
```

这验证了 NeoForge 玩家发现在测试客户端会话中有效。

但这尚未验证双向状态同步。

---

## 7.4 运行时反射和 Minecraft 映射差异

<badge color="warning">实现因版本而异</badge>

**症状**

在不同运行时环境中，Minecraft 方法不一定使用相同的方法名称。

如果运行时使用混淆名称，直接使用开发环境名称可能会失败。

**Fabric 实现**

经过测试的 Fabric 1.21.4 环境需要通过反射访问方法；这些方法是通过本地映射和运行时检查确定的。

当前实现使用以下方法名称：

```text
dA
dC
dG
dL
dN
cj
```

在经过测试的运行时中，这些方法对应玩家位置、朝向和疾跑状态的访问器。

**NeoForge 实现**

NeoForge 使用不同的运行时设置。

NeoForge 发送端设计为独立解析方法，而不复用 Fabric 的方法缓存。

预期的 Mojang 风格方法名称包括：

```java
getX()
getY()
getZ()
getYRot()
getXRot()
isSprinting()
```

**剩余工作**

- 动态解析方法映射。
- 针对每个确切的 Minecraft 版本验证映射。
- 在可行时避免硬编码名称。
- 检测缺失或不兼容的方法，且不要反复刷屏。
- 支持未来的 Minecraft 版本。

---

# 8. 跨版本玩家镜像和状态同步

测试的版本组合：

```text
Fabric     Minecraft 1.21.4
NeoForge   Minecraft 1.21.1
```

## 8.1 NeoForge 远程玩家初始化

<badge color="success">已实现原型</badge>

**目标**

在 NeoForge 沙盒中创建远程玩家实体，用来表示 Fabric 本地玩家。

**实现**

NeoForge Agent 使用反射访问 Minecraft 运行时类，并构造 `RemotePlayer`。

镜像实体使用易于识别的名称：

```text
FabricMirror
```

其 UUID 根据与 Fabric 镜像关联的稳定标识符生成。

使用较大的候选实体 ID，以降低与普通游戏实体发生冲突的风险。

**验证**

在测试环境中，NeoForge 客户端成功显示注入的镜像玩家。

这证明 NeoForge 运行时能够接受由当前基于反射的原型远程控制的玩家实体。

**剩余工作**

- 稳定的 UUID 管理。
- 实体 ID 冲突处理。
- 防止重复实体。
- 重生和断开连接处理。
- 维度切换。
- 源端断开时移除玩家。
- 兼容不同 Minecraft 版本。

---

## 8.2 Fabric 和 NeoForge 镜像出现在不同环境中

<badge color="warning">调查中</badge>

**症状**

Fabric 显示本地玩家身处森林环境。

NeoForge 也显示了 `FabricMirror` 实体，但镜像出现在一个地下封闭区域。

两个客户端显示的周围环境似乎并不相同。

**可能原因**

客户端运行的是不同的 Minecraft 版本：

```text
Fabric:   1.21.4
NeoForge: 1.21.1
```

可能的解释包括：

- 地形生成行为不同。
- 世界种子不同。
- 维度状态不同。
- 区块可用性不同。
- 客户端世界初始化时机不同。
- 坐标或坐标解释不同。
- 镜像实体的初始化和定位方式不同。

具体原因尚未确认。

**当前调查**

DataBus 已成功转发 Fabric 的玩家坐标，NeoForge 也成功显示镜像实体。

下一步是比较两个客户端的：

- X、Y 和 Z 坐标。
- 维度标识符。
- 世界种子。
- 已加载区块的位置。
- 本地玩家的位置。
- 镜像玩家的位置。

**剩余工作**

确定差异是由世界生成、世界状态、区块加载还是实体同步导致。

即使坐标相同，也不能保证不同 Minecraft 版本的地形或世界状态一致。

---

## 8.3 Minecraft 窗口失去焦点时自动暂停

<badge color="warning">需要完成双客户端最终验证</badge>

**症状**

切换到浏览器或其他应用程序时，一个或两个 Minecraft 客户端会自动进入 ESC 暂停菜单。

这妨碍了同时测试两个客户端。

**调查**

检查了游戏窗口失去焦点时 Minecraft 的自动暂停行为。

测试了相关键盘设置：

```text
F3 + P
```

也检查了各个客户端配置：

```text
options.txt
```

**剩余工作**

验证两个客户端在焦点变化后是否都能继续运行。

尤其要确认：

- 世界模拟仍在继续。
- 游戏线程仍在处理已安排的任务。
- 玩家状态采样仍在继续。
- DataBus 消息仍在传输。
- 镜像实体仍在更新。

仅禁用自动暂停并不能证明所有同步任务都仍然运行。

---

## 8.4 Fabric 到 NeoForge 的状态同步

<badge color="success">概念验证已通过</badge>

**实现**

Fabric Agent 会：

1. 定位 Fabric Minecraft 实例。
2. 查找本地玩家。
3. 提取坐标、旋转角度和疾跑状态。
4. 构造 JSON `player_move` 消息。
5. 将消息发送到 Rust DataBus。

DataBus 将消息路由到 NeoForge。

NeoForge Agent 接收并解析消息，然后通过 Minecraft 游戏线程更新镜像实体。

**消息示例**

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

**已验证的能力**

- Fabric 玩家状态提取。
- TCP 消息传输。
- Rust DataBus 路由。
- NeoForge 消息接收。
- NeoForge JSON 解析。
- NeoForge 镜像实体更新。

**剩余工作**

当前实现属于概念验证。还需要进一步测试移动平滑度、网络延迟、实体生命周期和版本兼容性。

---

## 8.5 NeoForge 到 Fabric 的同步 — 历史设计

<badge color="warning">已由单向权威架构取代</badge>

**历史目标**

早期原型曾考虑反向的玩家状态同步路径：

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

原始设计尝试让两个沙盒都发布本地玩家的移动状态，并镜像远端玩家。

### 为什么弃用了这种设计

双向的权威玩家状态同步会产生两个相互竞争的事实来源。

不同的 Minecraft 版本、彼此独立的游戏循环以及不同的世界状态会引发冲突，并需要额外的所有权管理、冲突解决和反馈循环预防机制。

当前原型不需要两个权威玩家状态发布方。

### 当前实现状态

- 已从修改过的 NeoForge 监视器源码中移除对 `sampleNeoForgePlayer(player)` 的活动调用。
- NeoForge 预期作为仅接收端客户端运行。
- 旧的 `sampleNeoForgePlayer()` 和 `sendNeoForgePlayer()` 方法仍保留在 Java 源码中，但除非其他代码路径调用，否则不会执行。
- 修改过的 Java 源码尚未重新构建并打包到正式的 `bus-agent.jar` 中。
- Rust DataBus 源码已按连接注册名拒绝非 Fabric 发布的 `player_move`、`monster_spawn`、`monster_update` 和 `monster_remove`，并返回 `state_authority_violation`；运行时仍待验证。
- 注册名由连接客户端自行声明，不构成真实进程身份认证。

### 当前架构策略

Fabric 是玩家移动状态的唯一权威发布方。

NeoForge 接收 Fabric 的 `player_move` 消息并更新远程玩家镜像。NeoForge 不得向 Fabric 发布权威 `player_move` 消息。

未来的反向消息可以包括明确定义的交互请求、确认、诊断信息或能力报告。每种消息都需要各自的协议定义和授权规则。

**剩余工作**

- 重新构建并打包 Java Agent。
- 运行时验证授权拒绝、不转发、连接保持以及正常 Fabric 状态传输。
- 限制或认证客户端自报的注册名，并验证重连和镜像恢复。

保留历史上的双向设计，是为了提供架构背景。

---

## 8.6 敌对生物镜像：已提交源码，运行时验证待完成

<badge color="warning">Java 源码已提交 · 打包和运行时验证待完成</badge>

已提交的 `bus_test/MonsterSync.java` 在 Fabric 客户端线程按 UUID 采样可见且存活的敌对 `Monster`，并发送 `monster_spawn` 和变化后的 `monster_update`。待确认状态包含序列号，未收到 ACK 时会按退避间隔重试。

已提交的 `bus_test/BusAgent.java` 包含 NeoForge 对 `monster_spawn` / `monster_update` 的解析、实体创建/更新、检测到世界变化时清理怪物镜像，以及应用状态后发送 `monster_ack`。Fabric Agent 接收 ACK 并确认对应序列。源码实现并不能证明打包 JAR 或真实客户端运行成功；在视为最新可用版本前，应重新构建并验证 JAR。

`monster_remove` 仍未实现；生物不在采样/渲染列表中不能单独作为已移除的证据。

**剩余工作**

- 从当前 Java 源码重新构建 Agent，并确认 JAR 与源码一致。
- 在 Fabric/NeoForge 客户端运行时验证怪物生成/更新、镜像应用、ACK/重试和世界切换清理。
- 验证断开、重连和新状态恢复。
- 定义可靠的生物移除语义并发布、应用 `monster_remove`。
- 在完成端到端测试前，不要把怪物同步描述为已完成。

## 8.7 窗口注册与只读 macOS 窗口发现

<badge color="warning">原型已实现；真实 Minecraft 窗口验证待完成</badge>

Rust Window Manager 接受 Java Bridge 的 Loader 进程注册，并跟踪进程
PID、Loader、角色和协议版本；Rust 所属进程可读取这些注册信息。Java
Bridge 支持连接重试，并在服务端明确拒绝后停止重试。Rust 测试覆盖注册
生命周期和 PID 关联；Java 测试程序覆盖 Fabric、Forge、NeoForge 标签的
注册、重连和拒绝行为。

macOS 实现可枚举屏幕上的窗口，并按 PID 将窗口与已注册 JVM 关联。用户
授予辅助功能权限后，可选的 Accessibility API 探针可以读取窗口标题和
边界。这些操作均为只读：当前原型不会移动、缩放、嵌入或合成窗口。测试
套件不能证明其已在真实 Minecraft 窗口中工作，也不能证明该功能可移植到
macOS 以外的平台。

**剩余工作**

- 使用真实 Minecraft 客户端验证进程注册和窗口关联，包括 macOS 权限流程。
- 在实现窗口控制、嵌入或合成前，先确定预期用户体验。
- 分别研究 Windows 和 Linux；当前 macOS 探针未实现这些平台的窗口 API。

# 9. ClassLoader 隔离和 Mixin 兼容性

<badge color="warning">尚未解决的主要研究领域</badge>

## 9.1 `parent = null` 并不是完整的隔离策略

**问题**

原始架构曾考虑使用：

```java
new URLClassLoader(urls, null)
```

来隔离不同的 Loader 环境。

但是，完全隔离且没有父加载器的 ClassLoader 无法自动访问真实 Loader 所需的 Java 平台类及其他共享运行时依赖。

**挑战**

真实的 Minecraft Loader 依赖：

- `java.lang.*`
- `java.util.*`
- 反射 API。
- Java 网络功能。
- 原生库接口。
- 运行时服务。
- 共享平台接口。

**研究方向**

研究受控的父 ClassLoader 或混合委派模型。

目标是在隔离 Loader 专属类的同时，不破坏对所需 Java 平台类的访问。

---

## 9.2 独立 Loader 环境之间的 Mixin 冲突

<badge color="warning">尚未解决</badge>

**问题**

Fabric 和 Forge／NeoForge 使用不同的 Mixin 集成和转换流程。

在共享 JVM 中运行这两个环境会带来以下问题：

- 转换顺序。
- 对同一目标类的重复转换。
- 类定义的所有权。
- 注入冲突。
- 不同类加载器之间共享的静态状态。
- 隔离环境之间的转换可见性。

**可能的方案**

- 独立的转换环境。
- 明确的转换所有权。
- 字节码检查和冲突检测。
- 基于优先级的转换规则。
- 受控的兼容接口。

目前尚未实现通用的 Mixin 冲突解决方案。

---

## 9.3 共享 Minecraft 类和原生运行时状态

<badge color="warning">尚未解决</badge>

**问题**

当两个环境依赖表示相似或相同 Minecraft 运行时概念的类时，ClassLoader 隔离很难处理。

即使两个 ClassLoader 能独立加载相同类名，得到的 Java 类型也不能互换。

这会导致以下问题：

- 实体引用。
- Minecraft 客户端实例。
- 静态单例字段。
- 事件对象。
- 渲染状态。
- 原生资源。
- 共享游戏数据。

**研究方向**

将 Loader 专属对象保留在各自的运行时边界内，并通过显式接口交换序列化数据。

避免在隔离环境之间直接传递任意 Java 对象。

---

# 10. 性能和同步挑战

## 10.1 不同的 Minecraft Tick 速率

<badge color="warning">尚未全面调查</badge>

**问题**

不同的 Minecraft 客户端可能不会以相同速率处理游戏 Tick。

可能的原因包括：

- 不同的帧率。
- 不同的 CPU 调度。
- 不同的 JVM 垃圾回收行为。
- 不同的世界加载时间。
- 不同的客户端暂停状态。

因此，简单的位置更新无法保证在两个客户端的同一个逻辑 Tick 到达。

**后续工作**

研究：

- 带时间戳的消息。
- 序列号。
- 插值。
- 外推。
- 过时状态检测。
- 有界更新队列。

---

## 10.2 锁步同步和双重卡顿

<badge color="warning">设计方面的顾虑</badge>

**问题**

严格的锁步架构可能导致两个客户端都等待对方的游戏状态推进。

这有可能造成双重卡顿问题：

1. 一个客户端等待同步消息。
2. 另一个客户端因自身帧处理或 Tick 处理而延迟。
3. 两个客户端都出现额外延迟。
4. 同步延迟在两个环境之间传播。

**设计方向**

初始实现应避免让 Rust 内核成为全局游戏 Tick 权威方。

相反，应研究异步状态复制，并采用：

- 本地模拟。
- 带时间戳的更新。
- 有界缓冲。
- 插值。
- 明确的一致性规则。

DataBus 应传输状态，而不是控制每一个游戏 Tick。

---

## 10.3 防作弊和服务器权威交互

<badge color="warning">未来研究</badge>

**问题**

部分玩家和服务器环境对防作弊有严格要求。

直接注入远程实体或修改客户端状态，在此类环境中可能不可接受。

仅同步玩家移动无法解决以下问题：

- 服务器端权威性。
- 碰撞验证。
- 物品栏一致性。
- 攻击注册。
- 交互顺序。
- 反作弊兼容性。

**设计方向**

在实现左键和右键同步之前，优先保证版本兼容性和实体状态复制。

未来的交互同步应区分：

- 客户端视觉状态。
- 服务器权威游戏操作。
- 已验证的交互事件。
- 复制得到的结果。

本项目不应假设客户端实体注入等同于合法的服务器端玩家。

---

# 11. 依赖解析和跨平台支持

## 11.1 静态依赖路径

<badge color="warning">部分实现</badge>

**问题**

一些实验性启动配置仍依赖手动检查依赖路径或复制 JAR 文件。

这使启动器容易受到以下因素影响：

- Minecraft 版本变更。
- Loader 版本变更。
- 库更新。
- 不同的安装布局。

**当前实现**

Fabric 依赖解析器会读取 Minecraft 版本元数据并应用平台规则。

部分 Forge 和 NeoForge 配置仍需要更专门的模块路径处理。

**剩余工作**

实现统一的依赖解析层，理解以下内容：

- Minecraft 版本清单。
- Fabric Loader 依赖。
- Forge 和 NeoForge 安装程序元数据。
- JPMS 模块依赖。
- 平台专用原生库。
- 依赖校验和。

---

## 11.2 macOS 特有的启动行为

<badge color="warning">部分处理</badge>

**问题**

部分启动要求仅适用于 macOS。

示例：

```text
-XstartOnFirstThread
```

以及 ARM64 原生库的选择。

**当前实现**

启动器已为经过测试的 macOS 环境加入平台专用依赖筛选和原生库路径。

**剩余工作**

在以下环境中验证相同的启动流程：

- macOS Intel。
- macOS Apple Silicon。
- Windows x86-64。
- Linux x86-64。
- Linux ARM64（如果所选 Minecraft 和 Loader 版本支持）。

---

# 12. 当前问题状态总结

| 领域 | 当前状态 |
|---|---|
| Rust JNI 集成 | <badge color="success">已验证</badge> |
| 基本 URLClassLoader 隔离 | <badge color="success">已验证</badge> |
| Fabric 1.21.4 启动 | <badge color="success">已验证</badge> |
| NeoForge 1.21.1 启动 | <badge color="success">已验证</badge> |
| Forge 1.21.4 启动 | <badge color="success">实验性成功</badge> |
| Forge 1.16.5 启动 | <badge color="danger">尚未解决</badge> |
| Rust TCP DataBus | <badge color="success">已验证</badge> |
| DataBus 连接启动竞态 | <badge color="success">已解决</badge> |
| 具名客户端注册和路由 | <badge color="success">已验证</badge> |
| 源码级单向状态消息拒绝 | <badge color="warning">已实现；运行时验证待完成</badge> |
| DataBus 注册身份认证 | <badge color="warning">未实现；注册名由客户端自报</badge> |
| Java Agent 注入 | <badge color="success">已验证</badge> |
| Fabric 玩家发现 | <badge color="success">已验证</badge> |
| NeoForge 玩家发现 | <badge color="success">已验证</badge> |
| Fabric 到 NeoForge 的状态传输 | <badge color="success">概念验证已通过</badge> |
| NeoForge RemotePlayer 注入 | <badge color="success">概念验证已通过</badge> |
| NeoForge 到 Fabric 的权威玩家状态传输 | <badge color="warning">不属于当前架构目标</badge> |
| Fabric 敌对生物采样及生成/更新 | <badge color="warning">源码已提交；打包与端到端验证待完成</badge> |
| NeoForge 怪物镜像及 ACK/重试 | <badge color="warning">已提交源码；打包与运行时验证待完成</badge> |
| Rust Window Manager 与 Java Bridge 生命周期 | <badge color="success">单元测试和 Bridge 测试程序通过</badge> |
| macOS 窗口发现和 PID 关联 | <badge color="warning">已实现；真实 Minecraft 验证待完成</badge> |
| 窗口移动、嵌入或合成 | <badge color="warning">尚未实现</badge> |
| 怪物移除事件 | <badge color="warning">尚未实现</badge> |
| 双向玩家镜像 | <badge color="warning">尚未完成</badge> |
| 跨版本世界一致性 | <badge color="warning">调查中</badge> |
| 同一 JVM 中的真实 Loader 隔离 | <badge color="warning">尚未解决</badge> |
| 通用 Mixin 冲突解决 | <badge color="warning">尚未解决</badge> |
| 真正的零拷贝 DataBus | <badge color="warning">尚未实现</badge> |
| 通用模组互操作 | <badge color="warning">尚未实现</badge> |
| 跨平台验证 | <badge color="warning">未完成</badge> |

---

# 13. 建议的开发优先级

以下顺序旨在降低当前原型尚未稳定时扩展架构所带来的风险。

### 优先级 1 — 强制并稳定单向玩家状态权威架构

**目标：** 将 Fabric 设为唯一权威玩家状态发布方，并确保 NeoForge 仅接收数据。

- [x] 将 Fabric 确立为唯一权威玩家状态来源。
- [x] 从已编辑的监视器源码中移除活动的 NeoForge 玩家状态采样调用。
- [x] 在 Rust DataBus 源码中拒绝非 Fabric 的玩家/怪物状态消息，且不转发被拒绝消息。
- [ ] 重新构建并打包更新后的 Java Agent。
- [ ] 限制或认证由客户端自行声明的 DataBus 注册名。
- [ ] 运行时验证未经授权消息拒绝、不转发及发送端连接保持。
- [ ] 保留明确授权的反向消息类型。
- [ ] 验证 Fabric 到 NeoForge 的玩家状态传递。
- [ ] 测试重新连接行为和新状态传输。
- [ ] 添加协议版本控制、序列号、时间戳和会话代次。
- [ ] 拒绝过时或不属于当前会话的玩家状态更新。

### 优先级 2 — 改进实体生命周期管理

- [ ] 稳定的远程玩家 UUID。
- [ ] 防止实体 ID 冲突。
- [ ] 生成和移除实体的处理。
- [ ] 维度切换处理。
- [ ] 断开连接时的清理。

### 优先级 3 — 完成敌对生物同步原型

- [x] Fabric 按 UUID 采样敌对生物，并从当前 Java 源码发送生成/更新消息。
- [x] 已提交 Java 源码加入 NeoForge 生成/更新接收、实体镜像和 ACK/重试。
- [ ] 重新构建并打包 Agent，运行时验证完整的生成/更新和 ACK/重试链路。
- [ ] 实现并验证可靠的 `monster_remove` 发布和接收。
- [ ] 测试世界切换、断开和重连恢复。

Java 源码已进入 `main` 历史；怪物同步尚未完成端到端验证。

### 优先级 4 — 提高同步质量

- [ ] 带时间戳的消息。
- [ ] 序列号。
- [ ] 过时状态检测。
- [ ] 插值。
- [ ] 网络延迟测量。
- [ ] 不依赖 Tick 速率。

### 优先级 5 — 扩大跨版本兼容性

- [ ] 动态验证方法映射。
- [ ] 测试双方使用相同 Minecraft 版本的情况。
- [ ] 测试不同世界种子。
- [ ] 比较维度和区块状态。
- [ ] 测试多个 Loader 版本。

### 优先级 6 — 回归原始微内核架构

- [ ] 开发受控的 ClassLoader 委派模型。
- [ ] 研究在同一 JVM 内隔离真实 Loader。
- [ ] 定义 Mixin 转换边界。
- [ ] 开发稳定的共享数据接口。
- [ ] 研究共享内存传输。

## 开发顺序

计划的实现顺序如下：

1. 重新构建并打包当前 Java Agent 源码。
2. 限制自报的 DataBus 注册名，运行时验证状态消息拒绝、不转发和连接保持。
3. 使用打包后的 Agent 验证 Fabric 到 NeoForge 的玩家状态传递和重连恢复。
4. 运行时验证已提交的怪物镜像及 ACK/重试链路。
5. 实现并测试可靠的 `monster_remove` 事件。
6. 改进实体生命周期处理和故障恢复。
7. 添加协议版本控制、时间戳、序列号和会话代次。
8. 扩展实体与游戏玩法同步功能。
9. 继续重构启动器并测试跨版本兼容性。

## 当前开发重点

当前重点是运行时验证单向玩家状态权威策略，并重新打包/测试已提交的怪物镜像和 ACK/重试实现。新加入的窗口管理原型还需在真实 macOS Minecraft 窗口中验证；目前仅支持观察窗口。

**下一个里程碑：**

重新构建 Java Agent；验证 Rust DataBus 状态消息拒绝、不转发和连接保持；运行时测试玩家与怪物消息传输、ACK/重试、重连恢复，并实现可靠的怪物移除事件。

---

## 最后说明

JVM Mod Combiner 是一个实验性系统项目。

成功启动 Minecraft、成功传输 TCP 消息以及成功注入远程玩家，是彼此独立的工程里程碑。任何一项单独成功，都不能证明已实现完整的跨 Loader 兼容。

本项目有意记录失败的实验和不完整的解决方案，因为理解故障模式对于设计稳定的运行时架构至关重要。

当前首要目标是先让现有的双进程原型可靠运行，再尝试更具挑战性的同一 JVM 隔离和通用模组互操作。
