import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The browser only ever talks to the BFF (/api). The Ledgerd API key never reaches it.
export default defineConfig({
  root: "web",
  plugins: [react()],
  build: { outDir: "dist", emptyOutDir: true },
  server: {
    port: 5173,
    fs: { allow: [".."] }, // web/ imports ../shared/schemas
    proxy: { "/api": process.env.BFF_URL ?? "http://localhost:3001" },
  },
});
