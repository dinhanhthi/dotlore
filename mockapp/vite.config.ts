import path from "node:path";
import { fileURLToPath } from "node:url";

import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const mockapp = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(mockapp, "..");
const uiSrc = path.join(repo, "src");
const uiNm = path.join(repo, "node_modules");

const ALIASED_TAURI = new Set([
  "@tauri-apps/api/core",
  "@tauri-apps/api/event",
  "@tauri-apps/api/window",
  "@tauri-apps/api/app",
  "@tauri-apps/plugin-opener",
  "@tauri-apps/plugin-dialog",
]);

function tauriAliasGuard() {
  return {
    name: "tauri-alias-guard",
    resolveId(id: string) {
      if (id.startsWith("@tauri-apps/") && !ALIASED_TAURI.has(id)) {
        throw new Error(
          `[mockapp] Unaliased @tauri-apps import: "${id}". Add a mock in mockapp/mocks/.`,
        );
      }
    },
  };
}

const TAURI_MOCK_IDS = [...ALIASED_TAURI];

export default defineConfig({
  root: mockapp,
  cacheDir: path.join(uiNm, ".vite-mockapp"),
  base: "./",
  publicDir: path.join(repo, "assets"),
  plugins: [react(), tailwindcss(), tauriAliasGuard()],
  optimizeDeps: {
    exclude: TAURI_MOCK_IDS,
  },
  resolve: {
    dedupe: ["react", "react-dom"],
    alias: {
      react: path.join(uiNm, "react"),
      "react-dom": path.join(uiNm, "react-dom"),
      "@": uiSrc,
      "@tauri-apps/api/core": path.join(mockapp, "mocks/core.ts"),
      "@tauri-apps/api/event": path.join(mockapp, "mocks/event.ts"),
      "@tauri-apps/api/window": path.join(mockapp, "mocks/window.ts"),
      "@tauri-apps/api/app": path.join(mockapp, "mocks/app.ts"),
      "@tauri-apps/plugin-opener": path.join(mockapp, "mocks/plugin-opener.ts"),
      "@tauri-apps/plugin-dialog": path.join(mockapp, "mocks/plugin-dialog.ts"),
    },
  },
  server: {
    port: 38422,
    strictPort: true,
    host: true,
  },
  preview: {
    port: 38423,
    strictPort: true,
  },
  build: {
    outDir: path.join(mockapp, "dist"),
    emptyOutDir: true,
    rollupOptions: {
      input: path.join(mockapp, "index.html"),
    },
  },
});
