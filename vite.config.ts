/// <reference types="vitest/config" />
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
export default defineConfig({
  // Node protocol fixtures run separately through test:e2e.
  test: { include: ["src/tests/**/*.test.{ts,tsx}"] },
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
    // Tauri watches Rust itself. Watching toolchains and build output can stall
    // startup before the webview receives its first HTML/JS response.
    watch: {
      ignored: [
        "**/src-tauri/**",
        "**/.tools/**",
        "**/.artifacts/**",
        "**/dist/**",
      ],
    },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    rollupOptions: {
      output: {
        manualChunks: {
          terminal: ["@xterm/xterm", "@xterm/addon-fit"],
          react: ["react", "react-dom"],
        },
      },
    },
  },
});
