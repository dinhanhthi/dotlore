import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

const website = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../website");

// Static host only. Root is `website/` so GitHub Pages and this server see the
// same files. Cache stays out of that folder — the Pages artifact uploads it whole.
export default defineConfig({
  root: website,
  publicDir: false,
  cacheDir: path.resolve(website, "../node_modules/.vite-website"),
  server: {
    port: 38423,
    strictPort: true,
  },
});
