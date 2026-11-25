import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "node:path";

// Serves the real UI in a normal browser with the Tauri APIs mocked (for recording the demo).
export default defineConfig({
  root: path.resolve(__dirname, ".."),
  plugins: [react()],
  resolve: {
    alias: {
      "@tauri-apps/api/core": path.resolve(__dirname, "mock-core.ts"),
      "@tauri-apps/plugin-dialog": path.resolve(__dirname, "mock-dialog.ts"),
    },
  },
  server: { port: 1430, strictPort: true },
});
