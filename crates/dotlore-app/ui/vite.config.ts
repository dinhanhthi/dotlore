import path from "node:path";
import { fileURLToPath } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const root = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(root, "../../..");

export default defineConfig({
  publicDir: path.join(repo, "assets"),
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.join(root, "src"),
    },
  },
  server: {
    port: 38421,
    strictPort: true,
  },
  build: {
    outDir: "dist",
  },
});
