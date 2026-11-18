import React, { useState } from "react";
import {
  useDiagnostics,
  snapshotAgeLabel,
  type DiagnosticsState,
} from "./diagnostics.js";
import { useLogs, type LogsState } from "./logs.js";
import { render, Box, Text, useApp, useInput } from "ink";

type Page =
  | "dashboard"
  | "logs"
  | "processes"
  | "windows"
  | "diagnostics"
  | "performance";
type Color = "green" | "yellow" | "gray" | "red" | "cyan";

const pages: { id: Page; key: string; label: string }[] = [
  { id: "dashboard", key: "F1", label: "Dashboard" },
  { id: "logs", key: "F2", label: "Logs" },
  { id: "processes", key: "F3", label: "Processes" },
  { id: "windows", key: "F4", label: "Windows" },
  { id: "diagnostics", key: "F5", label: "Diagnostics" },
  { id: "performance", key: "F6", label: "Performance" },
];

function Status({
  name,
  value,
  color = "green",
}: {
  name: string;
  value: string;
  color?: Color;
}) {
  return (
    <Box justifyContent="space-between">
      <Text color="gray">{name}</Text>
      <Text color={color} bold>
        {value}
      </Text>
    </Box>
  );
}

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <Box flexDirection="column" marginTop={1}>
      <Text bold color="yellow">
        {title}
      </Text>
      {children}
    </Box>
  );
}

function Dashboard({ diagnostics }: { diagnostics: DiagnosticsState }) {
  const snapshot = diagnostics.snapshot;
  const online = diagnostics.connection === "connected" && !diagnostics.stale;
  const count = (loader: string) =>
    snapshot?.children.filter((child) => child.loader === loader && child.live)
      .length ?? 0;

  return (
    <>
      <Section title="BACKENDS">
        <Status
          name="Rust Kernel / Fusion"
          value={online ? "CONNECTED" : diagnostics.connection.toUpperCase()}
          color={online ? "green" : "yellow"}
        />
        <Status
          name="Java Bridge"
          value={
            snapshot ? `${snapshot.children.length} REGISTERED` : "UNKNOWN"
          }
          color={snapshot ? "green" : "gray"}
        />
        <Status
          name="Swift Backend"
          value="EXPERIMENTAL / UNKNOWN"
          color="yellow"
        />
      </Section>
      <Section title="LOADER INSTANCES">
        <Status
          name="Fabric"
          value={`${count("fabric")} LIVE`}
          color={count("fabric") ? "green" : "gray"}
        />
        <Status
          name="Forge"
          value={`${count("forge")} LIVE`}
          color={count("forge") ? "green" : "gray"}
        />
        <Status
          name="NeoForge"
          value={`${count("neoforge")} LIVE`}
          color={count("neoforge") ? "green" : "gray"}
        />
      </Section>
      <Section title="WINDOW MANAGEMENT">
        <Status
          name="Discovery"
          value={snapshot?.status.toUpperCase() ?? "UNKNOWN"}
          color={
            snapshot?.status === "ready" || snapshot?.status === "empty"
              ? "green"
              : "yellow"
          }
        />
        <Status
          name="Discovered windows"
          value={snapshot ? String(snapshot.windows.length) : "UNKNOWN"}
          color={snapshot ? "green" : "gray"}
        />
        <Status
          name="Mutation Gate"
          value="DISABLED / FAIL-CLOSED"
          color="green"
        />
      </Section>
      <Section title="RUNTIME">
        <Status
          name="Diagnostic IPC"
          value={diagnostics.connection.toUpperCase()}
          color={online ? "green" : "yellow"}
        />
        <Status
          name="Snapshot age"
          value={snapshotAgeLabel(diagnostics)}
          color={diagnostics.stale ? "yellow" : "green"}
        />
      </Section>
    </>
  );
}

