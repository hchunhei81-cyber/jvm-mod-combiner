import { useEffect, useRef, useState } from "react";
import { createConnection } from "node:net";

export type LogSource = "fusion" | "fabric" | "neoforge";
export type LogStream = "stdout" | "stderr";
export type LogLevel =
  | "ERROR"
  | "WARN"
  | "INFO"
  | "DEBUG"
  | "TRACE"
  | "UNKNOWN";

export interface LogEntry {
  id: number;
  timestamp_ms: number;
  source: LogSource;
  stream: LogStream;
  level: LogLevel;
  message: string;
}

export interface LogsState {
  connection: "connecting" | "connected" | "unavailable";
  entries: LogEntry[];
  latestId: number;
  firstAvailableId: number;
  cursor: number;
  dropped: number;
  gapDetected: boolean;
  missingCount: number;
  cursorReset: boolean;
  error: string | null;
  latencyMs: number | null;
  lastSuccessAt: number | null;
  stale: boolean;
}

const SOCKET_PATH =
  `/tmp/jvm-combiner-${process.getuid?.() ?? "unknown"}/logs.sock`;

const REQUEST_TIMEOUT_MS = 1500;
const REFRESH_INTERVAL_MS = 250;
const CATCH_UP_INTERVAL_MS = 25;
const MAX_RESPONSE_BYTES = 256 * 1024;
const MAX_BATCH = 16;
const MAX_LOCAL_ENTRIES = 2048;
const STALE_AFTER_MS = 5000;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function exactKeys(
  value: Record<string, unknown>,
  expected: readonly string[],
): boolean {
  const actual = Object.keys(value);
  return (
    actual.length === expected.length &&
    expected.every((key) => Object.hasOwn(value, key))
  );
}

function safeInteger(value: unknown): value is number {
  return Number.isSafeInteger(value) && Number(value) >= 0;
}

function positiveInteger(value: unknown): value is number {
  return Number.isSafeInteger(value) && Number(value) > 0;
}

function validateEntry(value: unknown): LogEntry {
  if (
    !isRecord(value) ||
    !exactKeys(value, [
      "id",
      "timestamp_ms",
      "source",
      "stream",
      "level",
      "message",
    ]) ||
    !positiveInteger(value.id) ||
    !safeInteger(value.timestamp_ms) ||
    (value.source !== "fusion" &&
      value.source !== "fabric" &&
      value.source !== "neoforge") ||
    (value.stream !== "stdout" && value.stream !== "stderr") ||
    !(
      value.level === "ERROR" ||
      value.level === "WARN" ||
      value.level === "INFO" ||
      value.level === "DEBUG" ||
      value.level === "TRACE" ||
      value.level === "UNKNOWN"
    ) ||
    typeof value.message !== "string" ||
    Buffer.byteLength(value.message, "utf8") > 2048
  ) {
    throw new Error("Invalid log entry");
  }

  return {
    id: value.id,
    timestamp_ms: value.timestamp_ms,
    source: value.source,
    stream: value.stream,
    level: value.level,
    message: value.message,
  };
}

interface LogBatch {
  entries: LogEntry[];
  latestId: number;
  firstAvailableId: number;
  dropped: number;
  gapDetected: boolean;
  missingCount: number;
  cursorReset: boolean;
}

function validateBatch(value: unknown, afterId: number): LogBatch {
  if (
    !isRecord(value) ||
    !exactKeys(value, [
      "protocol",
      "type",
      "entries",
      "latest_id",
      "first_available_id",
      "dropped",
      "gap_detected",
      "missing_count",
      "cursor_reset",
    ])
  ) {
    throw new Error("Invalid log response fields");
  }

  if (value.protocol !== 1 || value.type !== "logs") {
    throw new Error("Unsupported log protocol");
  }

  if (
    !Array.isArray(value.entries) ||
    value.entries.length > MAX_BATCH ||
    !safeInteger(value.latest_id) ||
    !safeInteger(value.first_available_id) ||
    !safeInteger(value.dropped) ||
    !safeInteger(value.missing_count) ||
    typeof value.gap_detected !== "boolean" ||
    typeof value.cursor_reset !== "boolean"
  ) {
    throw new Error("Invalid log batch limits");
  }

  const entries = value.entries.map(validateEntry);

  let previousId = value.cursor_reset ? 0 : afterId;

  for (const entry of entries) {
    if (entry.id <= previousId) {
      throw new Error("Log IDs are not strictly increasing");
    }
    if (entry.id > value.latest_id) {
      throw new Error("Log entry exceeds latest ID");
    }
    previousId = entry.id;
  }

  return {
    entries,
    latestId: value.latest_id,
    firstAvailableId: value.first_available_id,
    dropped: value.dropped,
    gapDetected: value.gap_detected,
    missingCount: value.missing_count,
    cursorReset: value.cursor_reset,
  };
}

