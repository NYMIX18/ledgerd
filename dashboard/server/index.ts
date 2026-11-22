import { fileURLToPath } from "node:url";
import path from "node:path";
import { buildApp } from "./app";

const here = path.dirname(fileURLToPath(import.meta.url));

const app = buildApp({
  ledgerdUrl: process.env.LEDGERD_URL ?? "http://localhost:8080",
  apiKey: process.env.LEDGERD_API_KEY ?? "change-me-dev-key",
  staticDir: path.join(here, "../web/dist"),
});

const port = Number(process.env.BFF_PORT ?? 3001);
app.listen({ port, host: "0.0.0.0" }).catch((err) => {
  app.log.error(err);
  process.exit(1);
});
