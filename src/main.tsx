import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import App from "./App";
import "./styles/globals.css";
import { useThemeStore } from "./stores/theme";
import { isTauri, setApiPort } from "./api/client";
import { queryClient } from "./lib/queryClient";
import { getSettings } from "./api/settings";

useThemeStore.getState();

function renderApiStartupError(detail: string) {
  ReactDOM.createRoot(document.getElementById("root")!).render(
    <div
      style={{
        padding: "32px",
        fontFamily: "system-ui, sans-serif",
        color: "#e2e8f0",
        background: "#0f172a",
        minHeight: "100vh",
      }}
    >
      <h1 style={{ fontSize: 24, marginBottom: 12 }}>linXiv — backend failed to start</h1>
      <p style={{ color: "#94a3b8", marginBottom: 16 }}>
        The Python API server did not respond. The app cannot operate without it.
      </p>
      <p style={{ color: "#64748b", fontSize: 14, marginBottom: 16 }}>
        Try quitting and reopening the app. If the problem persists, check that no other
        process is occupying the local backend port, then file an issue with the detail below.
      </p>
      <pre
        style={{
          background: "#1e293b",
          color: "#cbd5e1",
          padding: 12,
          borderRadius: 6,
          fontSize: 12,
          whiteSpace: "pre-wrap",
        }}
      >
        {detail}
      </pre>
    </div>
  );
}

function renderApp() {
  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <QueryClientProvider client={queryClient}>
        <App />
      </QueryClientProvider>
    </React.StrictMode>
  );
}

async function bootstrap() {
  if (isTauri) {
    try {
      const port = await invoke<number>("get_api_port");
      setApiPort(port);
    } catch (err) {
      const detail = err instanceof Error ? err.message : String(err);
      // No silent fallback: the whole reason for the dynamic-port fix is that
      // the default 8000 may be unavailable. Surface the failure explicitly.
      renderApiStartupError(detail);
      return;
    }
  }
  // Warm the settings cache so the first render reads the saved preference
  // (e.g. tex_rendering_enabled) instead of the default, avoiding a flash.
  await queryClient
    .prefetchQuery({ queryKey: ["settings"], queryFn: getSettings })
    .catch(() => {});
  renderApp();
}

bootstrap();