function LogsPage({ logs }: { logs: LogsState }) {
  const [sourceFilter, setSourceFilter] = useState("all");
  const [levelFilter, setLevelFilter] = useState("all");

  useInput((input) => {
    const key = input.toLowerCase();

    if (key === "s") {
      setSourceFilter((current) => {
        const choices = ["all", "fusion", "fabric", "neoforge"];
        return choices[(choices.indexOf(current) + 1) % choices.length];
      });
    } else if (key === "l") {
      setLevelFilter((current) => {
        const choices = [
          "all", "ERROR", "WARN", "INFO", "DEBUG", "TRACE", "UNKNOWN",
        ];
        return choices[(choices.indexOf(current) + 1) % choices.length];
      });
    }
  });

  const visible = logs.entries.filter((entry) => {
    const sourceMatches =
      sourceFilter === "all" || entry.source === sourceFilter;
    const levelMatches =
      levelFilter === "all" || entry.level === levelFilter;
    return sourceMatches && levelMatches;
  });

  const displayed = visible.slice(-9);

  const status =
    logs.connection === "connected" && !logs.stale
      ? "CONNECTED"
      : logs.connection === "connecting"
        ? "CONNECTING"
        : "DISCONNECTED";

  const levelColor = (level: string): Color => {
    if (level === "ERROR") return "red";
    if (level === "WARN") return "yellow";
    if (level === "INFO") return "green";
    if (level === "DEBUG" || level === "TRACE") return "cyan";
    return "gray";
  };

  const safeMessage = (message: string) =>
    message.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ").slice(0, 43);

  return (
    <Section title="LIVE LOG STREAM">
      <Status
        name="Log UDS"
        value={status}
        color={status === "CONNECTED" ? "green" : "yellow"}
      />
      <Status
        name="Source filter"
        value={`${sourceFilter.toUpperCase()}  [S]`}
        color="cyan"
      />
      <Status
        name="Level filter"
        value={`${levelFilter}  [L]`}
        color="cyan"
      />
      <Status name="Cursor / latest" value={`${logs.cursor} / ${logs.latestId}`} />
      <Status
        name="Available from"
        value={String(logs.firstAvailableId)}
      />
      <Status
        name="Log gap"
        value={
          logs.cursorReset
            ? "SERVER RESTARTED"
            : logs.gapDetected
              ? `${logs.missingCount} MISSING`
              : "NONE"
        }
        color={
          logs.cursorReset || logs.gapDetected
            ? "yellow"
            : "green"
        }
      />
      <Status name="Ring buffer evictions" value={String(logs.dropped)} />
      <Status
        name="IPC latency"
        value={logs.latencyMs === null ? "UNKNOWN" : `${logs.latencyMs} ms`}
      />

      {logs.error && (
        <Text color="yellow">Connection: {logs.error.slice(0, 100)}</Text>
      )}

      <Text color="gray">TIME       SOURCE     LEVEL   MESSAGE</Text>

      {displayed.length === 0 && (
        <Text color="gray">
          {logs.connection === "connected"
            ? "No matching log entries."
            : "Waiting for logs.sock..."}
        </Text>
      )}

      {displayed.map((entry) => {
        const time = new Date(entry.timestamp_ms).toLocaleTimeString();
        return (
          <Box key={entry.id}>
            <Text color="gray">{time.slice(-8)} </Text>
            <Text color="cyan">{entry.source.padEnd(8)} </Text>
            <Text color={levelColor(entry.level)}>
              {entry.level.padEnd(5)}{" "}
            </Text>
            <Text>{safeMessage(entry.message)}</Text>
          </Box>
        );
      })}

      <Text color="gray">
        Showing {displayed.length} / {visible.length} · cached {logs.entries.length}
      </Text>
    </Section>
  );
}

function ProcessesPage({ diagnostics }: { diagnostics: DiagnosticsState }) {
  const children = diagnostics.snapshot?.children ?? [];

  return (
    <Section title="MANAGED PROCESSES">
      {children.length === 0 && (
        <Text color="gray">
          {diagnostics.snapshot
            ? "No registered child processes."
            : "Waiting for Rust diagnostic snapshot."}
        </Text>
      )}
      {children.map((child) => (
        <Box
          key={`${child.pid}-${child.loader}`}
          justifyContent="space-between"
        >
          <Text color="gray">
            {child.loader.toUpperCase()} · PID {child.pid}
          </Text>
          <Text color={child.live ? "green" : "red"} bold>
            {child.live ? "LIVE" : "EXITED"}
          </Text>
        </Box>
      ))}
      <Status
        name="Snapshot freshness"
        value={snapshotAgeLabel(diagnostics)}
        color={diagnostics.stale ? "yellow" : "green"}
      />
    </Section>
  );
}

function WindowsPage({ diagnostics }: { diagnostics: DiagnosticsState }) {
  const windows = diagnostics.snapshot?.windows ?? [];

  return (
    <Section title="WINDOW DISCOVERY">
      <Status
        name="Discovered windows"
        value={diagnostics.snapshot ? String(windows.length) : "UNKNOWN"}
        color={diagnostics.snapshot ? "green" : "gray"}
      />
      <Status
        name="Snapshot freshness"
        value={snapshotAgeLabel(diagnostics)}
        color={diagnostics.stale ? "yellow" : "green"}
      />
      <Status name="Window mutation" value="DISABLED" color="green" />
      {windows.length === 0 && (
        <Text color="gray">
          {diagnostics.snapshot
            ? "No windows in the current snapshot."
            : "Waiting for authenticated Rust snapshot."}
        </Text>
      )}
      {windows.map((window) => (
        <Box
          key={`${window.window_id}-${window.pid}`}
          flexDirection="column"
          marginTop={1}
        >
          <Text color="cyan" bold>
            {window.title || "<untitled>"}
          </Text>
          <Text color="gray">
            ID {window.window_id} · PID {window.pid} · {window.loader}
          </Text>
          <Text color="gray">
            {Math.round(window.bounds.x)},{Math.round(window.bounds.y)} ·{" "}
            {Math.round(window.bounds.width)}×{Math.round(window.bounds.height)}
          </Text>
        </Box>
      ))}
    </Section>
  );
}

