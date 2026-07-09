import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";
import App from "./App";
import "react-pdf/dist/Page/AnnotationLayer.css";
import "react-pdf/dist/Page/TextLayer.css";
import "./styles/globals.css";
import { useThemeStore } from "./stores/theme";
import { queryClient } from "./lib/queryClient";
import { getSettings } from "./api/settings";

useThemeStore.getState();

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
  // The backend runs in-process (Tauri manages it at startup), so there is no
  // port to resolve. Warm the settings cache so the first render reads the saved
  // preference (e.g. tex_rendering_enabled) instead of the default, avoiding a flash.
  await queryClient
    .prefetchQuery({ queryKey: ["settings"], queryFn: getSettings })
    .catch(() => {});
  renderApp();
}

bootstrap();
