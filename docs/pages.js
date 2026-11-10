(() => {
  "use strict";

  const common = {
    en: {
      skip: "Skip to main content",
      docs: "Documentation",
      overview: "Overview",
      problems: "Problems & solutions",
      todo: "TODO",
      progress: "Progress",
      github: "GitHub ↗",
      languageLabel: "Language",
      themeLabel: "Toggle theme",
      menuLabel: "Open menu",
      light: "LIGHT",
      dark: "DARK",
      rustLink: "Rust DataBus source",
      agentLink: "Java Agent source",
      mobLink: "MonsterSync source",
    },
    zh: {
      skip: "跳转到主要内容",
      docs: "文档导航",
      overview: "项目概览",
      problems: "问题与解决方案",
      todo: "待办事项",
      progress: "最新进展",
      github: "GitHub ↗",
      languageLabel: "语言",
      themeLabel: "切换主题",
      menuLabel: "打开菜单",
      light: "浅色",
      dark: "深色",
      rustLink: "Rust DataBus 源码",
      agentLink: "Java Agent 源码",
      mobLink: "MonsterSync 源码",
    },
  };

  const translations = {
    progress: {
      en: {
        documentTitle: "Project Progress — JVM Mod Combiner",
        metaDescription:
          "Current JVM Mod Combiner progress, implementation status, and latest source update.",
        eyebrow: "PROJECT STATUS",
        pageTitle: "Progress & latest update",
        pageIntro:
          "A source-based snapshot of verified work, current implementation, and the next validation steps.",
        latestStatus: "LATEST UPDATE · 2026-11-10",
        latestTitle: "Hostile-mob mirror pipeline added in the working tree",
        latestText:
          "The current Java sources add NeoForge handling for Fabric monster spawn/update messages, entity mirrors, and an ACK/retry path. The Rust DataBus continues to restrict protected state messages to the registered Fabric name.",
        branchLabel: "Main-branch context:",
        branchText:
          "The checked main history includes loader detection/DataBus registration (f2171ad), Minecraft 1.21.4 Agent/classpath work (f146f27), and the documentation refresh (e212414). The Java mob-mirror additions described here are uncommitted working-tree edits, not changes recorded in main.",
        verifyLabel: "Verification status:",
        verifyText:
          "These Java additions are source-level implementation only. The packaged Agent must be rebuilt from the current sources, then tested in the live Fabric/NeoForge setup. Monster removal is not implemented; do not treat mob synchronization as complete.",
        doneStatus: "Verified foundation",
        doneTitle: "Capabilities already established",
        doneIntro:
          "Previously verified prototype milestones remain separate from the newest untested source work.",
        done1: "Rust/JNI launcher and Java Agent initialization",
        done2: "Fabric and NeoForge startup experiments, plus experimental Forge startup",
        done3: "Rust TCP DataBus registration and JSON routing",
        done4: "Fabric-to-NeoForge player-state and RemotePlayer proof of concept",
        workStatus: "In current source · runtime test pending",
        workTitle: "Working-tree implementation",
        workIntro:
          "These changes are present in the current bus_test source, but are not yet in the main-branch history.",
        work1:
          "NeoForge parses monster_spawn/monster_update, creates or updates mirrors, and clears mob mirrors when a world change is observed.",
        work2:
          "NeoForge sends monster_ack after applying a message; Fabric tracks pending sequence-numbered states and retries until acknowledged.",
        work3:
          "The existing Rust policy rejects non-Fabric player/monster state messages, but registration names are self-asserted and are not authenticated identities.",
        nextStatus: "Next steps",
        nextTitle: "Build, test, then complete lifecycle",
        next1: "Rebuild/package the Java Agent from current source and confirm the JAR contains the latest code.",
        next2: "Runtime-test normal player and mob delivery, unauthorized rejection, no forwarding, and connection continuity.",
        next3: "Test monster ACK/retry behavior, world changes, disconnects, and reconnect recovery.",
        next4: "Design and publish reliable monster_remove events; absence from the sampled render list is not proof of despawn.",
        next5: "Validate or otherwise constrain DataBus registration names.",
        scopeLabel: "Scope:",
        scopeText:
          "A source implementation or successful launcher startup is not end-to-end verification or general cross-loader mod compatibility.",
        roadmapLink: "Development roadmap (TODO.md)",
        problemsLink: "Problems & solutions",
        architectureLink: "Proposed architecture (solve.md)",
        rustLink: "Rust DataBus source",
        agentLink: "Java Agent source",
        mobLink: "MonsterSync source",
        footerText: "Experimental research project · Rust × JVM · Minecraft 1.21.4",
        footerLink: "Development TODO →",
      },
      zh: {
        documentTitle: "项目进展 — JVM Mod Combiner",
        metaDescription: "JVM Mod Combiner 当前进度、实现状态与最新源码更新。",
        eyebrow: "项目状态",
        pageTitle: "项目进度与最新更新",
        pageIntro: "基于源码整理已验证成果、当前实现和下一步验证工作。",
        latestStatus: "最新更新 · 2026-11-10",
        latestTitle: "工作区新增敌对生物镜像链路",
        latestText:
          "当前 Java 源码新增 NeoForge 对 Fabric 怪物生成/更新消息的处理、实体镜像以及 ACK/重试流程。Rust DataBus 仍将受保护状态消息限制为已注册的 Fabric 名称。",
        branchLabel: "main 分支记录：",
        branchText:
          "当前 main 历史包括加载器检测与 DataBus 注册（f2171ad）、Minecraft 1.21.4 Agent/类路径工作（f146f27）以及文档更新（e212414）。此处描述的 Java 怪物镜像改动尚未提交到 main，仍是工作区修改。",
        verifyLabel: "验证状态：",
        verifyText:
          "这些 Java 改动目前仅有源码实现。需要从当前源码重新打包 Agent，并在 Fabric/NeoForge 环境中运行测试。怪物移除尚未实现，因此不能视为完整怪物同步。",
        doneStatus: "已验证基础",
        doneTitle: "已建立的能力",
        doneIntro: "此前验证的原型里程碑与最新的未验证源码工作分开记录。",
        done1: "Rust/JNI 启动器与 Java Agent 初始化",
        done2: "Fabric 与 NeoForge 启动实验，以及 Forge 实验性启动",
        done3: "Rust TCP DataBus 注册与 JSON 路由",
        done4: "Fabric 到 NeoForge 玩家状态与 RemotePlayer 概念验证",
        workStatus: "当前源码 · 待运行时测试",
        workTitle: "工作区实现",
        workIntro: "这些改动存在于当前 bus_test 源码中，但尚未进入 main 分支历史。",
        work1:
          "NeoForge 解析 monster_spawn/monster_update，创建或更新镜像，并在检测到世界变化时清理怪物镜像。",
        work2:
          "NeoForge 应用消息后发送 monster_ack；Fabric 跟踪带序列号的待确认状态，并在收到确认前重试。",
        work3:
          "现有 Rust 策略会拒绝非 Fabric 的玩家/怪物状态消息，但注册名由客户端自行声明，并非认证身份。",
        nextStatus: "下一步",
        nextTitle: "先打包和测试，再补齐生命周期",
        next1: "从当前源码重新构建/打包 Java Agent，并确认 JAR 包含最新代码。",
        next2: "运行时测试正常的玩家/怪物消息传递、未授权拒绝、不转发以及连接保持。",
        next3: "测试怪物 ACK/重试、世界切换、断开和重连恢复。",
        next4: "设计并发布可靠的 monster_remove 事件；采样渲染列表中缺失并不能证明实体已消失。",
        next5: "校验或以其他方式限制 DataBus 注册名称。",
        scopeLabel: "范围说明：",
        scopeText:
          "源码实现或成功启动加载器不等于端到端验证，也不代表通用跨加载器模组兼容。",
        roadmapLink: "开发路线图（TODO.md）",
        problemsLink: "问题与解决方案",
        architectureLink: "架构方案（solve.md）",
        rustLink: "Rust DataBus 源码",
        agentLink: "Java Agent 源码",
        mobLink: "MonsterSync 源码",
        footerText: "实验性研究项目 · Rust × JVM · Minecraft 1.21.4",
        footerLink: "开发待办 →",
      },
    },
    problems: {
      en: {
        documentTitle: "Problems & Solutions — JVM Mod Combiner",
        metaDescription:
          "Verified problems, implemented workarounds, and open engineering challenges in JVM Mod Combiner.",
        eyebrow: "ENGINEERING NOTES",
        pageTitle: "Problems & solutions",
        pageIntro:
          "A current, high-level view of what has been verified, what has a workaround, and what still needs implementation.",
        prototypeLabel: "Prototype status:",
        prototypeStatus:
          "Fabric and NeoForge startup, DataBus routing, and one-way player mirroring have reached verified prototype milestones. They do not establish general mod compatibility or complete gameplay synchronization.",
        p1Status: "Verified foundation",
        p1Title: "Starting isolated loader environments",
        p1Text:
          "The Rust launcher has started Fabric 1.21.4 and NeoForge 1.21.1 in tested configurations. Forge 1.21.4 has reached experimental startup, but full gameplay stability is a separate target. The current Rust NeoForge launch configuration targets NeoForge 21.4.161 / Minecraft 1.21.4; verify that configuration separately from the previously documented 1.21.1 run.",
        p1Approach:
          "<strong>Approach:</strong> Build loader-specific launch paths and resolve dependencies for each runtime instead of treating every loader as one interchangeable classpath.",
        p2Status: "Partly resolved",
        p2Title: "Dependency and classpath mismatches",
        p2Text:
          "Missing ASM/Mixin classes, game-provider discovery, LWJGL natives, and several tested launch-path issues have been addressed. A missing Netty class still requires verification against the exact runtime dependency graph.",
        p2Next:
          "<strong>Next:</strong> Complete manifest-based resolution, platform filtering, native handling, checksums, and conflict detection. Avoid broad library-directory scans.",
        p3Status: "Verified prototype",
        p3Title: "DataBus startup and basic routing",
        p3Text:
          "The Rust TCP DataBus registers named clients and routes JSON messages. Binding the listener synchronously before starting the accept thread resolved the observed startup race.",
        p3Limit:
          "<strong>Limit:</strong> Basic routing does not yet provide a fully versioned, validated protocol. The Rust source now rejects protected player and monster state types from non-Fabric registrations, but runtime tests remain open. Registration names are self-asserted and are not authenticated identities.",
        p4Status: "Source implemented · verify",
        p4Title: "One-way state policy and trusted identity",
        p4Text:
          "Fabric is the sole publisher for protected player/monster state types. Rust checks the connection's registered name and returns <code>state_authority_violation</code> without routing the rejected message or closing the connection.",
        p4Remaining:
          "<strong>Remaining:</strong> Runtime-test the behavior and constrain registration. The first message's registration name is client-supplied, so it is a routing label—not authenticated proof of which process connected. Rebuild/package the Java Agent from <code>bus_test/</code>.",
        p5Status: "Working-tree source · runtime pending",
        p5Title: "Hostile-mob synchronization",
        p5Text:
          "<code>bus_test/MonsterSync.java</code> samples visible, living hostile Fabric mobs on the game thread and emits <code>monster_spawn</code>/<code>monster_update</code>. Current working-tree <code>BusAgent.java</code> handles those messages in NeoForge, creates/updates mirrors, and uses sequence-based ACK/retry.",
        p5Incomplete:
          "<strong>Incomplete:</strong> These Java changes are not yet in the checked-in main history or runtime-verified. Missing-from-render iteration is not treated as removal, and <code>monster_remove</code> is not published.",
        p6Status: "Stability work remains",
        p6Title: "Mirror identity and recovery",
        p6Text:
          "RemotePlayer mirroring is a proof of concept. Initial timeout cleanup and mirror recreation have been observed, but stable identity, duplicate prevention, world changes, and reconnection recovery still need systematic testing.",
        p6Direction:
          "<strong>Solution direction:</strong> Add stable UUID mapping, explicit lifecycle events, fresh-state recovery, and smoothing while keeping each game's tick loop independent.",
        p7Status: "Long-term design",
        p7Title: "Cross-loader behavior and compatibility",
        p7Text:
          "Matching coordinates does not make different versions, registries, mod APIs, or game logic equivalent. Arbitrary bytecode merging and native-code interception are not reliable universal solutions.",
        p7Direction:
          "<strong>Solution direction:</strong> Define a normalized, versioned state protocol; use loader-specific adapters and explicit compatibility integrations; keep unsupported objects as visual proxies where appropriate.",
        p8Status: "Unresolved",
        p8Title: "Legacy Forge 1.16.5 startup",
        p8Text:
          "The native macOS installer path encountered a hash mismatch, and the older FML/ModLauncher launch pipeline was not brought to a working client state. Substituting newer launch components proved unreliable.",
        p8Stance:
          "<strong>Current stance:</strong> Keep this as a separate, version-specific investigation; document it as unsupported if the maintenance cost outweighs the value.",
        p9Status: "Not a current target",
        p9Title: "Cross-process rendering and window composition",
        p9Text:
          "Combining GPU resources across independent clients is platform-specific and adds driver, synchronization, and lifetime risks.",
        p9Direction:
          "<strong>Solution direction:</strong> Keep independent windows and prioritize reliable state synchronization. Consider platform-native composition only after core behavior is stable.",
        historyLink: "Detailed issue history (problems.md)",
        architectureLink: "Proposed architecture (solve.md)",
        todoLink: "See current TODO priorities →",
        footerText: "Experimental research project · Rust × JVM · Minecraft 1.21.4",
        footerLink: "Development TODO →",
      },
      zh: {
        documentTitle: "问题与解决方案 — JVM Mod Combiner",
        metaDescription: "JVM Mod Combiner 已验证的问题、临时方案与待解决的工程挑战。",
        eyebrow: "工程问题记录",
        pageTitle: "问题与解决方案",
        pageIntro: "汇总已验证的能力、已有的临时方案，以及仍待实现或验证的工作。",
        prototypeLabel: "原型状态：",
        prototypeStatus:
          "Fabric 与 NeoForge 启动、DataBus 路由和单向玩家镜像已达到原型验证里程碑。这并不代表通用模组兼容或完整的游戏状态同步。",
        p1Status: "基础能力已验证",
        p1Title: "启动隔离的模组加载器环境",
        p1Text:
          "Rust 启动器已在测试配置中启动 Fabric 1.21.4 与 NeoForge 1.21.1。Forge 1.21.4 已达到实验性启动阶段，但完整游戏稳定性仍需单独验证。当前 Rust 中的 NeoForge 启动配置目标为 NeoForge 21.4.161 / Minecraft 1.21.4，应与此前记录的 1.21.1 测试区分验证。",
        p1Approach:
          "<strong>处理方向：</strong> 为不同加载器构建各自的启动流程并解析对应依赖，不将它们视为可互换的单一类路径。",
        p2Status: "部分解决",
        p2Title: "依赖与类路径不匹配",
        p2Text:
          "缺少 ASM/Mixin 类、游戏提供程序发现、LWJGL 原生库以及若干启动路径问题已得到处理。缺少 Netty 类的问题仍需根据实际运行时依赖图验证。",
        p2Next:
          "<strong>后续：</strong> 完善基于版本清单的依赖解析、平台筛选、原生库处理、校验和冲突检测，避免扫描整个库目录。",
        p3Status: "原型已验证",
        p3Title: "DataBus 启动与基础路由",
        p3Text:
          "Rust TCP DataBus 支持客户端注册和 JSON 消息路由。在启动 accept 线程前同步绑定监听器，解决了观察到的启动竞态。",
        p3Limit:
          "<strong>限制：</strong> 当前尚无完整的版本化和校验协议。Rust 源码已拒绝非 Fabric 注册客户端发送受保护的玩家和怪物状态消息，但仍待运行时测试。注册名称由客户端自行声明，并非经过认证的身份。",
        p4Status: "源码已实现 · 待验证",
        p4Title: "单向状态策略与可信身份",
        p4Text:
          "Fabric 是受保护玩家/怪物状态消息的唯一发布方。Rust 根据连接注册名检查消息，并返回 <code>state_authority_violation</code>；拒绝时不转发消息，也不关闭连接。",
        p4Remaining:
          "<strong>待完成：</strong> 运行时测试并限制注册行为。首条注册消息中的名称由客户端提供，只是路由标签，不能证明连接进程的真实身份。需要从 <code>bus_test/</code> 重新构建并打包 Java Agent。",
        p5Status: "工作区源码 · 待运行时测试",
        p5Title: "敌对生物同步",
        p5Text:
          "<code>bus_test/MonsterSync.java</code> 在游戏线程按 UUID 采样可见且存活的 Fabric 敌对生物，并发送 <code>monster_spawn</code>/<code>monster_update</code>。当前工作区 <code>BusAgent.java</code> 已在 NeoForge 处理这些消息、创建/更新镜像，并使用序列号 ACK/重试。",
        p5Incomplete:
          "<strong>尚未完成：</strong> 这些 Java 改动尚未进入 main 历史，也未经过运行时验证。渲染列表中缺失不会被当作已移除，目前也不会发布 <code>monster_remove</code>。",
        p6Status: "仍需稳定化",
        p6Title: "镜像身份与恢复",
        p6Text:
          "RemotePlayer 镜像仍是概念验证。已观察到初始超时清理和镜像重建，但稳定身份、防重复、世界切换和重连恢复仍需系统测试。",
        p6Direction:
          "<strong>解决方向：</strong> 增加稳定 UUID 映射、明确的生命周期事件、新状态恢复和移动平滑，同时保持各游戏 tick 循环独立。",
        p7Status: "长期设计",
        p7Title: "跨加载器行为与兼容性",
        p7Text:
          "坐标相同不代表不同版本、注册表、模组 API 或游戏逻辑等价。任意字节码合并和原生代码拦截都不是可靠的通用方案。",
        p7Direction:
          "<strong>解决方向：</strong> 定义规范化且带版本的状态协议，使用加载器专属适配器和明确的兼容集成；必要时将不支持的对象表示为视觉代理。",
        p8Status: "尚未解决",
        p8Title: "旧版 Forge 1.16.5 启动",
        p8Text:
          "macOS 原生安装流程遇到哈希不匹配，旧版 FML/ModLauncher 启动链路也未能进入可用客户端。替换为较新的启动组件并不可靠。",
        p8Stance:
          "<strong>当前处理：</strong> 将其作为独立的版本专项调查；若维护成本高于价值，则记录为不支持。",
        p9Status: "当前非目标",
        p9Title: "跨进程渲染与窗口合成",
        p9Text:
          "在独立客户端间共享 GPU 资源具有平台差异，并增加驱动、同步和资源生命周期风险。",
        p9Direction:
          "<strong>解决方向：</strong> 保留独立窗口并优先保证状态同步可靠。核心行为稳定后，再考虑平台原生窗口合成。",
        historyLink: "详细问题记录（problems.md）",
        architectureLink: "架构方案（solve.md）",
        todoLink: "查看当前待办优先级 →",
        footerText: "实验性研究项目 · Rust × JVM · Minecraft 1.21.4",
        footerLink: "开发待办 →",
      },
    },
    todo: {
      en: {
        documentTitle: "Development TODO — JVM Mod Combiner",
        metaDescription:
          "JVM Mod Combiner development progress and upcoming engineering milestones.",
        eyebrow: "DEVELOPMENT TRACKER",
        pageTitle: "Progress & TODO",
        pageIntro:
          "Completed work and the next implementation steps, summarized from the project roadmap.",
        statusLabel: "Current status:",
        statusText:
          "The startup, Agent, DataBus, and initial player-mirror foundation is verified as a prototype. The Rust source already rejects protected player/monster state messages from non-Fabric registrations. Next: package the Java Agent and runtime-test the rejection path, registration identity, and reconnect behavior.",
        doneStatus: "Completed / verified",
        doneTitle: "Prototype foundation",
        doneIntro: "Verified achievements recorded in the project roadmap.",
        done1: "Rust/JNI launch and Java invocation",
        done2: "Fabric 1.21.4 and NeoForge 1.21.1 tested in isolated JVMs",
        done3: "Forge 1.21.4 experimental early-loading startup",
        done4: "Java Agent initialization and runtime class observation",
        done5: "TCP DataBus registration and JSON routing",
        done6: "Fabric player-state publishing and NeoForge RemotePlayer mirror proof of concept",
        done7: "Initial timeout cleanup and mirror recreation runtime verification",
        authorityStatus: "Implemented in source · verify",
        authorityTitle: "Validate one-way state authority",
        authorityIntro:
          "Fabric remains the sole authoritative publisher for player and monster state. The DataBus checks the name attached during client registration, rejects protected types from other names, and keeps the connection open. Registration names are currently self-asserted, not authenticated.",
        authority1: "Rust source rejects non-Fabric player_move and monster-state messages",
        authority2: "Validate allowed registration names and address client impersonation",
        authority3: "Rebuild/package Java Agent sources from bus_test/",
        authority4: "Runtime-test rejection, no forwarding, and sender connection continuity",
        authority5: "Verify Fabric delivery using the packaged Agent",
        authority6: "Test reconnect and fresh authoritative state transmission",
        mobStatus: "Working-tree source · runtime pending",
        mobTitle: "Complete hostile-mob synchronization",
        mobIntro:
          "bus_test/MonsterSync.java samples hostile Fabric mobs on the game thread at up to 5 Hz. Current working-tree BusAgent.java parses spawn/update messages, mirrors entities, and uses sequence-based ACK/retry; these changes still need packaging and runtime tests.",
        mob1: "Fabric-side mob discovery and state sampling by UUID",
        mob2: "Detect and publish mob removals",
        mob3: "NeoForge spawn/update parsing and mirror application (source implemented)",
        mob4: "Sequence-based monster ACK/retry path (source implemented)",
        mob5: "Runtime-test mirror lifecycle, world transitions, ACK/retries, and reconnects",
        laterStatus: "Following priorities",
        laterTitle: "Stabilize, then expand",
        later1: "Stable remote-player identity, collision-safe IDs, and duplicate prevention",
        later2: "World/dimension changes, disconnect cleanup, and mirror recovery",
        later3: "Versioned protocol, message validation, sequence numbers, and stale-state rejection",
        later4: "Heartbeat, reconnect resynchronization, rate limits, and message-size limits",
        later5: "Refactor launcher dependency resolution and child-process management",
        later6: "Expand beyond player coordinates through incremental entity/state adapters",
        later7: "Investigate Mixin compatibility, cross-platform support, and advanced mod integrations",
        scopeLabel: "Scope guard:",
        scopeText:
          "A successful loader launch or visual entity mirror does not mean arbitrary mods, inventories, combat, worlds, or game logic are synchronized.",
        roadmapLink: "Full development roadmap (TODO.md)",
        rustLink: "Rust DataBus source",
        agentLink: "Java Agent source",
        mobLink: "MonsterSync source",
        problemsLink: "Problems & solutions →",
        footerText: "Experimental research project · Rust × JVM · Minecraft 1.21.4",
        footerLink: "Problems & solutions →",
      },
      zh: {
        documentTitle: "开发待办 — JVM Mod Combiner",
        metaDescription: "JVM Mod Combiner 的开发进度与后续工程里程碑。",
        eyebrow: "开发进度跟踪",
        pageTitle: "进度与待办",
        pageIntro: "根据项目路线图整理的已完成工作和后续实现步骤。",
        statusLabel: "当前状态：",
        statusText:
          "启动器、Agent、DataBus 和初始玩家镜像基础已作为原型完成验证。Rust 源码已拒绝非 Fabric 注册客户端发送受保护的玩家/怪物状态消息。接下来需要打包 Java Agent，并运行时测试拒绝路径、注册身份和重连行为。",
        doneStatus: "已完成 / 已验证",
        doneTitle: "原型基础",
        doneIntro: "项目路线图中记录的已验证成果。",
        done1: "Rust/JNI 启动及 Java 方法调用",
        done2: "在独立 JVM 中测试 Fabric 1.21.4 与 NeoForge 1.21.1",
        done3: "Forge 1.21.4 实验性早期启动",
        done4: "Java Agent 初始化和运行时类观察",
        done5: "TCP DataBus 注册与 JSON 路由",
        done6: "Fabric 玩家状态发布与 NeoForge RemotePlayer 镜像概念验证",
        done7: "初始超时清理和镜像重建的运行时验证",
        authorityStatus: "源码已实现 · 待验证",
        authorityTitle: "验证单向状态权威",
        authorityIntro:
          "Fabric 仍是玩家和怪物状态的唯一权威发布方。DataBus 根据客户端注册时声明的名称检查消息，拒绝其他名称发送的受保护类型，同时保持连接。当前注册名由客户端自行声明，未经身份认证。",
        authority1: "Rust 源码拒绝非 Fabric 的 player_move 和怪物状态消息",
        authority2: "校验允许的注册名称并处理客户端冒充问题",
        authority3: "从 bus_test/ 重新构建并打包 Java Agent",
        authority4: "运行时测试拒绝、不转发以及发送端连接保持",
        authority5: "使用打包后的 Agent 验证 Fabric 消息送达",
        authority6: "测试重连和最新权威状态传输",
        mobStatus: "工作区源码 · 待运行时测试",
        mobTitle: "完成敌对生物同步",
        mobIntro:
          "bus_test/MonsterSync.java 在游戏线程以最高 5 Hz 采样 Fabric 敌对生物。当前工作区 BusAgent.java 已加入生成/更新消息解析、实体镜像及带序列号的 ACK/重试；仍需打包并运行时测试。",
        mob1: "Fabric 端按 UUID 发现并采样生物状态",
        mob2: "检测并发布生物移除事件",
        mob3: "NeoForge 生成/更新解析与镜像应用（源码已实现）",
        mob4: "带序列号的怪物 ACK/重试路径（源码已实现）",
        mob5: "运行时测试镜像生命周期、世界切换、ACK/重试和重连",
        laterStatus: "后续优先事项",
        laterTitle: "先稳定，再扩展",
        later1: "稳定远程玩家身份、防止实体 ID 冲突和重复实体",
        later2: "处理世界/维度切换、断开清理和镜像恢复",
        later3: "增加协议版本、消息校验、序列号和过期状态拒绝",
        later4: "增加心跳、重连同步、速率限制和消息大小限制",
        later5: "重构启动器依赖解析与子进程管理",
        later6: "通过渐进式实体/状态适配器扩展玩家坐标以外的同步",
        later7: "研究 Mixin 兼容性、跨平台支持和高级模组集成",
        scopeLabel: "范围说明：",
        scopeText:
          "成功启动加载器或显示实体镜像，不代表任意模组、物品栏、战斗、世界或游戏逻辑已经同步。",
        roadmapLink: "完整开发路线图（TODO.md）",
        rustLink: "Rust DataBus 源码",
        agentLink: "Java Agent 源码",
        mobLink: "MonsterSync 源码",
        problemsLink: "问题与解决方案 →",
        footerText: "实验性研究项目 · Rust × JVM · Minecraft 1.21.4",
        footerLink: "问题与解决方案 →",
      },
    },
  };

  const page = document.body.dataset.page;
  const languageSelect = document.getElementById("pageLanguage");
  const themeToggle = document.getElementById("pageTheme");
  const nav = document.getElementById("pageNav");
  const menuToggle = document.getElementById("pageMenu");

  function setLanguage(language) {
    const dict = { ...common[language], ...translations[page][language] };
    document.documentElement.lang = language === "zh" ? "zh-CN" : "en";
    document.title = dict.documentTitle;
    document
      .querySelector('meta[name="description"]')
      .setAttribute("content", dict.metaDescription);
    document.querySelectorAll("[data-i18n]").forEach((element) => {
      const value = dict[element.dataset.i18n];
      if (value !== undefined) element.innerHTML = value;
    });
    document.querySelectorAll("[data-i18n-aria]").forEach((element) => {
      const value = dict[element.dataset.i18nAria];
      if (value !== undefined) element.setAttribute("aria-label", value);
    });
    languageSelect.value = language;
    localStorage.setItem("jvm-combiner-language", language);
  }

  const savedLanguage = localStorage.getItem("jvm-combiner-language");
  setLanguage(savedLanguage === "en" ? "en" : "zh");
  languageSelect.addEventListener("change", () => {
    setLanguage(languageSelect.value);
  });

  function setTheme(theme) {
    const light = theme === "light";
    document.body.classList.toggle("light", light);
    themeToggle.textContent = light ? common[languageSelect.value].dark : common[languageSelect.value].light;
    localStorage.setItem("jvm-combiner-theme", light ? "light" : "dark");
  }

  setTheme(localStorage.getItem("jvm-combiner-theme") || "dark");
  themeToggle.addEventListener("click", () => {
    setTheme(document.body.classList.contains("light") ? "dark" : "light");
  });
  languageSelect.addEventListener("change", () => setTheme(document.body.classList.contains("light") ? "light" : "dark"));

  menuToggle.addEventListener("click", () => {
    const open = nav.classList.toggle("is-open");
    menuToggle.setAttribute("aria-expanded", String(open));
  });
  nav.querySelectorAll(".nav-links a").forEach((link) =>
    link.addEventListener("click", () => {
      nav.classList.remove("is-open");
      menuToggle.setAttribute("aria-expanded", "false");
    }),
  );
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && nav.classList.contains("is-open")) {
      nav.classList.remove("is-open");
      menuToggle.setAttribute("aria-expanded", "false");
      menuToggle.focus();
    }
  });
})();
