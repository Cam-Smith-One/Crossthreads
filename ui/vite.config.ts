import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";

// The built app is served by `crossthreadsd --ui <dist>`. In dev, proxy the API
// to a locally running daemon (`crossthreadsd --http 127.0.0.1:47101`).
export default defineConfig({
  plugins: [react()],
  resolve: { alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) } },
  build: { outDir: "dist" },
  server: {
    proxy: {
      "/api": "http://127.0.0.1:47101",
    },
  },
});
