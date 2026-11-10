(() => {
  "use strict";

  const owner = "hchunhei81-cyber";
  const repository = "jvm-mod-combiner";
  const apiUrl = `https://api.github.com/repos/${owner}/${repository}/commits`;
  const commitUrl = `https://github.com/${owner}/${repository}/commit/`;
  const pageSize = 100;
  const translations = {
    en: {
      historyEyebrow: "MAIN BRANCH · COMPLETE HISTORY",
      historyTitle: "Commit timeline",
      historyIntro:
        "Browse every commit from the first main commit to the latest. Select an entry to inspect its message and open it on GitHub.",
      loading: "Loading main history…",
      refreshing: "Refreshing from GitHub…",
      refresh: "Refresh GitHub",
      live: "Live main history · {count} commits",
      snapshot: "Saved main snapshot · {count} commits",
      fallback: "Showing saved history · GitHub refresh failed: {error}",
      commitCount: "Commits",
      firstCommit: "First commit",
      latestCommit: "Latest commit",
      searchLabel: "Search commits",
      searchPlaceholder: "Message, author, or SHA",
      yearLabel: "Year",
      allYears: "All years",
      jumpFirst: "First commit",
      jumpLatest: "Latest commit",
      rangeLabel: "Navigate the filtered timeline",
      oldest: "Oldest",
      newest: "Newest",
      selected: "Selected commit",
      openCommit: "Open commit ↗",
      empty: "No matching commits.",
      footnote:
        "History is limited to main. GitHub is refreshed automatically when this page opens; the saved snapshot is used if the API is unavailable.",
      categories: {
        feat: "Feature",
        fix: "Fix",
        docs: "Docs",
        chore: "Chore",
        refactor: "Refactor",
        test: "Test",
        milestone: "Milestone",
        other: "Commit",
      },
      noHistory: "No commit history is available.",
      apiError: "GitHub API request failed ({status}).",
      apiShape: "GitHub returned an unexpected commit list.",
      snapshotError: "Could not load the saved commit history.",
    },
    zh: {
      historyEyebrow: "MAIN 分支 · 完整提交历史",
      historyTitle: "提交时间线",
      historyIntro:
        "从 main 的首次提交浏览到最新提交。选择条目可查看提交信息并在 GitHub 上打开。",
      loading: "正在加载 main 历史…",
      refreshing: "正在从 GitHub 更新…",
      refresh: "刷新 GitHub",
      live: "GitHub main 实时历史 · {count} 次提交",
      snapshot: "本地 main 历史快照 · {count} 次提交",
      fallback: "正在显示保存的历史 · GitHub 更新失败：{error}",
      commitCount: "提交数",
      firstCommit: "首次提交",
      latestCommit: "最新提交",
      searchLabel: "搜索提交",
      searchPlaceholder: "提交说明、作者或 SHA",
      yearLabel: "年份",
      allYears: "所有年份",
      jumpFirst: "首次提交",
      jumpLatest: "最新提交",
      rangeLabel: "浏览筛选后的时间线",
      oldest: "最早",
      newest: "最新",
      selected: "当前选中",
      openCommit: "在 GitHub 查看 ↗",
      empty: "没有匹配的提交。",
      footnote:
        "时间线仅显示 main 分支。打开页面时会自动从 GitHub 更新；API 不可用时使用随仓库保存的历史快照。",
      categories: {
        feat: "功能",
        fix: "修复",
        docs: "文档",
        chore: "维护",
        refactor: "重构",
        test: "测试",
        milestone: "里程碑",
        other: "提交",
      },
      noHistory: "没有可用的提交历史。",
      apiError: "GitHub API 请求失败（{status}）。",
      apiShape: "GitHub 返回的提交列表格式不正确。",
      snapshotError: "无法加载保存的提交历史。",
    },
  };

  const elements = {
    language: document.getElementById("pageLanguage"),
    source: document.getElementById("historySource"),
    refresh: document.getElementById("historyRefresh"),
    count: document.getElementById("historyCount"),
    first: document.getElementById("historyFirst"),
    latest: document.getElementById("historyLatest"),
    search: document.getElementById("historySearch"),
    year: document.getElementById("historyYear"),
    range: document.getElementById("historyRange"),
    rangeOutput: document.getElementById("historyRangeOutput"),
    jumpFirst: document.getElementById("historyJumpFirst"),
    jumpLatest: document.getElementById("historyJumpLatest"),
    focusTitle: document.getElementById("historyFocusTitle"),
    focusMeta: document.getElementById("historyFocusMeta"),
    focusLink: document.getElementById("historyFocusLink"),
    empty: document.getElementById("historyEmpty"),
    timeline: document.getElementById("commitTimeline"),
  };

  const state = {
    commits: [],
    filtered: [],
    selectedSha: null,
    locale: "zh",
    sourceStatus: "snapshot",
    sourceError: "",
  };

  const language = () =>
    translations[elements.language?.value] ? elements.language.value : "zh";

  function text(key) {
    return translations[state.locale][key];
  }

  function interpolate(value, fields) {
    return value.replace(/\{(\w+)\}/g, (_, key) => fields[key] ?? "");
  }

  function escapeHtml(value) {
    return String(value).replace(/[&<>"']/g, (character) => {
      const entities = {
        "&": "&amp;",
        "<": "&lt;",
        ">": "&gt;",
        '"': "&quot;",
        "'": "&#39;",
      };
      return entities[character];
    });
  }

  function normalizeCommit(item) {
    const commit = item.commit || {};
    const message = item.message || commit.message || "";
    const date =
      item.date || commit.author?.date || commit.committer?.date || "";
    const sha = item.sha || "";
    const author =
      typeof item.author === "string"
        ? item.author
        : item.author?.login || commit.author?.name || "Unknown";

    return {
      sha,
      shortSha: sha.slice(0, 7),
      date,
      message,
      subject: message.split(/\r?\n/, 1)[0] || "(no commit message)",
      author,
      url: item.html_url || item.url || `${commitUrl}${sha}`,
    };
  }

  function categoryFor(message) {
    const lower = message.toLowerCase();
    const prefix = lower.match(/^(feat|fix|docs|chore|refactor|test)(?:\([^)]*\))?:/);
    if (prefix) return prefix[1];
    if (lower.includes("milestone")) return "milestone";
    return "other";
  }

  function formatDate(date, includeTime = false) {
    const value = new Date(date);
    if (Number.isNaN(value.getTime())) return date || "—";
    const options = includeTime
      ? { dateStyle: "medium", timeStyle: "short" }
      : { year: "numeric", month: "short", day: "numeric" };
    return new Intl.DateTimeFormat(
      state.locale === "zh" ? "zh-CN" : "en",
      options,
    ).format(value);
  }

  function setSource(message, status) {
    elements.source.textContent = message;
    elements.source.classList.toggle("is-live", status === "live");
    elements.source.classList.toggle("is-fallback", status === "fallback");
  }

  function applyLanguage() {
    state.locale = language();
    document.querySelectorAll("[data-timeline-i18n]").forEach((element) => {
      const value = text(element.dataset.timelineI18n);
      if (value !== undefined) element.textContent = value;
    });
    document
      .querySelectorAll("[data-timeline-i18n-placeholder]")
      .forEach((element) => {
        const value = text(element.dataset.timelineI18nPlaceholder);
        if (value !== undefined) element.placeholder = value;
      });
    elements.timeline.setAttribute(
      "aria-label",
      state.locale === "zh" ? "main 分支提交历史" : "Main branch commit history",
    );
    if (elements.year.value === "all" || !elements.year.value) {
      elements.year.options[0].textContent = text("allYears");
    }
    updateSource(state.sourceStatus, state.sourceError);
    render();
  }

  function buildYearOptions() {
    const current = elements.year.value || "all";
    const years = [
      ...new Set(
        state.commits
          .map((commit) => commit.date.slice(0, 4))
          .filter((year) => /^\d{4}$/.test(year)),
      ),
    ].sort();

    elements.year.replaceChildren();
    const allOption = document.createElement("option");
    allOption.value = "all";
    allOption.textContent = text("allYears");
    elements.year.append(allOption);

    for (const year of years) {
      const option = document.createElement("option");
      option.value = year;
      option.textContent = year;
      elements.year.append(option);
    }

    elements.year.value = years.includes(current) ? current : "all";
  }

  function filteredCommits() {
    const query = elements.search.value.trim().toLowerCase();
    const selectedYear = elements.year.value;

    return state.commits.filter((commit) => {
      const matchesYear =
        selectedYear === "all" || commit.date.startsWith(selectedYear);
      const searchable =
        `${commit.message} ${commit.author} ${commit.sha}`.toLowerCase();
      return matchesYear && (!query || searchable.includes(query));
    });
  }

  function renderStats() {
    const commits = state.commits;
    elements.count.textContent = String(commits.length);
    elements.first.textContent = commits.length
      ? formatDate(commits[0].date)
      : "—";
    elements.latest.textContent = commits.length
      ? formatDate(commits[commits.length - 1].date)
      : "—";
  }

  function renderTimeline() {
    state.filtered = filteredCommits();
    const commits = state.filtered;
    elements.empty.hidden = commits.length !== 0;
    elements.timeline.replaceChildren();
    elements.range.max = String(Math.max(commits.length - 1, 0));
    elements.range.disabled = commits.length === 0;

    if (!commits.length) {
      state.selectedSha = null;
      updateFocus();
      return;
    }

    let activeYear = "";
    for (let index = 0; index < commits.length; index += 1) {
      const commit = commits[index];
      const year = commit.date.slice(0, 4);

      if (year !== activeYear) {
        activeYear = year;
        const heading = document.createElement("li");
        heading.className = "timeline-year-heading";
        heading.setAttribute("role", "heading");
        heading.setAttribute("aria-level", "3");
        heading.textContent = year;
        elements.timeline.append(heading);
      }

      const item = document.createElement("li");
      item.className = "commit-item";
      item.dataset.sha = commit.sha;
      item.dataset.index = String(index);

      const details = document.createElement("details");
      details.className = "commit-details";
      const summary = document.createElement("summary");
      summary.innerHTML = `
        <time class="commit-date" datetime="${escapeHtml(commit.date)}">${escapeHtml(formatDate(commit.date))}</time>
        <span class="commit-kind ${escapeHtml(categoryFor(commit.message))}">${escapeHtml(text("categories")[categoryFor(commit.message)])}</span>
        <span class="commit-subject">${escapeHtml(commit.subject)}</span>
        <code class="commit-sha">${escapeHtml(commit.shortSha)}</code>
      `;

      const extra = document.createElement("div");
      extra.className = "commit-extra";
      extra.innerHTML = `
        <span>${escapeHtml(commit.author)} · <code>${escapeHtml(commit.sha)}</code></span>
        <a href="${escapeHtml(commit.url)}" target="_blank" rel="noopener noreferrer">${escapeHtml(text("openCommit"))}</a>
      `;

      details.append(summary, extra);
      item.append(details);
      elements.timeline.append(item);
    }

    const selectedIndex = commits.findIndex(
      (commit) => commit.sha === state.selectedSha,
    );
    const index = selectedIndex >= 0 ? selectedIndex : commits.length - 1;
    updateFocus(index);
  }

  function updateFocus(index) {
    const commits = state.filtered;
    if (!commits.length) {
      elements.focusTitle.textContent = text("noHistory");
      elements.focusMeta.textContent = "";
      elements.focusLink.removeAttribute("href");
      elements.rangeOutput.textContent = "0 / 0";
      return;
    }

    const boundedIndex = Math.max(0, Math.min(index ?? 0, commits.length - 1));
    const commit = commits[boundedIndex];
    state.selectedSha = commit.sha;
    elements.range.value = String(boundedIndex);
    elements.rangeOutput.textContent = `${boundedIndex + 1} / ${commits.length}`;
    elements.focusTitle.textContent = commit.subject;
    elements.focusMeta.textContent = `${formatDate(commit.date, true)} · ${commit.author} · ${commit.shortSha}`;
    elements.focusLink.href = commit.url;
    elements.timeline
      .querySelectorAll(".commit-item")
      .forEach((item) =>
        item.classList.toggle("is-selected", item.dataset.sha === commit.sha),
      );
  }

  function render() {
    renderStats();
    renderTimeline();
  }

  function updateSource(status, error = "") {
    state.sourceStatus = status;
    state.sourceError = error;
    if (status === "live") {
      setSource(
        interpolate(text("live"), { count: state.commits.length }),
        "live",
      );
    } else if (status === "fallback") {
      setSource(
        interpolate(text("fallback"), {
          error: error || "unknown error",
        }),
        "fallback",
      );
    } else {
      setSource(
        interpolate(text("snapshot"), { count: state.commits.length }),
        "snapshot",
      );
    }
  }

  async function fetchMainHistory() {
    const commits = [];
    let page = 1;

    while (true) {
      const response = await fetch(
        `${apiUrl}?sha=main&per_page=${pageSize}&page=${page}`,
        {
          headers: { Accept: "application/vnd.github+json" },
          cache: "no-store",
        },
      );
      if (!response.ok) {
        throw new Error(
          interpolate(text("apiError"), { status: response.status }),
        );
      }

      const batch = await response.json();
      if (!Array.isArray(batch)) throw new Error(text("apiShape"));
      commits.push(...batch);

      const hasNext = /rel="next"/.test(response.headers.get("Link") || "");
      if (!hasNext && batch.length < pageSize) break;
      if (batch.length === 0) break;
      page += 1;
    }

    const unique = new Map();
    for (const item of commits) {
      const commit = normalizeCommit(item);
      if (commit.sha) unique.set(commit.sha, commit);
    }

    return [...unique.values()].reverse();
  }

  async function refreshFromGitHub() {
    elements.refresh.disabled = true;
    setSource(text("refreshing"), "");
    try {
      const commits = await fetchMainHistory();
      if (!commits.length) throw new Error(text("noHistory"));
      state.commits = commits;
      buildYearOptions();
      updateSource("live");
      render();
    } catch (error) {
      updateSource("fallback", error.message);
    } finally {
      elements.refresh.disabled = false;
    }
  }

  async function loadSnapshot() {
    try {
      const response = await fetch("progress-history.json", {
        cache: "no-cache",
      });
      if (!response.ok) throw new Error(text("snapshotError"));
      const snapshot = await response.json();
      if (!Array.isArray(snapshot.commits)) {
        throw new Error(text("snapshotError"));
      }
      state.commits = snapshot.commits.map(normalizeCommit);
      buildYearOptions();
      updateSource("snapshot");
      render();
    } catch (error) {
      state.commits = [];
      buildYearOptions();
      updateSource("fallback", error.message);
      render();
    }
  }

  elements.search.addEventListener("input", renderTimeline);
  elements.year.addEventListener("change", renderTimeline);
  elements.range.addEventListener("input", () =>
    updateFocus(Number(elements.range.value)),
  );
  elements.jumpFirst.addEventListener("click", () => updateFocus(0));
  elements.jumpLatest.addEventListener("click", () =>
    updateFocus(state.filtered.length - 1),
  );
  elements.timeline.addEventListener("click", (event) => {
    const item = event.target.closest(".commit-item");
    if (item) updateFocus(Number(item.dataset.index));
  });
  elements.refresh.addEventListener("click", refreshFromGitHub);
  elements.language.addEventListener("change", applyLanguage);

  state.locale = language();
  applyLanguage();
  void loadSnapshot().then(refreshFromGitHub);
})();