function fetchLogBatch(afterId: number): Promise<LogBatch> {
  return new Promise((resolve, reject) => {
    let settled = false;
    let received = Buffer.alloc(0);

    const socket = createConnection({ path: SOCKET_PATH });

    const finish = (error: Error | null, result?: LogBatch) => {
      if (settled) return;
      settled = true;
      socket.destroy();

      if (error) reject(error);
      else if (result) resolve(result);
      else reject(new Error("Empty log response"));
    };

    socket.setTimeout(REQUEST_TIMEOUT_MS, () => {
      finish(new Error("Log socket timed out"));
    });

    socket.on("connect", () => {
      const request = JSON.stringify({
        protocol: 1,
        type: "logs",
        after_id: afterId,
        limit: MAX_BATCH,
      }) + "\n";

      socket.write(request, (error) => {
        if (error) finish(error);
      });
    });

    socket.on("data", (chunk: Buffer) => {
      received = Buffer.concat([received, chunk]);

      if (received.length > MAX_RESPONSE_BYTES) {
        finish(new Error("Log response exceeds size limit"));
        return;
      }

      const newline = received.indexOf(0x0a);
      if (newline < 0) return;

      if (received.subarray(newline + 1).length !== 0) {
        finish(new Error("Unexpected trailing log data"));
        return;
      }

      try {
        const parsed: unknown = JSON.parse(
          received.subarray(0, newline).toString("utf8"),
        );

        finish(null, validateBatch(parsed, afterId));
      } catch (error) {
        finish(
          error instanceof Error ? error : new Error("Invalid log response"),
        );
      }
    });

    socket.on("error", (error) => finish(error));

    socket.on("end", () => {
      if (!settled) {
        finish(new Error("Log server closed before response"));
      }
    });
  });
}

export function useLogs(enabled: boolean): LogsState {
  const [state, setState] = useState<LogsState>({
    connection: "connecting",
    entries: [],
    latestId: 0,
    firstAvailableId: 0,
    cursor: 0,
    dropped: 0,
    gapDetected: false,
    missingCount: 0,
    cursorReset: false,
    error: null,
    latencyMs: null,
    lastSuccessAt: null,
    stale: true,
  });

  const cursorRef = useRef(0);

  useEffect(() => {
    if (!enabled) return;

    let active = true;
    let timer: ReturnType<typeof setTimeout> | undefined;

    setState((current) => ({
      ...current,
      connection: "connecting",
      error: null,
    }));

    const poll = async () => {
      const startedAt = Date.now();
      let nextRefreshIntervalMs = REFRESH_INTERVAL_MS;

      try {
        const batch = await fetchLogBatch(cursorRef.current);
        nextRefreshIntervalMs =
          batch.entries.length === MAX_BATCH
            ? CATCH_UP_INTERVAL_MS
            : REFRESH_INTERVAL_MS;
        const receivedAt = Date.now();

        if (!active) return;

        // Reset the local cache when the server's ID counter restarted.
        if (batch.cursorReset) {
          cursorRef.current = 0;
        }

        // Advance only to the last entry actually received.
        if (batch.entries.length > 0) {
          cursorRef.current =
            batch.entries[batch.entries.length - 1].id;
        }

        setState((current) => {
          const merged = batch.cursorReset ? [] : [...current.entries];

          for (const entry of batch.entries) {
            if (!merged.some((existing) => existing.id === entry.id)) {
              merged.push(entry);
            }
          }

          merged.sort((a, b) => a.id - b.id);

          return {
            connection: "connected",
            entries: merged.slice(-MAX_LOCAL_ENTRIES),
            latestId: batch.latestId,
            firstAvailableId: batch.firstAvailableId,
            cursor: cursorRef.current,
            dropped: batch.dropped,
            gapDetected: batch.gapDetected,
            missingCount: batch.missingCount,
            cursorReset: batch.cursorReset,
            error: null,
            latencyMs: receivedAt - startedAt,
            lastSuccessAt: receivedAt,
            stale: false,
          };
        });
      } catch (error) {
        if (active) {
          setState((current) => ({
            ...current,
            connection: "unavailable",
            cursor: cursorRef.current,
            error:
              error instanceof Error ? error.message : "Unknown log error",
          }));
        }
      } finally {
        if (active) {
          timer = setTimeout(poll, nextRefreshIntervalMs);
        }
      }
    };

    void poll();

    return () => {
      active = false;
      if (timer) clearTimeout(timer);
    };
  }, [enabled]);

  const stale =
    state.lastSuccessAt === null ||
    Date.now() - state.lastSuccessAt > STALE_AFTER_MS;

  return { ...state, stale };
}
