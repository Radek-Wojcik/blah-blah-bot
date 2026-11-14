import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { BackendProvider } from "./backend-context";
import { selectBackend } from "./select-backend";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <BackendProvider value={selectBackend()}>
      <App />
    </BackendProvider>
  </StrictMode>,
);
