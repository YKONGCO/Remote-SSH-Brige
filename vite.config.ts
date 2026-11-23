import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
export default defineConfig({
  test: {
    include: ["src/tests/**/*.test.{ts,tsx}"],
    environment: "jsdom",
  },
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
