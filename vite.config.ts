import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "path";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: { "@": path.resolve(__dirname, "src") },
  },
  clearScreen: false,
  server: {
    // 5180, not 5173: the embedded TeXbrain editor's dev server owns 5173
    // (ADR 0015 "Dev ports: host 5180, editor 5173").
    port: 5180,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 5183 } : undefined,
    watch: { ignored: ["**/src-tauri/**", "**/.venv/**", "**/node_modules/**"] },
    proxy: {
      // Override with LINXIV_API_PROXY to point dev at an api on another port
      // (e.g. a second instance with a scratch LINXIV_DATA_DIR for testing).
      "/api": process.env.LINXIV_API_PROXY || "http://127.0.0.1:8000",
    },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    minify: !process.env.TAURI_ENV_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
