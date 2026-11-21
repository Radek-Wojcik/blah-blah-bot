/**
 * Scratch folders of the end-to-end and screenshot runs.
 *
 * They live under `<repo>/target/e2e-tmp/`, never in the user's temp directory:
 * - `globalSetup` removes entries older than one hour (left by an aborted run) and opens a
 *   folder for this run (`startRun`);
 * - tests take their folders from `freshDir` and remove them in `afterEach` (`cleanupDirs`);
 * - `globalTeardown` removes the folder of this run (`endRun`).
 * One command, `scripts/clean-test-tmp.ps1` (or deleting `target/e2e-tmp`), removes everything.
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
export const E2E_TMP = path.resolve(here, "../../target/e2e-tmp");
const RUN_ENV = "BBB_E2E_RUN_DIR";
const MAX_AGE_MS = 60 * 60 * 1000;

/** Removes a folder tree; retries because a just killed process may still hold a file open. */
export function removeDir(dir: string) {
  try {
    fs.rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 500 });
  } catch (e) {
    console.warn(`could not remove ${dir}: ${String(e)}`);
  }
}

/** Removes the entries of `target/e2e-tmp` that are older than one hour. */
export function sweepOld(now = Date.now()) {
  if (!fs.existsSync(E2E_TMP)) return;
  for (const entry of fs.readdirSync(E2E_TMP)) {
    const p = path.join(E2E_TMP, entry);
    try {
      if (now - fs.statSync(p).mtimeMs > MAX_AGE_MS) removeDir(p);
    } catch {
      /* gone meanwhile */
    }
  }
}

/** Opens the folder of this run and tells the workers about it through the environment. */
export function startRun(): string {
  fs.mkdirSync(E2E_TMP, { recursive: true });
  const dir = fs.mkdtempSync(path.join(E2E_TMP, "run-"));
  process.env[RUN_ENV] = dir;
  return dir;
}

/** Removes the folder of this run. */
export function endRun() {
  const dir = process.env[RUN_ENV];
  if (dir) removeDir(dir);
}

const made = new Set<string>();

/** A new empty folder for one test, inside the folder of the run. */
export function freshDir(label: string): string {
  const run = process.env[RUN_ENV] ?? startRun();
  const dir = fs.mkdtempSync(path.join(run, `${label}-`));
  made.add(dir);
  return dir;
}

/** Removes every folder made by `freshDir` in this worker. Call it from `afterEach`. */
export function cleanupDirs() {
  for (const dir of made) removeDir(dir);
  made.clear();
}
