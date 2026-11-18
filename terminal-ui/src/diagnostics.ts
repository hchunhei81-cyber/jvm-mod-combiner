import { useEffect, useState } from "react";
import { createConnection } from "node:net";

export type DiagnosticStatus =
  | "discovering"
  | "ready"
  | "empty"
  | "unavailable"
  | "failed";

export interface DiagnosticChild {
  pid: number;
  loader: "fabric" | "forge" | "neoforge";
  role: string;
  live: boolean;
}

export interface DiagnosticWindow {
  window_id: number;
  pid: number;
  loader: "fabric" | "forge" | "neoforge";
  title: string;
  bounds: {
    x: number;
    y: number;
    width: number;
    height: number;
  };
}

export interface DiagnosticSnapshot {
  protocol: 1;
  status: DiagnosticStatus;
  snapshotAgeMs: number | null;
  children: DiagnosticChild[];
  windows: DiagnosticWindow[];
}

export interface DiagnosticsState {
  connection: "connecting" | "connected" | "unavailable";
  snapshot: DiagnosticSnapshot | null;
  error: string | null;
  latencyMs: number | null;
  lastSuccessAt: number | null;
  stale: boolean;
}

const SOCKET_PATH = `/tmp/jvm-combiner-${process.getuid?.() ?? "unknown"}/diagnostics.sock`;
const REQUEST_TIMEOUT_MS = 1500;
const MAX_RESPONSE_BYTES = 64 * 1024;
const REFRESH_INTERVAL_MS = 2000;
const STALE_AFTER_MS = 5000;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasExactKeys(
  value: Record<string, unknown>,
  expected: readonly string[],
): boolean {
  const actual = Object.keys(value);
  return (
    actual.length === expected.length &&
    expected.every((key) => Object.hasOwn(value, key))
  );
}

