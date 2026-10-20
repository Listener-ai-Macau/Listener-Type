import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { execFileSync } from "node:child_process";

const host = process.env.TAURI_DEV_HOST;
const sourceRevision = (() => {
  try {
    const revision = execFileSync('git', ['rev-parse', '--short=12', 'HEAD'], { encoding: 'utf8' }).trim();
    const modified = execFileSync('git', ['status', '--porcelain', '--untracked-files=normal'], { encoding: 'utf8' }).trim();
    return `${revision}${modified ? '+modified' : ''}`;
  } catch {
    return 'unavailable';
  }
})();

export default defineConfig(async () => ({
  plugins: [react()],
  define: { 'import.meta.env.VITE_APP_SOURCE_REVISION': JSON.stringify(sourceRevision) },
  base: "./",
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? { protocol: "ws", host, port: 1421 }
      : undefined,
    watch: { ignored: ["**/src-tauri/**", "**/ref/**", "**/.task/**"] },
  },
  optimizeDeps: {
    entries: ["index.html"],
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: process.env.TAURI_PLATFORM === "windows" ? "chrome105" : "safari13",
    minify: !process.env.TAURI_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_DEBUG,
    chunkSizeWarningLimit: 800,
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (!id.includes("node_modules")) return undefined;
          if (id.includes("/react/") || id.includes("/react-dom/")) return "vendor-react";
          if (id.includes("/i18next/") || id.includes("/react-i18next/")) return "vendor-i18n";
          if (id.includes("/marked/")) return "vendor-markdown";
          if (id.includes("/@tauri-apps/")) return "vendor-tauri";
          return undefined;
        },
      },
    },
  },
}));
