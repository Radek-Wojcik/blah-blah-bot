/**
 * Binary command results arrive as an `ArrayBuffer` over the fast IPC protocol, but as a plain
 * array of numbers when Tauri falls back to the postMessage interface (for example when the
 * content security policy blocks the protocol). A `Blob` built from the array itself would hold
 * the text "137,80,78,..." instead of the bytes, so every binary result goes through here.
 */
export type IpcBytes = ArrayBuffer | ArrayLike<number>;

/** A blob with exactly these bytes, whichever form they arrived in. */
export function bytesToBlob(bytes: IpcBytes, type: string): Blob {
  const data = bytes instanceof ArrayBuffer ? bytes : Uint8Array.from(bytes);
  return new Blob([data], { type });
}
