/**
 * Scratch folders of the scripted Chrome tests: `<repo>/target/e2e-tmp/`, never the user's
 * temp directory. `globalSetup` removes entries older than one hour (left by an aborted
 * run) and opens a folder for this run, tests take their folders from `freshDir` and remove
 * them in `afterAll`, `globalTeardown` removes the folder of the run.
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
export const REPO_ROOT = path.resolve(here, "../../..");
export const E2E_TMP = path.join(REPO_ROOT, "target", "e2e-tmp");
const RUN_ENV = "BBB_EXT_E2E_RUN_DIR";
const MAX_AGE_MS = 60 * 60 * 1000;

export function removeDir(dir: string): void {
  try {
    fs.rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 500 });
  } catch (e) {
    console.warn(`could not remove ${dir}: ${String(e)}`);
  }
}

export function sweepOld(now = Date.now()): void {
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

export function startRun(): string {
  fs.mkdirSync(E2E_TMP, { recursive: true });
  const dir = fs.mkdtempSync(path.join(E2E_TMP, "ext-"));
  process.env[RUN_ENV] = dir;
  return dir;
}

export function endRun(): void {
  const dir = process.env[RUN_ENV];
  if (dir) removeDir(dir);
}

const made = new Set<string>();

export function freshDir(label: string): string {
  const run = process.env[RUN_ENV] ?? startRun();
  const dir = fs.mkdtempSync(path.join(run, `${label}-`));
  made.add(dir);
  return dir;
}

export function cleanupDirs(): void {
  for (const dir of made) removeDir(dir);
  made.clear();
}