function DiagnosticsPage({ diagnostics }: { diagnostics: DiagnosticsState }) {
  const snapshot = diagnostics.snapshot;

  return (
    <Section title="SYSTEM DIAGNOSTICS">
      <Status
        name="Rust diagnostic endpoint"
        value={diagnostics.connection.toUpperCase()}
        color={diagnostics.connection === "connected" ? "green" : "yellow"}
      />
      <Status
        name="Protocol version"
        value={snapshot ? String(snapshot.protocol) : "UNKNOWN"}
        color={snapshot ? "green" : "gray"}
      />
      <Status
        name="Snapshot status"
        value={snapshot?.status.toUpperCase() ?? "UNKNOWN"}
        color={snapshot ? "green" : "gray"}
      />
      <Status
        name="Snapshot freshness"
        value={snapshotAgeLabel(diagnostics)}
        color={diagnostics.stale ? "yellow" : "green"}
      />
      <Status
        name="IPC round trip"
        value={
          diagnostics.latencyMs === null
            ? "UNKNOWN"
            : `${diagnostics.latencyMs} ms`
        }
        color="cyan"
      />
      <Status
        name="Peer authentication"
        value={
          diagnostics.connection === "connected"
            ? "RUST ACCEPTED"
            : "NOT VERIFIED"
        }
        color={diagnostics.connection === "connected" ? "green" : "yellow"}
      />
      {diagnostics.error && <Text color="yellow">{diagnostics.error}</Text>}
      <Text color="gray">
        Read-only TUI. Authorization remains inside Rust.
      </Text>
    </Section>
  );
}

function PerformancePage() {
  return (
    <Section title="PERFORMANCE TELEMETRY">
      <Status name="Kernel CPU / memory" value="UNAVAILABLE" color="gray" />
      <Status name="JVM CPU / memory" value="UNAVAILABLE" color="gray" />
      <Status name="IPC latency" value="UNAVAILABLE" color="gray" />
      <Status name="Queue depth" value="UNAVAILABLE" color="gray" />
      <Text color="gray">Metrics require a real telemetry source.</Text>
    </Section>
  );
}

function App() {
  const [page, setPage] = useState<Page>("dashboard");
  const diagnostics = useDiagnostics();
  const logs = useLogs(page === "logs");
  const { exit } = useApp();

  useInput((input, key) => {
    if (
      input.toLowerCase() === "q" ||
      key.escape ||
      (key.ctrl && input === "c")
    ) {
      exit();
      return;
    }

    const functionKey = key as typeof key & {
      f1?: boolean;
      f2?: boolean;
      f3?: boolean;
      f4?: boolean;
      f5?: boolean;
      f6?: boolean;
    };

    if (functionKey.f1 || input === "1") setPage("dashboard");
    else if (functionKey.f2 || input === "2") setPage("logs");
    else if (functionKey.f3 || input === "3") setPage("processes");
    else if (functionKey.f4 || input === "4") setPage("windows");
    else if (functionKey.f5 || input === "5") setPage("diagnostics");
    else if (functionKey.f6 || input === "6") setPage("performance");
  });

  const active = pages.find((item) => item.id === page)!;

  return (
    <Box
      flexDirection="column"
      borderStyle="round"
      borderColor="cyan"
      padding={1}
      width={84}
    >
      <Box justifyContent="space-between">
        <Text bold color="cyan">
          JVM MOD COMBINER / DEVELOPER CONSOLE
        </Text>
        <Text color="gray">v0.1.0</Text>
      </Box>
      <Text color="gray">
        Rust kernel · JVM bridge · Native backend · Runtime diagnostics
      </Text>

      <Box marginTop={1}>
        {pages.map((item) => (
          <Box key={item.id} marginRight={2}>
            <Text
              bold={page === item.id}
              color={page === item.id ? "cyan" : "gray"}
            >
              {item.key} {item.label}
            </Text>
          </Box>
        ))}
      </Box>

      <Box
        marginTop={1}
        borderStyle="single"
        borderColor="gray"
        paddingX={1}
        flexDirection="column"
      >
        <Text bold color="cyan">
          {active.label.toUpperCase()}
        </Text>
        {page === "dashboard" && <Dashboard diagnostics={diagnostics} />}
        {page === "logs" && <LogsPage logs={logs} />}
        {page === "processes" && <ProcessesPage diagnostics={diagnostics} />}
        {page === "windows" && <WindowsPage diagnostics={diagnostics} />}
        {page === "diagnostics" && (
          <DiagnosticsPage diagnostics={diagnostics} />
        )}
        {page === "performance" && <PerformancePage />}
      </Box>

      <Box marginTop={1} justifyContent="space-between">
        <Text color="gray">F1–F6 / 1–6 Navigate</Text>
        <Text color="gray">Q / Esc Quit</Text>
      </Box>
    </Box>
  );
}

render(<App />, {
  alternateScreen: true,
});
