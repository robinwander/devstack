import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react-swc";
import viteTsConfigPaths from "vite-tsconfig-paths";
import tailwindcss from "@tailwindcss/vite";
import { request } from "undici";
import { Agent } from "undici";
import { Readable } from "stream";
import { homedir } from "os";
import { platform } from "process";

function getSocketPath(): string {
  if (platform === "darwin") {
    return `${homedir()}/Library/Application Support/devstack/daemon/devstackd.sock`;
  }
  return `${homedir()}/.local/share/devstack/daemon/devstackd.sock`;
}

function unixSocketProxy(): Plugin {
  const socketAgent = new Agent({
    connect: {
      socketPath: getSocketPath(),
    },
  });

  return {
    name: "unix-socket-proxy",
    configureServer(server) {
      server.middlewares.use(async (req, res, next) => {
        if (!req.url?.startsWith("/api/")) {
          return next();
        }

        const path = req.url.replace(/^\/api/, "");
        const isEventStream = path.startsWith("/v1/events");

        try {
          let body: string | undefined;
          if (req.method === "POST") {
            const chunks: Buffer[] = [];
            for await (const chunk of req) {
              chunks.push(chunk);
            }
            body = Buffer.concat(chunks).toString();
          }

          const response = await request(`http://localhost${path}`, {
            method: req.method as "GET" | "POST",
            dispatcher: socketAgent,
            body: body,
            headers: body ? { "Content-Type": "application/json" } : undefined,
          });

          res.statusCode = response.statusCode;

          if (isEventStream && response.body) {
            for (const [header, value] of Object.entries(response.headers)) {
              if (value !== undefined) {
                res.setHeader(header, value);
              }
            }
            res.flushHeaders();

            const stream = Readable.fromWeb(response.body as never);
            req.on("close", () => stream.destroy());
            stream.on("error", (error) => {
              if (!res.writableEnded) {
                res.destroy(error);
              }
            });
            stream.pipe(res);
            return;
          }

          const responseBody = await response.body.text();
          res.setHeader(
            "Content-Type",
            response.headers["content-type"] ?? "application/json",
          );
          res.end(responseBody);
        } catch (err) {
          console.error("[proxy] error:", err);
          res.statusCode = 502;
          res.setHeader("Content-Type", "application/json");
          res.end(JSON.stringify({ error: "Daemon unavailable" }));
        }
      });
    },
  };
}

export default defineConfig({
  plugins: [
    viteTsConfigPaths({
      projects: ["./tsconfig.json"],
    }),
    tailwindcss(),
    react(),
    unixSocketProxy(),
  ],
  server: {
    port: 47832,
    allowedHosts: true,
    // HMR websocket fails through reverse proxies (e.g. Tailscale serve)
    // and can stall page rendering on mobile. Since the dashboard is served
    // by the daemon, not a dev workflow, HMR isn't needed.
    hmr: false,
  },
  build: {
    outDir: "dist",
  },
});
