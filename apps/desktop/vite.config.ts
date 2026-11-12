import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { readFileSync } from "node:fs";
import { fileURLToPath, URL } from "node:url";

const pkg = JSON.parse(readFileSync(new URL("./package.json", import.meta.url), "utf-8")) as { version: string };

// Tauri expects a fixed dev port; the Rust side points devUrl at it.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  define: { __APP_VERSION__: JSON.stringify(pkg.version) },
  resolve: {
    alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
  },
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { outDir: "dist", emptyOutDir: true },
});
