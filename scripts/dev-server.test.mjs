import assert from "node:assert/strict";
import { fork } from "node:child_process";
import { once } from "node:events";
import { fileURLToPath } from "node:url";
import test from "node:test";

if (process.argv.includes("--serve")) {
  const { createServer } = await import("vite");
  const server = await createServer({
    configFile: fileURLToPath(new URL("../vite.config.ts", import.meta.url)),
    logLevel: "error",
    server: { port: 0 },
  });
  await server.listen();
  process.send({ port: server.httpServer.address().port });
} else {
  test(
    "dev server serves the startup HTML, React entry and stylesheet without stalling",
    { timeout: 60000 },
    async () => {
      const child = fork(fileURLToPath(import.meta.url), ["--serve"], {
        stdio: ["ignore", "pipe", "pipe", "ipc"],
        env: { ...process.env, DEBUG: "vite:deps,vite:transform,vite:html" },
      });
      let diagnostics = "";
      child.stdout.on("data", (chunk) => {
        diagnostics += chunk;
      });
      child.stderr.on("data", (chunk) => {
        diagnostics += chunk;
      });
      try {
        const ready = new Promise((resolve, reject) => {
          child.once("message", resolve);
          child.once("error", reject);
          child.once("exit", (code) =>
            reject(new Error(`Vite exited (${code}): ${diagnostics}`)),
          );
        });
        const { port } = await Promise.race([
          ready,
          new Promise((_, reject) => {
            const timer = setTimeout(
              () => reject(new Error(`Vite startup timed out: ${diagnostics}`)),
              15000,
            );
            timer.unref();
          }),
        ]);
        for (const [path, contentType, expected] of [
          ["/", "text/html", "/@react-refresh"],
          ["/src/main.tsx", "javascript", "createRoot"],
          ["/src/styles.css", "javascript", "updateStyle"],
        ]) {
          let response;
          try {
            response = await fetch(`http://127.0.0.1:${port}${path}`, {
              signal: AbortSignal.timeout(12000),
            });
          } catch (cause) {
            throw new Error(
              `Vite did not serve ${path}: ${diagnostics.slice(-5000)}`,
              { cause },
            );
          }
          assert.equal(response.status, 200, `${path}: ${diagnostics}`);
          assert(
            response.headers.get("content-type").includes(contentType),
            path,
          );
          assert((await response.text()).includes(expected), path);
        }
      } finally {
        if (child.exitCode === null && child.signalCode === null) {
          const exited = once(child, "exit");
          child.kill();
          await exited;
        }
      }
    },
  );
}
