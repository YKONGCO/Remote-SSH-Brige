import assert from "node:assert/strict";
import { createServer, connect } from "node:net";
import { once } from "node:events";

export async function runForwardChecks({ call, value, check, tools }) {
  const schema = tools.tools.find((t) => t.name === "port_forward").inputSchema;
  check(
    "Forward schema has four strict actions",
    schema.oneOf.length === 4 &&
      schema.oneOf.every((s) => s.additionalProperties === false),
  );
  for (const args of [
    { action: "open" },
    { action: "open", remote_port: 0 },
    { action: "open", remote_port: 65536 },
    { action: "open", remote_port: 8080, remote_host: "10.0.0.1" },
    { action: "open", remote_port: 8080, local_host: "0.0.0.0" },
    { action: "list", remote_port: 8080 },
    { action: "close" },
  ])
    check(
      "Forward rejects invalid arguments " + JSON.stringify(args),
      (await call("port_forward", args)).error,
    );
  for (const action of ["open", "list", "read", "close"])
    check(
      "Independent forward permission: " + action,
      (
        await call("port_forward", {
          action,
          server_id: "limited-host",
          remote_port: action === "open" ? 8080 : undefined,
          tunnel_id: ["read", "close"].includes(action) ? "missing" : undefined,
        })
      ).error,
    );

  const sockets = new Set();
  const echo = createServer({ allowHalfOpen: true }, (socket) => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
    socket.on("error", () => {});
    socket.on("data", (data) => {
      if (!socket.write(data)) socket.pause();
    });
    socket.on("drain", () => socket.resume());
    socket.on("end", () => socket.end("<EOF>"));
  });
  await new Promise((resolve) => echo.listen(0, "127.0.0.1", resolve));
  let tunnel;
  try {
    tunnel = await value("port_forward", {
      action: "open",
      remote_port: echo.address().port,
    });
    check(
      "Forward listens on loopback with an automatically assigned port",
      tunnel.local_host === "127.0.0.1" &&
        tunnel.local_port > 0 &&
        tunnel.status === "running",
    );
    check(
      "Forward appears in MCP list",
      (await value("port_forward", { action: "list" })).tunnels.some(
        (t) => t.tunnel_id === tunnel.tunnel_id,
      ),
    );
    check(
      "Forward refuses an occupied local port",
      (
        await call("port_forward", {
          action: "open",
          remote_port: echo.address().port,
          local_port: tunnel.local_port,
        })
      ).error,
    );
    check(
      "Forward enforces SSH host fingerprint",
      (
        await call("port_forward", {
          action: "open",
          server_id: "wrong-fingerprint",
          remote_port: echo.address().port,
        })
      ).error,
    );
    check(
      "Forward IDs cannot cross servers",
      (
        await call("port_forward", {
          action: "read",
          server_id: "wrong-fingerprint",
          tunnel_id: tunnel.tunnel_id,
        })
      ).error,
    );
    const exchange = async (payload) => {
      const socket = connect(tunnel.local_port, "127.0.0.1");
      socket.setTimeout(20000, () =>
        socket.destroy(new Error("forward exchange timeout")),
      );
      const chunks = [];
      socket.on("data", (chunk) => chunks.push(chunk));
      socket.end(payload);
      await once(socket, "end");
      assert.deepEqual(
        Buffer.concat(chunks),
        Buffer.concat([payload, Buffer.from("<EOF>")]),
      );
    };
    await Promise.all([
      exchange(Buffer.alloc(2 * 1024 * 1024, 173)),
      exchange(Buffer.from("forward-secret-payload")),
      exchange(Buffer.from("并发转发")),
    ]);
    check(
      "Forward preserves binary bytes, concurrent clients and TCP half close",
      true,
    );
    const info = await value("port_forward", {
      action: "read",
      tunnel_id: tunnel.tunnel_id,
    });
    check(
      "Forward reports both traffic counters",
      info.bytes_sent >= 2 * 1024 * 1024 &&
        info.bytes_received >= info.bytes_sent,
    );
    const active = connect(tunnel.local_port, "127.0.0.1");
    active.on("error", () => {});
    await once(active, "connect");
    let sawClose = false;
    active.on("close", () => {
      sawClose = true;
    });
    await value("port_forward", {
      action: "close",
      tunnel_id: tunnel.tunnel_id,
    });
    let closed;
    for (let i = 0; i < 100; i++) {
      closed = await value("port_forward", {
        action: "read",
        tunnel_id: tunnel.tunnel_id,
      });
      if (closed.status === "closed" && sawClose) break;
      await new Promise((r) => setTimeout(r, 50));
    }
    active.destroy();
    check(
      "Closing forward terminates active clients",
      closed.status === "closed" && sawClose,
    );
    check(
      "Closing an already closed forward is idempotent",
      (
        await value("port_forward", {
          action: "close",
          tunnel_id: tunnel.tunnel_id,
        })
      ).status === "closed",
    );
    check(
      "Forward listing excludes closed tunnels but read retains final metadata",
      !(await value("port_forward", { action: "list" })).tunnels.some(
        (item) => item.tunnel_id === tunnel.tunnel_id,
      ),
    );
    const reuse = createServer();
    await new Promise((resolve, reject) => {
      reuse.once("error", reject);
      reuse.listen(tunnel.local_port, "127.0.0.1", resolve);
    });
    await new Promise((resolve) => reuse.close(resolve));
    check("Closing forward releases the local listening port", true);
  } finally {
    if (tunnel)
      await call("port_forward", {
        action: "close",
        tunnel_id: tunnel.tunnel_id,
      });
    for (const socket of sockets) socket.destroy();
    await new Promise((resolve) => echo.close(resolve));
  }
}
