(() => {
  "use strict";

  const translations = {
    zh: {
      skipToContent: "跳转到主要内容",
      navFeatures: "特性",
      navArchitecture: "架构",
      navDemo: "终端",
      navRoadmap: "进度",
      navProgress: "最新进展",
      navProblems: "问题与方案",
      navTodo: "待办",
      navStart: "快速开始",
      navGithub: "GitHub ↗",
      viewSource: "在 GitHub 上查看 ↗",
      heroBadge: "实验性原型 · 单向玩家镜像概念验证",
      heroTitle:
        '连接不同的<br><span class="gradient-text">Minecraft 模组生态。</span>',
      heroDesc:
        "JVM Mod Combiner 是一个基于 Rust 和 Java 的实验性原型。已验证 Fabric 与 NeoForge 在独立 JVM 中运行，并完成单向玩家状态同步概念验证；近期还加入了只读 macOS 窗口发现原型。",
      explore: "探索架构",
      viewDemo: "查看启动演示",
      metricKernel: "启动内核",
      metricBridge: "Java 虚拟机桥接",
      metricVersion: "目标游戏版本",
      metricGoal: "独立 JVM 并行运行",
      flowTitle: "一次原型验证，经过哪些环节？",
      flowDesc: "从 Rust 启动独立游戏进程，到通过本地 DataBus 传递单向玩家状态。",
      flowVerified: "已验证",
      flowPoc: "概念验证",
      flowLaunchTitle: "Rust 启动器",
      flowLaunchDesc: "解析运行环境并启动独立 JVM，加载对应的 Minecraft 模组加载器。",
      flowAgentTitle: "Java Agent",
      flowAgentDesc: "在目标 JVM 中初始化 Agent，读取游戏实例并提取 Fabric 玩家状态。",
      flowBusTitle: "本地 TCP DataBus",
      flowBusDesc: "注册 Fabric 与 NeoForge 客户端，并在本机路由 JSON 消息。",
      flowMirrorTitle: "单向玩家镜像",
      flowMirrorDesc: "Fabric 玩家状态已能驱动 NeoForge RemotePlayer 镜像；生命周期稳定性仍在完善。",
      flowFootnote: "这是本地原型的已验证数据流，不代表任意模组或服务器间的通用兼容。",
      featuresTitle: "为 JVM 互操作而设计",
      featuresDesc: "从启动流程到运行时通信，逐层构建跨模组生态的技术基础。",
      searchPlaceholder: "搜索功能 / Search features...",
      featureCount: "{visible} / {total} 项功能",
      verified: "已验证启动",
      agentVerified: "初始化已验证",
      nextMilestone: "下一里程碑",
      routingVerified: "基础路由已验证",
      playerStateVerified: "状态读取与发布已验证",
      pocNeedsStability: "概念验证完成 · 持续稳定中",
      windowPrototypeStatus: "原型已实现 · 真实环境验证待完成",
      diagnosticsVerified: "基础诊断已验证",
      noResults: "没有找到匹配的功能。",
      f1Title: "Rust 启动内核",
      f1Desc: "解析版本依赖、组织 JVM 类路径，并通过 JNI 启动目标 Java 环境。",
      f2Title: "Java Agent",
      f2Desc: "使用 premain 和 Instrumentation 探索运行时类结构及诊断能力。",
      f3Title: "Fabric 玩家状态读取",
      f3Desc: "通过反射读取玩家状态并发布到 Rust DataBus；bus_test 中还包含敌对生物采样及初始/变化消息的早期原型。",
      f4Title: "Rust TCP DataBus",
      f4Desc: "Rust 源码按注册连接名限制玩家及怪物状态消息仅由 Fabric 发布；注册名仍由客户端自报，且拒绝行为待运行时验证。",
      f5Title: "双端实体镜像",
      f5Desc:
        "Fabric 到 NeoForge 的 RemotePlayer 镜像已完成概念验证。已提交源码包含 NeoForge 怪物接收与 ACK/重试流程，但仍需打包和运行时测试；怪物移除尚未实现。",
      f6Title: "运行时结构诊断",
      f6Desc: "输出 Minecraft 实例字段、类型和值，辅助分析混淆环境。",
      f7Title: "窗口注册与观察",
      f7Desc:
        "Java Bridge 注册 Loader JVM，Rust 按 PID 关联窗口；macOS 窗口发现和辅助功能检查为只读，不会操作窗口。",
      architectureTitle: "系统架构",
      architectureDesc: "选择一个模块，查看它在整个启动和同步流程中的职责。",
      archRust: "依赖解析 · JNI",
      archAgent: "premain · Instrumentation",
      archGame: "游戏实例 · 玩家状态",
      detailRust: "负责解析版本元数据、组织类路径，并通过 JNI 创建和启动 JVM。",
      detailAgent:
        "通过 premain 进入目标 JVM，注册 Agent 并启动诊断或监控任务。",
      detailGame:
        "Fabric 玩家状态提取和单向镜像概念验证已通过；Rust 源码已有单向发布策略，但注册名验证、运行时测试及实体生命周期仍需完善。",
      detailBus:
        "Rust TCP DataBus 已实现连接注册和 JSON 路由，并在源码中拒绝非 Fabric 的玩家/怪物状态消息。注册名由客户端自报；认证、运行时测试和协议校验仍待完成。",
      detailNote: "架构图描述的是目标系统分层，不代表所有链路都已经完成。",
      demoTitle: "原型验证流程回放",
      demoDesc:
        "根据已验证的启动、DataBus 路由和单向玩家镜像阶段整理的示例日志，并非实时连接游戏进程。",
      play: "播放",
      pause: "暂停",
      reset: "重置",
      waiting: "等待开始",
      playing: "正在回放",
      paused: "已暂停",
      finished: "回放完成",
      roadmapTitle: "开发路线图",
      roadmapDesc: "用明确的里程碑记录工程进展，不把计划当作已交付功能。",
      r1Title: "Rust/JVM 启动基础",
      r1Desc: "验证 Rust/JNI 启动基础、Fabric 与 NeoForge 启动，以及 Forge 实验性启动。",
      r2Title: "Agent 初始化与诊断",
      r2Desc: "premain、等待游戏实例和基础结构扫描。",
      r3Title: "DataBus 与玩家镜像概念验证",
      r3Desc: "验证 Rust TCP 消息路由、Fabric 玩家状态发布和 NeoForge RemotePlayer 镜像。",
      r4Title: "单向状态权威与发送方授权",
      r4Desc: "Rust 源码已有玩家/怪物状态发送方策略；重打包 Agent、约束自报注册名，并运行时验证拒绝不转发且连接不断开。",
      r4Status: "源码已实现 · 待验证",
      r5Title: "同步稳定性与启动器重构",
      r5Desc: "完善实体生命周期、重连和协议校验，并继续重构 Rust 启动器与依赖处理。",
      completed: "已完成",
      planned: "后续重点",
      stackTitle: "技术栈",
      startTitle: "从本地环境开始",
      startDesc:
        "以下命令以项目根目录为起点。请根据实际仓库地址和本地环境调整。",
      copy: "复制",
      copied: "已复制",
      copyFailed: "复制失败，请手动复制",
      footerText: "独立技术研究项目 · Rust × JVM · Minecraft 1.21.4",
      backTop: "返回顶部 ↑",
      donate: "纯为爱发电，不接受任何形式的资助或捐赠。",
    },
    en: {
      skipToContent: "Skip to main content",
      navFeatures: "Features",
      navArchitecture: "Architecture",
      navDemo: "Terminal",
      navRoadmap: "Roadmap",
      navProgress: "Progress",
      navProblems: "Problems & solutions",
      navTodo: "TODO",
      navStart: "Get started",
      navGithub: "GitHub ↗",
      viewSource: "View on GitHub ↗",
      heroBadge: "Experimental prototype · One-way player-mirroring PoC",
      heroTitle:
        'Connecting different<br><span class="gradient-text">Minecraft mod ecosystems.</span>',
      heroDesc:
        "JVM Mod Combiner is an experimental Rust and Java prototype. Fabric and NeoForge have been verified in separate JVMs, with a proof of concept for one-way player-state synchronization; a read-only macOS window-discovery prototype has also been added.",
      explore: "Explore architecture",
      viewDemo: "View launch demo",
      metricKernel: "Launcher kernel",
      metricBridge: "Java VM bridge",
      metricVersion: "Target game version",
      metricGoal: "Separate JVMs running together",
      flowTitle: "What happens in a prototype run?",
      flowDesc:
        "From launching isolated game processes with Rust to routing one-way player state through a local DataBus.",
      flowVerified: "Verified",
      flowPoc: "Proof of concept",
      flowLaunchTitle: "Rust launcher",
      flowLaunchDesc:
        "Prepares the runtime and starts an isolated JVM with its Minecraft mod loader.",
      flowAgentTitle: "Java Agent",
      flowAgentDesc:
        "Initializes inside the target JVM, finds the game instance, and extracts Fabric player state.",
      flowBusTitle: "Local TCP DataBus",
      flowBusDesc:
        "Registers Fabric and NeoForge clients and routes JSON messages locally.",
      flowMirrorTitle: "One-way player mirror",
      flowMirrorDesc:
        "Fabric player state drives a NeoForge RemotePlayer mirror; lifecycle stability is still being improved.",
      flowFootnote:
        "This is a verified data flow in a local prototype, not general compatibility between arbitrary mods or servers.",
      featuresTitle: "Designed for JVM interoperability",
      featuresDesc:
        "Building the technical foundations for cross-loader interoperability, from launch to runtime communication.",
      searchPlaceholder: "Search features...",
      featureCount: "{visible} / {total} features",
      verified: "Launch verified",
      agentVerified: "Initialization verified",
      nextMilestone: "Next milestone",
      routingVerified: "Basic routing verified",
      playerStateVerified: "State extraction and publishing verified",
      pocNeedsStability: "PoC verified · Stabilization ongoing",
      windowPrototypeStatus: "Prototype implemented · live validation pending",
      diagnosticsVerified: "Basic diagnostics verified",
      noResults: "No matching features found.",
      f1Title: "Rust launcher kernel",
      f1Desc:
        "Resolves version dependencies, assembles JVM classpaths, and launches the target Java environment through JNI.",
      f2Title: "Java Agent",
      f2Desc:
        "Uses premain and Instrumentation to inspect runtime class structures and provide diagnostics.",
      f3Title: "Fabric player-state extraction",
      f3Desc:
        "Reflection reads player state and publishes it to the Rust DataBus; bus_test also contains an early hostile-mob sampling and spawn/update publisher prototype.",
      f4Title: "Rust TCP DataBus",
      f4Desc:
        "Rust source restricts player and monster state messages to the registered Fabric name; registration is self-asserted and rejection still needs runtime verification.",
      f5Title: "Cross-instance entity mirror",
      f5Desc:
        "The Fabric-to-NeoForge RemotePlayer mirror is a verified proof of concept. Committed Java sources add a NeoForge monster receiver and ACK/retry path, but these changes need packaging and runtime tests; monster removal is not implemented.",
      f6Title: "Runtime diagnostics",
      f6Desc:
        "Inspects Minecraft instance fields, types, and values to help analyze obfuscated environments.",
      f7Title: "Window registration and observation",
      f7Desc:
        "The Java bridge registers loader JVMs and Rust associates windows by PID; macOS discovery and Accessibility inspection are read-only and do not control windows.",
      architectureTitle: "System architecture",
      architectureDesc:
        "Select a module to inspect its role in the launch and synchronization pipeline.",
      archRust: "Dependency resolution · JNI",
      archAgent: "premain · Instrumentation",
      archGame: "Game instance · Player state",
      detailRust:
        "Resolves version metadata, assembles classpaths, and creates and launches the JVM through JNI.",
      detailAgent:
        "Enters the target JVM through premain, registers the agent, and starts diagnostic or monitoring tasks.",
      detailGame:
        "Fabric player-state extraction and a one-way mirroring proof of concept are verified. The Rust source has a one-way publisher policy; registration validation, runtime tests, and entity lifecycle still need work.",
      detailBus:
        "The Rust TCP DataBus implements client registration, JSON routing, and source-level filtering of protected state messages. Names are self-asserted; authentication, runtime verification, and protocol validation remain.",
      detailNote:
        "This diagram describes the target architecture; it does not imply that every component is complete.",
      demoTitle: "Prototype verification replay",
      demoDesc:
        "A representative sequence based on verified launch, DataBus routing, and one-way player-mirroring milestones. It is not connected to a live game process.",
      play: "Play",
      pause: "Pause",
      reset: "Reset",
      waiting: "Ready",
      playing: "Playing",
      paused: "Paused",
      finished: "Replay complete",
      roadmapTitle: "Development roadmap",
      roadmapDesc:
        "Milestones track engineering progress without presenting planned work as shipped functionality.",
      r1Title: "Rust/JVM launch foundation",
      r1Desc:
        "Verified Rust/JNI foundations, Fabric and NeoForge launches, and an experimental Forge boot.",
      r2Title: "Agent initialization and diagnostics",
      r2Desc:
        "premain, waiting for the game instance, and basic structure inspection.",
      r3Title: "DataBus and player-mirroring proof of concept",
      r3Desc:
        "Verified Rust TCP routing, Fabric player-state publishing, and a NeoForge RemotePlayer mirror.",
      r4Title: "One-way authority and sender authorization",
      r4Desc:
        "The Rust source has a player/monster state sender policy. Repackage the Agent, constrain self-asserted registration names, and runtime-test rejection, no forwarding, and connection continuity.",
      r4Status: "Implemented in source · verify",
      r5Title: "Synchronization stability and launcher refactoring",
      r5Desc:
        "Improve entity lifecycle, reconnection, and protocol validation, then continue refactoring Rust launch and dependency handling.",
      completed: "Complete",
      planned: "Next focus",
      stackTitle: "Technology stack",
      startTitle: "Start from your local environment",
      startDesc:
        "These commands assume the project root directory. Adjust them for your repository and environment.",
      copy: "Copy",
      copied: "Copied",
      copyFailed: "Copy failed; please copy manually",
      footerText:
        "Independent research project · Rust × JVM · Minecraft 1.21.4",
      backTop: "Back to top ↑",
      donate: "This project is purely for love of the craft; no funding or donations are accepted.",
    },
  };

  const language = document.getElementById("language");
  function setLanguage(lang) {
    const dict = translations[lang] || translations.zh;
    document.documentElement.lang =
      lang === "zh" ? "zh-CN" : lang === "ja" ? "ja" : "en";
    document.querySelectorAll("[data-i18n]").forEach((el) => {
      const key = el.dataset.i18n;
      if (dict[key] !== undefined) el.innerHTML = dict[key];
    });
    document.querySelectorAll("[data-i18n-placeholder]").forEach((el) => {
      const key = el.dataset.i18nPlaceholder;
      if (dict[key] !== undefined) el.placeholder = dict[key];
    });
    localStorage.setItem("jvm-combiner-language", lang);
  }
  language.value = localStorage.getItem("jvm-combiner-language") || "zh";
  language.addEventListener("change", () => {
    setLanguage(language.value);
    filterFeatures();
    renderSelectedNode();
  });
  setLanguage(language.value);

  const themeToggle = document.getElementById("themeToggle");
  function setTheme(theme) {
    document.body.classList.toggle("light", theme === "light");
    themeToggle.textContent = theme === "light" ? "DARK" : "LIGHT";
    localStorage.setItem("jvm-combiner-theme", theme);
  }
  setTheme(localStorage.getItem("jvm-combiner-theme") || "dark");
  themeToggle.addEventListener("click", () =>
    setTheme(document.body.classList.contains("light") ? "dark" : "light"),
  );

  const nav = document.querySelector(".nav");
  const menuToggle = document.getElementById("menuToggle");
  menuToggle.addEventListener("click", () => {
    const open = nav.classList.toggle("is-open");
    menuToggle.setAttribute("aria-expanded", String(open));
  });
  document.querySelectorAll(".nav-links a").forEach((a) =>
    a.addEventListener("click", () => {
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

  const featureSearch = document.getElementById("featureSearch");
  const featureCards = [...document.querySelectorAll(".feature-card")];
  const featureCount = document.getElementById("featureCount");
  function filterFeatures() {
    const query = featureSearch.value.trim().toLowerCase();
    featureCards.forEach((card) => {
      card.hidden = !(card.innerText + " " + card.dataset.search)
        .toLowerCase()
        .includes(query);
    });
    document.getElementById("emptyState").hidden = featureCards.some(
      (card) => !card.hidden,
    );
    const visible = featureCards.filter((card) => !card.hidden).length;
    featureCount.textContent = dict()
      .featureCount.replace("{visible}", visible)
      .replace("{total}", featureCards.length);
  }
  featureSearch.addEventListener("input", filterFeatures);
  filterFeatures();

  const nodeDetails = {
    rust: {
      eyebrow: "01 / KERNEL",
      title: "Rust Kernel",
      text: "detailRust",
      tags: ["Rust", "JNI", "Classpath"],
    },
    agent: {
      eyebrow: "02 / AGENT",
      title: "BusAgent",
      text: "detailAgent",
      tags: ["Java Agent", "premain", "Instrumentation"],
    },
    game: {
      eyebrow: "03 / GAME",
      title: "Fabric / NeoForge",
      text: "detailGame",
      tags: ["Minecraft", "Reflection", "Player"],
    },
    bus: {
      eyebrow: "04 / TRANSPORT",
      title: "TCP Bus",
      text: "detailBus",
      tags: ["TCP", "JSON", "127.0.0.1"],
    },
  };
  let selectedNode = "rust";
  function renderSelectedNode() {
    const detail = nodeDetails[selectedNode];
    if (!detail) return;
    document.querySelectorAll(".arch-node").forEach((node) => {
      const active = node.dataset.node === selectedNode;
      node.classList.toggle("active", active);
      node.setAttribute("aria-pressed", String(active));
    });
    document.getElementById("detailEyebrow").textContent = detail.eyebrow;
    document.getElementById("detailTitle").textContent = detail.title;
    document.getElementById("detailText").textContent = dict()[detail.text];
    document.getElementById("detailTags").replaceChildren(
      ...detail.tags.map((tag) => {
        const span = document.createElement("span");
        span.textContent = tag;
        return span;
      }),
    );
  }
  document.querySelectorAll(".arch-node").forEach((button) =>
    button.addEventListener("click", () => {
      selectedNode = button.dataset.node;
      renderSelectedNode();
    }),
  );
  renderSelectedNode();

  const replayLines = [
    ["[Replay] Cross-sandbox prototype milestones", "info"],
    ["[BusAgent:fabric] premain", "info"],
    ["[Rust DataBus] Fabric and NeoForge clients registered", "info"],
    ["[Rust DataBus] JSON hello / ack routing verified", "ok"],
    ["[Fabric] Player state routed to NeoForge", "ok"],
    ["[NeoForge] RemotePlayer mirror updated", "ok"],
    ["[Verified] Initial timeout removal and mirror recreation", "ok"],
    ["[Rust source] Protected state types restricted to Fabric registration", "ok"],
    ["[Next] Runtime policy tests and registration validation", "warn"],
  ];
  const replay = document.getElementById("replay");
  const replayHint = document.getElementById("replayHint");
  let replayIndex = 0,
    replayTimer = null,
    replayState = "idle";
  function dict() {
    return translations[language.value] || translations.zh;
  }
  function updateReplayHint() {
    const d = dict();
    replayHint.textContent =
      replayState === "playing"
        ? d.playing
        : replayState === "paused"
          ? d.paused
          : replayState === "finished"
            ? d.finished
            : d.waiting;
  }
  function stopReplayTimer() {
    if (replayTimer) {
      clearTimeout(replayTimer);
      replayTimer = null;
    }
  }
  function appendReplayLine() {
    if (replayIndex >= replayLines.length) {
      replayState = "finished";
      updateReplayHint();
      return;
    }
    const [text, cls] = replayLines[replayIndex++];
    const span = document.createElement("span");
    if (cls) span.className = cls;
    span.textContent = text + "\n";
    replay.appendChild(span);
    replay.scrollTop = replay.scrollHeight;
    if (replayIndex >= replayLines.length) {
      replayState = "finished";
      updateReplayHint();
      return;
    }
    replayTimer = setTimeout(appendReplayLine, 480);
  }
  function playReplay() {
    if (replayState === "finished") {
      replayIndex = 0;
      replay.textContent = "";
    }
    if (replayState === "playing") return;
    replayState = "playing";
    updateReplayHint();
    stopReplayTimer();
    appendReplayLine();
  }
  function pauseReplay() {
    if (replayState !== "playing") return;
    stopReplayTimer();
    replayState = "paused";
    updateReplayHint();
  }
  function resetReplay() {
    stopReplayTimer();
    replayIndex = 0;
    replay.textContent = "";
    replayState = "idle";
    updateReplayHint();
  }
  document.getElementById("replayBtn").addEventListener("click", playReplay);
  document.getElementById("pauseBtn").addEventListener("click", pauseReplay);
  document.getElementById("resetBtn").addEventListener("click", resetReplay);

  // Replay state is initialized now; safely update the initial status.
  updateReplayHint();

  const toast = document.getElementById("toast");
  let toastTimer;
  function showToast(message) {
    toast.textContent = message;
    toast.classList.add("show");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("show"), 1800);
  }
  document.querySelectorAll(".copy-btn").forEach((button) =>
    button.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(button.dataset.copy);
        showToast(dict().copied);
      } catch {
        showToast(dict().copyFailed);
      }
    }),
  );

  // Small hero terminal preview. These are representative log excerpts, not a live process.
  const terminalLines = [
    ["[Rust] JVM Combiner prototype", "info"],
    ["[Rust] Fabric and NeoForge run in separate JVMs", "info"],
    ["[BusAgent:fabric] premain", "info"],
    ["[Rust DataBus] Client registration and JSON routing verified", "ok"],
    ["[Fabric -> NeoForge] Player mirror proof of concept", "ok"],
    ["[Next] Enforce receiver-only player-state authority", "warn"],
  ];
  const terminal = document.getElementById("terminal");
  terminalLines.forEach(([line, cls]) => {
    const span = document.createElement("span");
    if (cls) span.className = cls;
    span.textContent = line + "\n";
    terminal.appendChild(span);
  });

  if (
    "IntersectionObserver" in window &&
    !matchMedia("(prefers-reduced-motion: reduce)").matches
  ) {
    const observer = new IntersectionObserver(
      (entries) =>
        entries.forEach((entry) => {
          if (entry.isIntersecting) {
            entry.target.classList.add("visible");
            observer.unobserve(entry.target);
          }
        }),
      { threshold: 0.12 },
    );
    document
      .querySelectorAll(
        ".feature-card,.flow-step,.roadmap-item,.stack-grid div,.section-heading",
      )
      .forEach((el) => {
        el.style.opacity = "0";
        el.style.transform = "translateY(18px)";
        el.style.transition = "opacity .6s ease,transform .6s ease";
        observer.observe(el);
      });
    const style = document.createElement("style");
    style.textContent =
      ".visible{opacity:1!important;transform:translateY(0)!important}";
    document.head.appendChild(style);
  }
})();
