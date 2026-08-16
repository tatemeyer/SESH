/// <reference types="vitest" />
import { defineConfig } from "vite";

export default defineConfig({
  server: {
    proxy: {
      "/api": "http://127.0.0.1:7373",
      "/ws": { target: "ws://127.0.0.1:7373", ws: true },
    },
  },
  build: { outDir: "dist", emptyOutDir: true },
  test: { environment: "jsdom" },
});
