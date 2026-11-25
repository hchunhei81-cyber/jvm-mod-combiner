import {
  lstatSync,
  readdirSync,
  readFileSync,
} from "node:fs";
import { join } from "node:path";

export interface RuntimeSockets {
  logs: string;
  diagnostics: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function exactKeys(
  value: Record<string, unknown>,
  expected: readonly string[],
): boolean {
  const keys = Object.keys(value);
  return keys.length === expected.length &&
    expected.every((key) => Object.hasOwn(value, key));
}

function processIsAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "EPERM";
  }
}

function validSocket(
  path: string,
  directory: string,
  prefix: string,
  pid: number,
  uid: number,
): boolean {
  const name = path.split("/").at(-1) ?? "";

  if (
    path !== join(directory, name) ||
    !name.startsWith(`${prefix}-${pid}-`) ||
    !name.endsWith(".sock")
  ) {
    return false;
  }

  try {
    const stat = lstatSync(path);
    return stat.isSocket() &&
      stat.uid === uid &&
      (stat.mode & 0o777) === 0o600;
  } catch {
    return false;
  }
}

function validateManifest(
  file: string,
  directory: string,
  uid: number,
): RuntimeSockets | null {
  const match = /^runtime-(\d+)-(\d+-\d+)\.json$/.exec(file);
  if (!match) return null;

  const pid = Number(match[1]);
  const nonce = match[2];

  if (!Number.isSafeInteger(pid) || pid <= 0 || !processIsAlive(pid)) {
    return null;
  }

  const manifestPath = join(directory, file);

  try {
    const before = lstatSync(manifestPath);
    if (
      !before.isFile() ||
      before.uid !== uid ||
      (before.mode & 0o777) !== 0o600
    ) {
      return null;
    }

    const raw: unknown = JSON.parse(readFileSync(manifestPath, "utf8"));
    const after = lstatSync(manifestPath);

    if (before.dev !== after.dev || before.ino !== after.ino) {
      return null;
    }

    if (
      !isRecord(raw) ||
      !exactKeys(raw, [
        "protocol",
        "pid",
        "nonce",
        "logs_socket",
        "diagnostics_socket",
      ]) ||
      raw.protocol !== 1 ||
      raw.pid !== pid ||
      raw.nonce !== nonce ||
      typeof raw.logs_socket !== "string" ||
      typeof raw.diagnostics_socket !== "string"
    ) {
      return null;
    }

    if (
      !validSocket(raw.logs_socket, directory, "logs", pid, uid) ||
      !validSocket(raw.diagnostics_socket, directory, "diagnostics", pid, uid)
    ) {
      return null;
    }

    return {
      logs: raw.logs_socket,
      diagnostics: raw.diagnostics_socket,
    };
  } catch {
    return null;
  }
}

export function resolveRuntimeSockets(): RuntimeSockets {
  const uid = process.getuid?.();
  if (uid === undefined) {
    throw new Error("Runtime discovery requires a Unix process UID");
  }

  const directory = `/tmp/jvm-combiner-${uid}`;
  const dirStat = lstatSync(directory);

  if (
    !dirStat.isDirectory() ||
    dirStat.uid !== uid ||
    (dirStat.mode & 0o777) !== 0o700
  ) {
    throw new Error("Runtime directory identity or permissions are invalid");
  }

  const candidates: RuntimeSockets[] = [];

  for (const file of readdirSync(directory)) {
    const candidate = validateManifest(file, directory, uid);
    if (candidate) candidates.push(candidate);
  }

  if (candidates.length === 0) {
    throw new Error("No valid active Fusion runtime manifest found");
  }

  if (candidates.length !== 1) {
    throw new Error(
      `Multiple active Fusion instances found (${candidates.length}); refusing automatic selection`,
    );
  }

  return candidates[0];
}
