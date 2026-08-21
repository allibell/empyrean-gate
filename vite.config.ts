import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { createReadStream, existsSync, readdirSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { basename, join, resolve } from "node:path";

const uprisingDir = process.env.EMPYREAN_UPRISING_DIR
  ? resolve(process.env.EMPYREAN_UPRISING_DIR)
  : join(process.env.XDG_DATA_HOME || join(homedir(), ".local", "share"), "empyrean-gate", "uprising");

function uprisingArchive() {
  return {
    name: "empyrean-uprising-archive",
    configureServer(server: { middlewares: { use: (handler: (req: { url?: string }, res: {
      statusCode: number;
      setHeader: (name: string, value: string) => void;
      end: (body?: string) => void;
    }, next: () => void) => void) => void } }) {
      server.middlewares.use((req, res, next) => {
        const url = new URL(req.url || "/", "http://localhost");
        if (url.pathname === "/__empyrean/uprising") {
          const fixtures = existsSync(uprisingDir)
            ? readdirSync(uprisingDir)
                .filter((name) => name.toLowerCase().endsWith(".eg.data"))
                .sort((a, b) => a.localeCompare(b))
                .map((name) => ({
                  name,
                  size: statSync(join(uprisingDir, name)).size,
                  url: `/__empyrean/uprising/${encodeURIComponent(name)}`,
                }))
            : [];
          res.setHeader("Content-Type", "application/json");
          res.end(JSON.stringify({ directory: uprisingDir, fixtures }));
          return;
        }
        if (url.pathname.startsWith("/__empyrean/uprising/")) {
          const name = basename(decodeURIComponent(url.pathname.slice("/__empyrean/uprising/".length)));
          const file = join(uprisingDir, name);
          if (!name.toLowerCase().endsWith(".eg.data") || !existsSync(file)) {
            res.statusCode = 404;
            res.end("Archive not found");
            return;
          }
          res.setHeader("Content-Type", "application/octet-stream");
          res.setHeader("Content-Length", String(statSync(file).size));
          createReadStream(file).pipe(res as never);
          return;
        }
        next();
      });
    },
  };
}

// Tauri expects a fixed dev port and doesn't want vite to clear the console.
export default defineConfig({
  plugins: [react(), uprisingArchive()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    target: "chrome120",
    sourcemap: true,
  },
});
