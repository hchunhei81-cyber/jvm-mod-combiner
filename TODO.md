# JVM Mod Combiner: 开发路线图 (TODO.md)

## 🟢 已完成 (Done)
- [x] **PCL Mac MVP 发布**：Avalonia UI 跨平台重构，打包 `.app` 并发布 v1.0.1 Release。
- [x] **Fabric 1.21.4 跑通**：Rust 微内核成功拉起 Fabric Loader，主菜单渲染、音效及纹理加载正常。
- [x] **NeoForge 1.21.1 跑通**：成功绕过官方 Bootstrap 限制，主菜单渲染，支持显示 Mod 数量。
- [x] **JNI 基础与沙盒隔离**：验证 `URLClassLoader` 物理隔离，同名类互不冲突。

---

## 🟡 高优先级 (High Priority)

### 1. 清理 Rust 与 Python 的“技术债”
- [ ] **将 Python 逻辑重写为 Rust**：
  - 目前依赖大量的 Python 脚本动态扫描、过滤、打补丁。
  - 需要将 `resolve_classpath`、版本过滤、下载逻辑迁移到 Rust 原生实现。
- [ ] **重构 `launch_neoforge_subprocess` 函数**：
  - 去除外部 `forge_1165_launch.sh` / `neoforge_launch.sh` 脚本的依赖。
  - 在 Rust 内部动态拼装 `--module-path`、`-DlegacyClassPath` 和 JVM flags。
  - 实现真正的“一键启动”沙盒 B。
- [ ] **清理打包与构建产物**：
  - 按照 `.gitignore` 清理无用的大文件。
  - 统一 README 和实际代码的目录结构。

### 2. Forge 1.16.5 的终极方案 (备选)
- [ ] **验证虚拟机提取文件（当前进度）**：
  - 将 Windows 虚拟机中生成的完整 1.16.5 核心依赖（`srg.jar`, `extra.jar`, `fmlloader.jar`）拷回 Mac 测试。
- [ ] **放弃兼容 1.16.5**：
  - 鉴于官方 Installer 在 macOS 的哈希死锁以及 Maven 仓库的 404 问题，如果跨版本借用（1.17 `fmlloader`）失败，考虑在项目文档中正式宣布放弃对 1.16.5 的微内核原生支持，建议用户通过 Prism 等启动器运行旧版。

---

## 🔵 中优先级 (Medium Priority)

### 3. 沙盒之间的数据总线 (Data Bus)
- [ ] **设计跨进程通信 (IPC)**：
  - 由于 Fabric 和 NeoForge 完全运行在不同的 Subprocess 中，需要设计 Rust 侧的 IPC 机制（如共享内存 / Unix Domain Socket）。
- [ ] **零拷贝数据传输**：
  - 探索如何在两个 JVM 进程之间传递渲染数据或方块状态，而不产生大量内存开销。

### 4. Mixin 冲突处理策略
- [ ] **优先级白名单机制**：
  - 如果沙盒 A（Fabric）和沙盒 B（NeoForge）都需要修改同一个原版类，设计一套基于优先级的字节码重写拦截规则。
- [ ] **字节码合并**（长远目标）：
  - 利用 ASM 或字节码库，在 Rust 侧将两边的 Mixin 修改合并后，再注入给 Minecraft 类加载器。

---

## 🟣 低优先级 / 未来规划 (Low Priority / Future)

### 5. 跨平台支持
- [ ] **Windows 支持**：
  - 适配 Windows 下的 `java.exe` 路径、`JAVA_HOME` 环境变量及 `%APPDATA%` 目录。
- [ ] **Linux 支持**：
  - 适配路径分隔符、原生的 LWJGL natives 提取。
- [ ] **路径处理**：
  - 彻底修复 macOS 和 Windows 路径斜杠（`/` 和 `\`）在 JVM 参数和 JSON 解析中的兼容问题。

### 6. 动态依赖解析
- [ ] **实现 Manifest 解析**：
  - 目前为了避免冲突，是大范围扫描或手动写死白名单。
  - 需要实现一个完整的 Version JSON 解析器，按规则（os/arch）精确下载并组装所需的依赖，而不是全量复制 `libraries` 目录。
