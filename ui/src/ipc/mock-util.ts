import type { IpcErrorCode } from "./types";

/** Throws the same shape of error the Tauri backend rejects with (`{ code, message }`). */
export function fail(code: IpcErrorCode, message: string): never {
  throw { code, message };
}

/** Identifiers of cycles, participants, templates and meeting types (`docs/CYCLES.md`). */
export const IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9_-]*$/;

/** A section id: starts with a lowercase letter, then lowercase letters, digits, `_` and `-`. */
export const SECTION_ID = /^[a-z][a-z0-9_-]*$/;