function isPositiveInteger(value: unknown): value is number {
  return Number.isSafeInteger(value) && Number(value) > 0;
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function isLoader(
  value: unknown,
): value is DiagnosticChild["loader"] {
  return value === "fabric" || value === "forge" || value === "neoforge";
}

function validateSnapshot(value: unknown): DiagnosticSnapshot {
  if (
    !isRecord(value) ||
    !hasExactKeys(value, [
      "protocol",
      "type",
      "status",
      "snapshot_age_ms",
      "children",
      "windows",
    ])
  ) {
    throw new Error("Invalid diagnostic response fields");
  }

  if (value.protocol !== 1 || value.type !== "snapshot") {
    throw new Error("Unsupported diagnostic protocol");
  }

  const statuses: DiagnosticStatus[] = [
    "discovering",
    "ready",
    "empty",
    "unavailable",
    "failed",
  ];

  if (
    typeof value.status !== "string" ||
    !statuses.includes(value.status as DiagnosticStatus)
  ) {
    throw new Error("Invalid diagnostic status");
  }

  const age = value.snapshot_age_ms;
  if (
    age !== null &&
    (!Number.isSafeInteger(age) || Number(age) < 0)
  ) {
    throw new Error("Invalid snapshot age");
  }

  if (
    !Array.isArray(value.children) ||
    value.children.length > 32 ||
    !Array.isArray(value.windows) ||
    value.windows.length > 128
  ) {
    throw new Error("Diagnostic collection limit exceeded");
  }

  const children: DiagnosticChild[] = value.children.map((item) => {
    if (
      !isRecord(item) ||
      !hasExactKeys(item, ["pid", "loader", "role", "live"]) ||
      !isPositiveInteger(item.pid) ||
      !isLoader(item.loader) ||
      typeof item.role !== "string" ||
      item.role.length === 0 ||
      item.role.length > 64 ||
      typeof item.live !== "boolean"
    ) {
      throw new Error("Invalid diagnostic child");
    }

    return {
      pid: item.pid,
      loader: item.loader,
      role: item.role,
      live: item.live,
    };
  });

  const windows: DiagnosticWindow[] = value.windows.map((item) => {
    if (
      !isRecord(item) ||
      !hasExactKeys(item, [
        "window_id",
        "pid",
        "loader",
        "title",
        "bounds",
      ]) ||
      !isPositiveInteger(item.window_id) ||
      !isPositiveInteger(item.pid) ||
      !isLoader(item.loader) ||
      typeof item.title !== "string" ||
      item.title.length > 256 ||
      !isRecord(item.bounds) ||
      !hasExactKeys(item.bounds, ["x", "y", "width", "height"])
    ) {
      throw new Error("Invalid diagnostic window");
    }

    const { x, y, width, height } = item.bounds;

    if (
      !isFiniteNumber(x) ||
      !isFiniteNumber(y) ||
      !isFiniteNumber(width) ||
      !isFiniteNumber(height) ||
      width <= 0 ||
      height <= 0 ||
      !Number.isFinite(x + width) ||
      !Number.isFinite(y + height)
    ) {
      throw new Error("Invalid diagnostic window bounds");
    }

    return {
      window_id: item.window_id,
      pid: item.pid,
      loader: item.loader,
      title: item.title,
      bounds: { x, y, width, height },
    };
  });

  return {
    protocol: 1,
    status: value.status as DiagnosticStatus,
    snapshotAgeMs: age as number | null,
    children,
    windows,
  };
}

function fetchDiagnosticSnapshot(): Promise<DiagnosticSnapshot> {
  return new Promise((resolve, reject) => {
    let settled = false;
    let received = Buffer.alloc(0);

    const socket = createConnection({ path: SOCKET_PATH });

    const finish = (error: Error | null, result?: DiagnosticSnapshot) => {
      if (settled) return;
      settled = true;
      socket.destroy();

      if (error) reject(error);
      else if (result) resolve(result);
      else reject(new Error("Empty diagnostic response"));
    };

    socket.setTimeout(REQUEST_TIMEOUT_MS, () => {
      finish(new Error("Diagnostic socket timed out"));
    });

    socket.on("connect", () => {
      socket.write('{"protocol":1,"type":"snapshot"}\n', (error) => {
        if (error) finish(error);
      });
    });

    socket.on("data", (chunk: Buffer) => {
      received = Buffer.concat([received, chunk]);

      if (received.length > MAX_RESPONSE_BYTES) {
        finish(new Error("Diagnostic response exceeds size limit"));
        return;
      }

      const newline = received.indexOf(0x0a);
      if (newline < 0) return;

      if (received.subarray(newline + 1).length !== 0) {
        finish(new Error("Unexpected trailing diagnostic data"));
        return;
      }

      try {
        const parsed: unknown = JSON.parse(
          received.subarray(0, newline).toString("utf8"),
        );
        finish(null, validateSnapshot(parsed));
      } catch (error) {
        finish(
          error instanceof Error
            ? error
            : new Error("Invalid diagnostic response"),
        );
      }
    });

    socket.on("error", (error) => finish(error));

    socket.on("end", () => {
      if (!settled) {
        finish(new Error("Diagnostic server closed before response"));
      }
    });
  });
}

export function useDiagnostics(): DiagnosticsState {
  const [state, setState] = useState<DiagnosticsState>({
    connection: "connecting",
    snapshot: null,
    error: null,
    latencyMs: null,
    lastSuccessAt: null,
    stale: true,
  });

  useEffect(() => {
    let active = true;
    let timer: ReturnType<typeof setTimeout> | undefined;

    const poll = async () => {
      const startedAt = Date.now();

      try {
        const snapshot = await fetchDiagnosticSnapshot();
        const receivedAt = Date.now();

        if (active) {
          setState({
            connection: "connected",
            snapshot,
            error: null,
            latencyMs: receivedAt - startedAt,
            lastSuccessAt: receivedAt,
            stale: false,
          });
        }
      } catch (error) {
        if (active) {
          setState((current) => ({
            ...current,
            connection: "unavailable",
            error:
              error instanceof Error
                ? error.message
                : "Unknown diagnostic connection error",
          }));
        }
      } finally {
        if (active) {
          timer = setTimeout(poll, REFRESH_INTERVAL_MS);
        }
      }
    };

    void poll();

    return () => {
      active = false;
      if (timer) clearTimeout(timer);
    };
  }, []);

  const stale =
    state.lastSuccessAt === null ||
    Date.now() - state.lastSuccessAt > STALE_AFTER_MS;

  return { ...state, stale };
}

export function snapshotAgeLabel(state: DiagnosticsState): string {
  if (!state.snapshot || state.lastSuccessAt === null) return "UNKNOWN";

  const serverAge = state.snapshot.snapshotAgeMs ?? 0;
  const elapsed = Math.max(0, Date.now() - state.lastSuccessAt);
  const ageSeconds = Math.floor((serverAge + elapsed) / 1000);

  return state.stale ? `STALE · ${ageSeconds}s` : `${ageSeconds}s`;
}
