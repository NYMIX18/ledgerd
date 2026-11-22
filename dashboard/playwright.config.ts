import { defineConfig } from "@playwright/test";

// Expects the whole stack to be running: `docker compose up --build` from the repo root.
export default defineConfig({
  testDir: "e2e",
  timeout: 60_000,
  use: { baseURL: process.env.DASHBOARD_URL ?? "http://localhost:3001" },
});
