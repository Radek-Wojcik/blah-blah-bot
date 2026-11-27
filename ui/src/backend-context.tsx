import { createContext, useContext } from "react";
import type { Backend } from "./ipc/backend";

const BackendContext = createContext<Backend | null>(null);

export const BackendProvider = BackendContext.Provider;

/** The backend, or null outside a provider (for components that can work without one). */
export function useOptionalBackend(): Backend | null {
  return useContext(BackendContext);
}

export function useBackend(): Backend {
  const b = useContext(BackendContext);
  if (!b) throw new Error("BackendProvider is missing");
  return b;
}
