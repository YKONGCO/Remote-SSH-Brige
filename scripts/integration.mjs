import { DatabaseSync } from "node:sqlite";
// Protocol integration test: a disposable, in-memory SSH/SFTP fixture on loopback.
// No user servers or SSH configuration are contacted.
import assert from "node:assert/strict";
import { generateKeyPairSync, createHash } from "node:crypto";
import { mkdir, writeFile, readFile } from "node:fs/promises";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { connect } from "node:net";
import { runForwardChecks } from "./forward-checks.mjs";
import ssh2 from "ssh2";
const { Server, utils } = ssh2;
const sftpUtils = utils.sftp;
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const work = resolve(root, ".artifacts", "integration-" + Date.now());
await mkdir(work, { recursive: true });
const key = () =>
  generateKeyPairSync("rsa", {
    modulusLength: 2048,
    privateKeyEncoding: { type: "pkcs1", format: "pem" },
    publicKeyEncoding: { type: "spki", format: "pem" },
  }).privateKey;
const hostKey = utils.generateKeyPairSync("ed25519").private,
  clientKey = utils.generateKeyPairSync("ed25519").private;
await writeFile(resolve(work, "client.pem"), clientKey);
const publicKey = utils.parseKey(clientKey).getPublicSSH();
const fingerprint =
  "SHA256:" +
  createHash("sha256")
    .update(utils.parseKey(hostKey).getPublicSSH())
    .digest("base64")
    .replace(/=+$/, "");
const entries = new Map([
  ["/workspace", null],
  ["/workspace/hello.txt", Buffer.from("hello SSH\n")],
  ["/outside", null],
  ["/outside/secret", Buffer.from("restricted")],
  ["/workspace/escape", { link: "/outside/secret" }],
]);
const { STATUS_CODE: STATUS, OPEN_MODE: OPEN } = sftpUtils;
const clients = new Set();
const jobsEnabled = process.env.REMOTE_SSH_TEST_JOBS === "1";
const executionCounts = new Map();
const heldExecutions = new Map();
const fixture = {
  rejectCommit: false,
  dropCommitResponse: false,
  createRace: false,
  commitCalls: 0,
  failDeletePath: null,
  dropDeletePath: null,
  deleteCalls: 0,
  transferReadDelay: 0,
  transferWriteDelay: 0,
  dropTransferRead: false,
};
const canonical = (p) => (p === "/" ? "/" : p.replace(/\/+$/, ""));
const stats = (p) => {
  const e = entries.get(p);
  return {
    mode: e === null ? 0o40755 : e?.link ? 0o120777 : 0o100644,
    uid: 1000,
    gid: 1000,
    size: Buffer.isBuffer(e) ? e.length : 0,
    atime: 1700000000,
    mtime: 1700000000,
  };
};
const ssh = new Server(
  {
    hostKeys: [hostKey],
    algorithms: { kex: ["diffie-hellman-group14-sha256"] },
    debug: process.env.SSH_TEST_DEBUG ? console.log : undefined,
  },
  (client) => {
    client.on("tcpip", (accept, reject, info) => {
      if (!["127.0.0.1", "::1"].includes(info.destIP)) return reject();
      const upstream = connect({ host: info.destIP, port: info.destPort });
      let channel;
      upstream.on("connect", () => {
        channel = accept();
        upstream.pipe(channel).pipe(upstream);
        channel.on("error", () => upstream.destroy());
        channel.on("close", () => upstream.destroy());
      });
      upstream.on("error", () => {
        if (channel) channel.destroy();
        else reject();
      });
    });
    client.setNoDelay(true);
    clients.add(client);
    client.on("close", () => clients.delete(client));
    client.on("error", (e) => {
      if (process.env.SSH_TEST_DEBUG) console.error("SSH fixture:", e.message);
    });
    client.on("authentication", (ctx) => {
      if (
        ctx.method === "publickey" &&
        ctx.key.data.equals(publicKey) &&
        (!ctx.signature ||
          utils
            .parseKey(clientKey)
            .verify(ctx.blob, ctx.signature, ctx.hashAlgo))
      )
        ctx.accept();
      else ctx.reject();
    });
    client.on("ready", () =>
      client.on("session", (accept) => {
        const session = accept();
        session.on("pty", (accept) => accept?.());
        session.on("window-change", (accept) => accept?.());
        session.on("exec", (accept, reject, info) => {
          const stream = accept();
          executionCounts.set(
            info.command,
            (executionCounts.get(info.command) || 0) + 1,
          );
          stream.on("error", () => {});
          if (info.command.startsWith("fixture:job")) {
            heldExecutions.set(info.command, { stream, client });
            stream.write("before ✓\n");
            stream.stderr.write("error before ✓\n");
            return;
          }
          if (info.command === "fixture:streams") {
            stream.write("SSH stdout ✓\n");
            stream.stderr.write("SSH stderr ✓\n");
            stream.exit(7);
            stream.end();
          } else if (info.command === "fixture:timeout") {
            /* Intentionally hold the command open. */
          } else {
            stream.write("SSH command: " + info.command + "\n");
            stream.exit(0);
            stream.end();
          }
        });
        session.on("shell", (accept) => {
          const stream = accept();
          stream.write("fixture PTY ready\r\n$ ");
          let cwd = "/workspace",
            line = "";
          stream.on("data", (bytes) => {
            line += bytes.toString();
            while (line.includes("\n")) {
              const i = line.indexOf("\n"),
                cmd = line.slice(0, i).trim();
              line = line.slice(i + 1);
              if (cmd.startsWith("cd ")) {
                cwd = cmd.slice(3);
                stream.write("$ ");
              } else if (cmd === "pwd") stream.write(cwd + "\r\n$ ");
              else stream.write("echo: " + cmd + "\r\n$ ");
            }
          });
        });
        session.on("sftp", (accept) => {
          const channel = accept();
          channel.on("end", () => channel.end());
          const handles = new Map();
          let next = 1;
          const ok = (id) => channel.status(id, STATUS.OK);
          const fail = (id, code = STATUS.NO_SUCH_FILE) =>
            channel.status(id, code);
          const makeHandle = (id, value) => {
            const h = Buffer.alloc(4);
            h.writeUInt32BE(next++);
            handles.set(h.toString("hex"), value);
            channel.handle(id, h);
          };
          channel.on("REALPATH", (id, p) => {
            p = canonical(p);
            if (!entries.has(p)) return fail(id);
            const e = entries.get(p);
            const dest = e?.link || p;
            channel.name(id, [
              { filename: dest, longname: dest, attrs: stats(dest) },
            ]);
          });
          for (const event of ["STAT", "LSTAT"])
            channel.on(event, (id, p) => {
              p = canonical(p);
              if (!entries.has(p)) return fail(id);
              channel.attrs(id, stats(p));
            });
          channel.on("SETSTAT", (id, p) =>
            entries.has(p) ? ok(id) : fail(id),
          );
          channel.on("FSTAT", (id, h) => {
            const handle = handles.get(h.toString("hex"));
            if (!handle) return fail(id);
            channel.attrs(id, stats(handle.path));
          });
          channel.on("OPENDIR", (id, p) => {
            p = canonical(p);
            if (entries.get(p) !== null) return fail(id);
            makeHandle(id, { path: p, directory: true, done: false });
          });
          channel.on("READDIR", (id, h) => {
            const handle = handles.get(h.toString("hex"));
            if (!handle) return fail(id);
            if (handle.done) return fail(id, STATUS.EOF);
            handle.done = true;
            const prefix = handle.path + "/";
            channel.name(
              id,
              [...entries.keys()]
                .filter(
                  (p) =>
                    p.startsWith(prefix) &&
                    !p.slice(prefix.length).includes("/"),
                )
                .map((p) => ({
                  filename: p.slice(prefix.length),
                  longname: p.slice(prefix.length),
                  attrs: stats(p),
                })),
            );
          });
          channel.on("OPEN", (id, p, flags) => {
            p = canonical(p);
            if (flags & OPEN.CREAT) {
              if (entries.has(p) && flags & OPEN.EXCL)
                return fail(id, STATUS.FAILURE);
              if (!entries.has(p)) entries.set(p, Buffer.alloc(0));
            }
            if (!Buffer.isBuffer(entries.get(p))) return fail(id);
            if (flags & OPEN.TRUNC) entries.set(p, Buffer.alloc(0));
            makeHandle(id, { path: p });
          });
          channel.on("READ", (id, h, offset, length) => {
            const handle = handles.get(h.toString("hex"));
            if (!handle) return fail(id);
            const bytes = entries.get(handle.path);
            if (offset >= bytes.length) return fail(id, STATUS.EOF);
            if (fixture.dropTransferRead) {
              fixture.dropTransferRead = false;
              client.end();
              return;
            }
            const reply = () => {
              if (!channel.destroyed)
                channel.data(id, bytes.subarray(offset, offset + length));
            };
            if (fixture.transferReadDelay)
              setTimeout(reply, fixture.transferReadDelay);
            else reply();
          });
          channel.on("WRITE", (id, h, offset, data) => {
            const handle = handles.get(h.toString("hex"));
            if (!handle) return fail(id);
            const old = entries.get(handle.path);
            const size = Math.max(old.length, offset + data.length);
            if (!handle.backing || handle.backing.length < size) {
              const capacity = Math.max(
                65536,
                size,
                (handle.backing?.length || 0) * 2,
              );
              const backing = Buffer.alloc(capacity);
              old.copy(backing);
              handle.backing = backing;
            }
            data.copy(handle.backing, offset);
            entries.set(handle.path, handle.backing.subarray(0, size));
            if (fixture.transferWriteDelay)
              setTimeout(() => {
                if (!channel.destroyed) ok(id);
              }, fixture.transferWriteDelay);
            else ok(id);
          });
          channel.on("CLOSE", (id, h) => {
            handles.delete(h.toString("hex"));
            ok(id);
          });
          channel.on("MKDIR", (id, p) => {
            p = canonical(p);
            if (entries.has(p)) return fail(id, STATUS.FAILURE);
            entries.set(p, null);
            ok(id);
          });
          channel.on("REMOVE", (id, p) => {
            p = canonical(p);
            fixture.deleteCalls++;
            if (!entries.has(p)) return fail(id);
            if (p === fixture.failDeletePath)
              return fail(id, STATUS.PERMISSION_DENIED);
            entries.delete(p);
            if (p === fixture.dropDeletePath) {
              channel.end();
              return;
            }
            ok(id);
          });
          channel.on("RMDIR", (id, p) => {
            p = canonical(p);
            if (
              entries.get(p) !== null ||
              [...entries.keys()].some((k) => k.startsWith(p + "/"))
            )
              return fail(id, STATUS.FAILURE);
            entries.delete(p);
            ok(id);
          });
          const rename = (id, from, to) => {
            from = canonical(from);
            to = canonical(to);
            if (!entries.has(from)) return fail(id);
            for (const p of [...entries.keys()])
              if (p === from || p.startsWith(from + "/")) {
                entries.set(to + p.slice(from.length), entries.get(p));
                entries.delete(p);
              }
            ok(id);
          };
          channel.on("RENAME", (id, from, to) => {
            if (entries.has(canonical(to))) return fail(id, STATUS.FAILURE);
            rename(id, from, to);
          });
          channel.on("EXTENDED", (id, name, data) => {
            fixture.commitCalls++;
            if (fixture.rejectCommit) return fail(id, STATUS.OP_UNSUPPORTED);
            let cursor = 0;
            const readString = () => {
              const size = data.readUInt32BE(cursor);
              cursor += 4;
              const value = data.subarray(cursor, cursor + size).toString();
              cursor += size;
              return canonical(value);
            };
            const from = readString(),
              to = readString();
            if (name === "hardlink@openssh.com") {
              if (fixture.createRace) {
                entries.set(to, Buffer.from("concurrent-create"));
                fixture.createRace = false;
              }
              if (entries.has(to)) return fail(id, STATUS.FAILURE);
              if (!entries.has(from)) return fail(id);
              entries.set(to, entries.get(from));
              if (fixture.dropCommitResponse) {
                channel.end();
                return;
              }
              ok(id);
            } else if (name === "posix-rename@openssh.com") {
              if (fixture.dropCommitResponse) {
                const moved = entries.get(from);
                entries.delete(from);
                entries.set(to, moved);
                channel.end();
                return;
              }
              rename(id, from, to);
            } else fail(id, STATUS.OP_UNSUPPORTED);
          });
        });
      }),
    );
  },
);
await new Promise((r) => ssh.listen(0, "127.0.0.1", r));
const sshPort = ssh.address().port;
const configServer = {
  id: "test-host",
  name: "Protocol fixture",
  host: "127.0.0.1",
  port: sshPort,
  username: "fixture",
  auth: "key",
  key_path: resolve(work, "client.pem"),
  fingerprint,
  group: "测试",
  color: "blue",
  enabled: true,
  allow_exec: true,
  allow_write: true,
  allow_terminal: true,
  mcp_permissions: [
    "port_forward_open",
    "port_forward_list",
    "port_forward_read",
    "port_forward_close",
    "read_file",
    "list_files",
    "stat_file",
    "write_file",
    "edit_file",
    "remove_path",
    "exec",
    "job",
    "list_terminals",
    "terminal_open",
    "terminal_read",
    "terminal_write",
    "terminal_resize",
    "terminal_close",
  ],
  root: "/workspace",
};
const configServers = [
  configServer,
  { ...configServer, id: "no-job-host", mcp_permissions: ["exec"] },
  {
    ...configServer,
    id: "limited-host",
    name: "Limited host",
    mcp_permissions: [
      "read_file",
      "terminal_open",
      "terminal_read",
      "terminal_close",
    ],
  },
  {
    ...configServer,
    id: "hidden-host",
    name: "Hidden host",
    mcp_enabled: false,
  },
  {
    ...configServer,
    id: "wrong-fingerprint",
    name: "Wrong fingerprint",
    fingerprint: "SHA256:wrong",
  },
];
const settingsDB = new DatabaseSync(resolve(work, "remote-desk.sqlite3"));
settingsDB.exec(
  await readFile(resolve(root, "src-tauri/src/schema.sql"), "utf8"),
);
settingsDB.exec("PRAGMA user_version=2");
const insertServer = settingsDB.prepare(`INSERT INTO servers
  (id,name,host,port,username,auth,key_path,fingerprint,group_name,color,enabled,
   allow_exec,allow_write,allow_terminal,root,mcp_enabled,mcp_permissions_json)
  VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)`);
for (const server of configServers) {
  insertServer.run(
    server.id,
    server.name,
    server.host,
    server.port,
    server.username,
    server.auth,
    server.key_path,
    server.fingerprint,
    server.group,
    server.color,
    Number(server.enabled),
    Number(server.allow_exec),
    Number(server.allow_write),
    Number(server.allow_terminal),
    server.root,
    Number(server.mcp_enabled !== false),
    JSON.stringify(server.mcp_permissions),
  );
}
settingsDB
  .prepare("INSERT INTO metadata(key,value) VALUES('runtime_settings',?)")
  .run(
    JSON.stringify({
      deletion_protection: false,
      block_dangerous_commands: true,
      command_policy: {
        diagnostic_only: false,
        allow_pipes: true,
        allow_redirection: true,
        allow_compound: true,
        allow_scripts: true,
        allow_substitution: true,
        allow_mcp_terminal: true,
      },
      remote_read_only: false,
      approval_mode: "off",
      upload_limit_mib: 4,
      download_limit_mib: 1,
      connect_timeout_secs: 8,
      io_timeout_secs: 15,
      command_timeout_secs: 1,
      exec_timeout_to_job: jobsEnabled,
      max_terminals: 1,
    }),
  );
settingsDB.close();
const binary =
  process.env.REMOTE_SSH_BRIGE_TEST_BINARY ||
  resolve(
    root,
    process.env.CARGO_TARGET_DIR || "src-tauri/target",
    "debug",
    process.platform === "win32"
      ? "remote-ssh-brige-headless.exe"
      : "remote-ssh-brige-headless",
  );
const proc = spawn(binary, [work, "18929"], {
  cwd: root,
  windowsHide: true,
  stdio: ["ignore", "pipe", "pipe"],
});
let stderr = "";
proc.stderr.on("data", (b) => (stderr += b));
proc.on("error", (e) => console.error(e));
const started = Date.now();
while (!stderr.includes('"token"')) {
  if (proc.exitCode !== null) throw new Error("MCP process exited: " + stderr);
  if (Date.now() - started > 15000)
    throw new Error("MCP startup timeout: " + stderr);
  await new Promise((r) => setTimeout(r, 50));
}
const status = JSON.parse(
  stderr.split("\n").find((l) => l.includes('"token"')),
);
const url = new URL("http://127.0.0.1:" + status.port + "/mcp");
const mcp = new Client({
  name: "remote-ssh-brige-integration",
  version: "1.0.0",
});
const transport = new StreamableHTTPClientTransport(url, {
  requestInit: { headers: { Authorization: "Bearer " + status.token } },
});
const checks = [];
const check = (label, value) => {
  assert.ok(value, label);
  checks.push(label);
  console.log("PASS " + label);
};
try {
  const oversizedHeaderStatus = await new Promise((resolve, reject) => {
    const socket = connect({ host: "127.0.0.1", port: status.port });
    socket.setTimeout(5000, () =>
      socket.destroy(new Error("Oversized header rejection timed out")),
    );
    socket.on("error", reject);
    socket.on("connect", () =>
      socket.write(
        `POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer ${status.token}\r\nContent-Length: ${4 * 1024 * 1024 * 6 + 65537}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n`,
      ),
    );
    let response = "";
    socket.on("data", (chunk) => {
      response += chunk.toString();
      if (response.includes("\r\n")) {
        socket.destroy();
        resolve(response.split("\r\n")[0]);
      }
    });
  });
  check(
    "Reject oversized request headers before buffering the body",
    String(oversizedHeaderStatus).includes("413"),
  );
  const unauthorized = await fetch(url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: "{}",
  });
  check("Reject missing bearer token", unauthorized.status === 401);
  check(
    "Missing-token error is specific and English",
    (await unauthorized.text()) === "Missing or invalid MCP bearer token",
  );
  const badToken = await fetch(url, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: "Bearer incorrect",
    },
    body: "{}",
  });
  check("Reject incorrect bearer token", badToken.status === 401);
  const browser = await fetch(url, {
    method: "POST",
    headers: {
      Origin: "https://untrusted.example",
      Authorization: "Bearer " + status.token,
      "Content-Type": "application/json",
    },
    body: "{}",
  });
  check("Reject browser Origin", browser.status === 403);
  check(
    "Origin error is specific and English",
    (await browser.text()) === "Browser-origin MCP requests are not allowed",
  );
  await mcp.connect(transport);
  check(
    "MCP reports the new service name and title",
    mcp.getServerVersion()?.name === "remote-ssh" &&
      mcp.getServerVersion()?.title === "Remote SSH Brige",
  );
  const tools = await mcp.listTools();
  check(
    "MCP exposes exactly 14 focused tools",
    JSON.stringify(tools.tools.map((t) => t.name).sort()) ===
      JSON.stringify(
        [
          "list_servers",
          "exec",
          "job",
          "port_forward",
          "upload_file",
          "download_file",
          "transfer",
          "read_file",
          "list_files",
          "stat_file",
          "write_file",
          "edit_file",
          "remove_path",
          "terminal",
        ].sort(),
      ),
  );
  const call = async (name, args = {}) => {
    try {
      const result = await mcp.callTool({
        name,
        arguments:
          name === "list_servers" ||
          (name === "terminal" && args.action === "list")
            ? args
            : { server_id: "test-host", ...args },
      });
      if (result.isError) {
        assert.equal(
          result.structuredContent,
          undefined,
          "Error payload must not be duplicated",
        );
        assert.equal(result.content.length, 1);
        const text = result.content[0].text;
        assert.equal(typeof text, "string");
        assert.ok(text.length > 0);
        assert.ok([...text].length <= 240);
        assert.ok(!text.includes("\n"));
        assert.ok(!text.includes('"command_policy"'));
        assert.ok(!text.startsWith("{"));
        assert.ok(!/[\u3400-\u9fff]/u.test(text), text);
      }
      return {
        error: result.isError,
        value: result.content?.map((c) => c.text || "").join(""),
        metadata: result.structuredContent,
      };
    } catch (error) {
      if (error?.code === "ERR_ASSERTION") throw error;
      return { error: true, value: String(error) };
    }
  };
  const value = async (name, args) => {
    const r = await call(name, args);
    assert.ok(!r.error, r.value);
    return name === "read_file"
      ? { content: r.value, ...r.metadata }
      : name === "terminal" && args?.action === "read"
        ? { stdout: r.value, ...r.metadata }
        : JSON.parse(r.value);
  };
  const jobSchema = tools.tools.find((t) => t.name === "job").inputSchema;
  check(
    "Unknown tool error identifies the requested tool",
    (await call("missing_tool")).value.includes("Unknown tool: missing_tool"),
  );
  await runForwardChecks({ call, value, check, tools });
  check(
    "Job schema exposes exactly list/read/close",
    jobSchema.type === "object" &&
      jobSchema.oneOf.length === 3 &&
      jobSchema.oneOf.every((s) => s.additionalProperties === false),
  );
  for (const args of [
    { action: "status" },
    { action: "read" },
    { action: "list", job_id: "irrelevant" },
    { action: "close", job_id: "missing", wait_secs: 1 },
  ]) {
    check(
      "Job rejects invalid action fields: " + JSON.stringify(args),
      (await call("job", args)).error,
    );
  }
  for (const [name, expected] of Object.entries({
    list_servers: [],
    read_file: ["server_id", "path", "offset", "limit"],
    list_files: ["server_id", "path", "offset", "limit"],
    stat_file: ["server_id", "path"],
    write_file: ["server_id", "path", "content", "mode", "create_parents"],
    edit_file: ["server_id", "path", "edits"],
    exec: ["server_id", "command", "timeout_secs"],
    upload_file: ["server_id", "local_path", "remote_path", "overwrite"],
    download_file: ["server_id", "local_path", "remote_path", "overwrite"],
    remove_path: ["server_id", "path", "recursive", "missing_ok"],
  })) {
    const schema = tools.tools.find((tool) => tool.name === name).inputSchema;
    check(
      name + " exposes only its own parameters",
      JSON.stringify(Object.keys(schema.properties || {}).sort()) ===
        JSON.stringify(expected.sort()) &&
        schema.additionalProperties === false,
    );
  }
  for (const [name, args] of [
    ["list_servers", { server_id: "irrelevant" }],
    ["read_file", {}],
    ["write_file", { path: "/workspace/new" }],
    ["read_file", { path: "/workspace/hello.txt", command: "irrelevant" }],
    ["read_file", { path: "/workspace/hello.txt", offset: 0 }],
    ["terminal", {}],
    ["terminal", { action: "unknown" }],
    ["terminal", { action: "list", server_id: "irrelevant" }],
    ["terminal", { action: "open", cols: 0 }],
    ["terminal", { action: "write", session_id: "s" }],
    ["terminal", { action: "read", session_id: "s", content: "irrelevant" }],
    ["terminal", { action: "resize", session_id: "s", cols: 100 }],
    ["exec", { command: "pwd", timeout_secs: 301 }],
    ["edit_file", { path: "/workspace/hello.txt", edits: [] }],
    ["remove_path", {}],
    ["remove_path", { path: "/workspace/file", recursive: "true" }],
    ["remove_path", { path: "/workspace/file", force: true }],
  ])
    check(
      "Reject invalid " + name + " arguments: " + JSON.stringify(args),
      (await call(name, args)).error,
    );
  const terminalSchema = tools.tools.find(
    (tool) => tool.name === "terminal",
  ).inputSchema;
  check(
    "Terminal schema is discriminated by action",
    terminalSchema.type === "object" &&
      terminalSchema.oneOf.length === 6 &&
      terminalSchema.oneOf.every(
        (variant) =>
          variant.additionalProperties === false &&
          variant.required.includes("action"),
      ),
  );
  for (const oldName of [
    "mkdir",
    "rename",
    "remove",
    "upload",
    "download",
    "list_terminals",
    "terminal_open",
    "terminal_read",
    "terminal_write",
    "terminal_resize",
    "terminal_close",
  ]) {
    check(
      "Retired tool is not callable: " + oldName,
      (await call(oldName, {})).error,
    );
  }
  const hosts = await value("list_servers", {});
  const listing = await value("list_files", { path: "/workspace", limit: 1 });
  check(
    "Directory listing is paginated and contains metadata",
    listing.entries.length === 1 &&
      listing.has_more &&
      listing.next_offset === 1 &&
      typeof listing.entries[0].permissions === "string",
  );
  const metadata = await value("stat_file", { path: "/workspace/hello.txt" });
  check(
    "Stat reads metadata without file content",
    metadata.kind === "file" &&
      metadata.size === 10 &&
      !Object.hasOwn(metadata, "content"),
  );
  const missingParents = await call("write_file", {
    path: "/workspace/new-parents/a/test.txt",
    content: "nested",
  });
  check(
    "write_file leaves missing parents untouched by default",
    missingParents.error &&
      missingParents.value.includes("Parent directory") &&
      !entries.has("/workspace/new-parents"),
  );
  await value("write_file", {
    path: "/workspace/new-parents/a/test.txt",
    content: "nested",
    create_parents: true,
    mode: "create",
  });
  check(
    "write_file creates nested parents when explicitly requested",
    entries.get("/workspace/new-parents") === null &&
      entries.get("/workspace/new-parents/a/test.txt").toString() === "nested",
  );
  const outsideParents = await call("write_file", {
    path: "/outside/new-parents/test.txt",
    content: "denied",
    create_parents: true,
  });
  check(
    "create_parents rejects outside-root ancestors before mutation",
    outsideParents.error &&
      outsideParents.value.includes("allowed file root") &&
      !entries.has("/outside/new-parents"),
  );
  const deniedListing = await call("list_files", {
    server_id: "limited-host",
    path: "/workspace",
  });
  check(
    "Permission errors identify the missing grant in one sentence",
    deniedListing.error &&
      deniedListing.value.includes("Missing MCP permission") &&
      deniedListing.value.includes("list_files"),
  );
  const invalidPath = await call("stat_file", { path: "relative/path" });
  check(
    "Path validation errors identify the invalid parameter",
    invalidPath.error &&
      invalidPath.value.includes("path must be an absolute POSIX path"),
  );
  check(
    "Discovery exposes independently configured Shell policy",
    hosts[0].security.command_policy.allow_pipes &&
      hosts[0].security.command_policy.allow_scripts &&
      hosts[0].security.command_protection,
  );
  for (const command of [
    "ls | head",
    "echo hello > /workspace/output.txt",
    "pwd && ls",
    "bash -c 'echo hello'",
    "echo $(pwd)",
  ]) {
    const result = await call("exec", { command });
    check(
      `Enabled Shell feature reaches SSH: ${command}`,
      !result.error && JSON.parse(result.value).stdout.includes(command),
    );
  }
  for (const command of [
    "rm -rf /workspace/test",
    "ls | rm -rf /workspace/test",
    "bash -c 'rm -rf /workspace/test'",
  ]) {
    const result = await call("exec", { command });
    check(
      `High-risk command stays blocked with Shell enabled: ${command}`,
      result.error && result.value.includes("Dangerous"),
    );
  }
  check(
    "Discover configured host without credentials",
    hosts.length ===
      configServers.filter((s) => s.mcp_enabled !== false).length &&
      !JSON.stringify(hosts).includes("client.pem"),
  );
  check(
    "MCP-disabled host is hidden from discovery",
    !hosts.some((host) => host.id === "hidden-host"),
  );
  const hidden = await call("read_file", {
    server_id: "hidden-host",
    path: "/workspace/hello.txt",
  });
  check(
    "MCP-disabled host rejects calls by known ID",
    hidden.error && hidden.value.includes("Missing MCP permission"),
  );
  const limitedHost = hosts.find((host) => host.id === "limited-host");
  check(
    "Discovery exposes precise tool permissions",
    limitedHost.mcp_permissions.includes("read_file") &&
      !limitedHost.mcp_permissions.includes("upload") &&
      !limitedHost.mcp_permissions.includes("exec") &&
      JSON.stringify(limitedHost.terminal_actions) ===
        JSON.stringify(["open", "read", "close"]),
  );
  entries.set("/workspace/limited.txt", Buffer.from("limited read"));
  const limitedText = await value("read_file", {
    server_id: "limited-host",
    path: "/workspace/limited.txt",
  });
  check(
    "Limited file grants do not imply shell permission",
    limitedText.content === "limited read" && !limitedHost.allow_exec,
  );
  for (const [kind, args] of [
    ["write_file", { path: "/workspace/limited.txt", content: "denied" }],
    ["exec", { command: "fixture:streams" }],
    [
      "terminal",
      { action: "write", session_id: "denied-session", content: "denied" },
    ],
  ]) {
    const denied = await call(kind, { server_id: "limited-host", ...args });
    check(
      "Independent permission denies " + kind,
      denied.error && denied.value.includes("Missing MCP permission"),
    );
  }
  const mismatch = await call("read_file", {
    server_id: "wrong-fingerprint",
    path: "/workspace/hello.txt",
  });
  check(
    "Reject changed SSH host fingerprint before authentication",
    mismatch.error && mismatch.value.includes("fingerprint mismatch"),
  );
  const exec = await value("exec", { command: "fixture:streams" });
  check(
    "Real SSH exec captures stdout, stderr and nonzero exit",
    exec.exit_code === 7 &&
      exec.stdout.includes("stdout ✓") &&
      exec.stderr.includes("stderr ✓"),
  );
  for (const command of [
    "ls -la /workspace",
    "mv /workspace/from /workspace/to",
    "mkdir /workspace/new-dir",
    "find /workspace -type f",
    "grep -n port /workspace/config",
    "base64 /workspace/binary.bin",
  ]) {
    const result = await value("exec", { command });
    check(
      "Use exec instead of a dedicated tool: " + command,
      result.stdout.includes(command),
    );
  }
  const text = await value("read_file", { path: "/workspace/hello.txt" });
  check("Read remote UTF-8 file", text.content === "hello SSH\n");
  await value("write_file", {
    path: "/workspace/new.txt",
    content: "原子写入 ✓",
  });
  const written = await value("read_file", { path: "/workspace/new.txt" });
  check("Atomic SFTP create and read", written.content === "原子写入 ✓");
  await value("write_file", {
    path: "/workspace/new.txt",
    content: "updated",
  });
  check(
    "Atomic SFTP replace existing file",
    (await value("read_file", { path: "/workspace/new.txt" })).content ===
      "updated",
  );
  await value("write_file", {
    path: "/workspace/page.txt",
    content: "一\r\n二\r\n三\r\n",
  });
  const page = await call("read_file", {
    path: "/workspace/page.txt",
    limit: 2,
  });
  check(
    "Read returns raw text with continuation metadata",
    !page.error &&
      page.value === "一\r\n二\r\n" &&
      page.metadata.next_offset === 3,
  );
  check(
    "Read continuation preserves original bytes",
    (await value("read_file", { path: "/workspace/page.txt", offset: 3 }))
      .content === "三\r\n",
  );
  await value("write_file", {
    path: "/workspace/edit.txt",
    content: "keep “quotes”  \r\nport=80\r\ndebug=true\r\n",
  });
  const edited = await value("edit_file", {
    path: "/workspace/edit.txt",
    edits: [
      { old_text: "port=80", new_text: "port=90" },
      { old_text: "debug=true", new_text: "debug=false" },
    ],
  });
  check(
    "Batch edit commits once and returns diff",
    edited.applied_edits === 2 && edited.diff.includes("+port=90"),
  );
  check(
    "Batch edit preserves CRLF and untouched text",
    entries.get("/workspace/edit.txt").toString() ===
      "keep “quotes”  \r\nport=90\r\ndebug=false\r\n",
  );
  const beforeFailure = Buffer.from(entries.get("/workspace/edit.txt"));
  const failedBatch = await call("edit_file", {
    path: "/workspace/edit.txt",
    edits: [
      { old_text: "port=90", new_text: "port=99" },
      { old_text: "missing-text", new_text: "never-committed-secret" },
    ],
  });
  check(
    "Failed batch leaves original file intact",
    failedBatch.error &&
      failedBatch.value.includes("Text not found") &&
      beforeFailure.equals(entries.get("/workspace/edit.txt")),
  );
  const deniedEdit = await call("edit_file", {
    server_id: "limited-host",
    path: "/workspace/limited.txt",
    edits: [{ old_text: "limited", new_text: "denied-edit" }],
  });
  check(
    "Edit permission is independent of shell access",
    deniedEdit.error && deniedEdit.value.includes("Missing MCP permission"),
  );
  await value("write_file", {
    path: "/workspace/create-only.txt",
    content: "created",
    mode: "create",
  });
  check(
    "Create-only mode refuses overwrite",
    (
      await call("write_file", {
        path: "/workspace/create-only.txt",
        content: "overwritten",
        mode: "create",
      })
    ).error &&
      entries.get("/workspace/create-only.txt").toString() === "created",
  );
  fixture.createRace = true;
  check(
    "Atomic create refuses a concurrent creator",
    (
      await call("write_file", {
        path: "/workspace/create-race.txt",
        content: "must-not-win",
        mode: "create",
      })
    ).error &&
      entries.get("/workspace/create-race.txt").toString() ===
        "concurrent-create",
  );
  fixture.rejectCommit = true;
  const unsupported = await call("write_file", {
    path: "/workspace/edit.txt",
    content: "must-not-commit",
  });
  fixture.rejectCommit = false;
  check(
    "Unsupported atomic write fails without touching destination",
    unsupported.error &&
      unsupported.value.includes("atomic SFTP extension is unsupported") &&
      beforeFailure.equals(entries.get("/workspace/edit.txt")),
  );
  fixture.dropCommitResponse = true;
  const commitCount = fixture.commitCalls;
  const uncertain = await call("write_file", {
    path: "/workspace/uncertain.txt",
    content: "committed-without-ack",
  });
  fixture.dropCommitResponse = false;
  check(
    "Lost commit response is reported without retry",
    uncertain.error &&
      uncertain.value.includes("read back before retrying") &&
      fixture.commitCalls === commitCount + 1 &&
      entries.get("/workspace/uncertain.txt").toString() ===
        "committed-without-ack",
  );
  await value("write_file", {
    path: "/workspace/concurrent.txt",
    content: "a=1\nb=1\n",
  });
  const parallel = await Promise.all([
    call("edit_file", {
      path: "/workspace/concurrent.txt",
      edits: [{ old_text: "a=1", new_text: "a=2" }],
    }),
    call("edit_file", {
      path: "/workspace/concurrent.txt",
      edits: [{ old_text: "b=1", new_text: "b=2" }],
    }),
  ]);
  check(
    "Concurrent edits preserve both changes",
    parallel.every((r) => !r.error) &&
      entries.get("/workspace/concurrent.txt").toString() === "a=2\nb=2\n",
  );
  check(
    "Temporary files are cleaned up",
    ![...entries.keys()].some((p) => p.includes(".remote-ssh-brige-")),
  );
  entries.set("/workspace/binary.bin", Buffer.from([0, 255, 16, 128]));
  await value("write_file", {
    path: "/workspace/large.txt",
    content: "x".repeat(3 * 1024 * 1024),
  });
  check(
    "Configured write limit permits large text",
    entries.get("/workspace/large.txt").length === 3 * 1024 * 1024,
  );
  const tooLarge = await call("write_file", {
    path: "/workspace/too-large.txt",
    content: "x".repeat(4 * 1024 * 1024 + 1),
  });
  check(
    "Configured write limit rejects oversized text",
    tooLarge.error && !entries.has("/workspace/too-large.txt"),
  );
  check(
    "Deny traversal outside configured root",
    (await call("read_file", { path: "/workspace/../outside/secret" })).error,
  );
  check(
    "Deny symlink escape outside configured root",
    (await call("read_file", { path: "/workspace/escape" })).error,
  );
  await value("write_file", {
    path: "/workspace/mcp_test_remote_ssh.txt",
    content: "",
  });
  const cleanup = await value("remove_path", {
    path: "/workspace/mcp_test_remote_ssh.txt",
  });
  check(
    "Safe removal requires deletion protection off while command protection stays on",
    cleanup.removed &&
      cleanup.removed_entries === 1 &&
      !entries.has("/workspace/mcp_test_remote_ssh.txt") &&
      !hosts[0].security.deletion_protection &&
      hosts[0].security.command_protection,
  );
  const repeated = await value("remove_path", {
    path: "/workspace/mcp_test_remote_ssh.txt",
  });
  check(
    "Safe cleanup is idempotent",
    repeated.missing && !repeated.removed && repeated.removed_entries === 0,
  );
  check(
    "Strict missing mode reports a missing target",
    /not found|no such|不存在/i.test(
      (
        await call("remove_path", {
          path: "/workspace/mcp_test_remote_ssh.txt",
          missing_ok: false,
        })
      ).value,
    ),
  );
  entries.set("/workspace/delete-tree", null);
  entries.set("/workspace/delete-tree/a", Buffer.from("a"));
  entries.set("/workspace/delete-tree/sub", null);
  entries.set("/workspace/delete-tree/sub/b", Buffer.from("b"));
  entries.set("/workspace/delete-tree/outside-link", {
    link: "/outside/secret",
  });
  const nonEmpty = await call("remove_path", {
    path: "/workspace/delete-tree",
  });
  check(
    "Non-empty directory requires explicit recursion",
    nonEmpty.error &&
      nonEmpty.value.includes("not empty") &&
      entries.has("/workspace/delete-tree/a"),
  );
  const removedTree = await value("remove_path", {
    path: "/workspace/delete-tree",
    recursive: true,
  });
  check(
    "Recursive deletion unlinks symlinks instead of their targets",
    removedTree.removed_entries === 5 &&
      !entries.has("/workspace/delete-tree") &&
      entries.has("/outside/secret"),
  );
  entries.set("/workspace/empty-dir", null);
  check(
    "Empty directories need no recursive flag",
    (await value("remove_path", { path: "/workspace/empty-dir" })).kind ===
      "directory",
  );
  entries.set("/workspace/dangling-link", { link: "/outside/absent" });
  check(
    "Dangling link itself is removed",
    (await value("remove_path", { path: "/workspace/dangling-link" })).kind ===
      "symlink",
  );
  const outside = await call("remove_path", { path: "/outside/secret" });
  check(
    "Safe removal refuses root and outside paths",
    (await call("remove_path", { path: "/workspace", recursive: true }))
      .error &&
      outside.error &&
      entries.has("/outside/secret"),
  );
  const deniedRemoval = await call("remove_path", {
    server_id: "limited-host",
    path: "/workspace/limited.txt",
  });
  check(
    "Safe deletion requires a new explicit permission",
    deniedRemoval.error &&
      deniedRemoval.value.includes("Missing MCP permission") &&
      entries.has("/workspace/limited.txt"),
  );
  entries.set("/workspace/partial", null);
  entries.set("/workspace/partial/a", Buffer.from("a"));
  entries.set("/workspace/partial/b", Buffer.from("b"));
  fixture.failDeletePath = "/workspace/partial/b";
  const partial = await call("remove_path", {
    path: "/workspace/partial",
    recursive: true,
  });
  fixture.failDeletePath = null;
  check(
    "Recursive deletion reports partial progress on server denial",
    partial.error &&
      partial.value.includes("removed entries: 1") &&
      !entries.has("/workspace/partial/a") &&
      entries.has("/workspace/partial/b"),
  );
  await value("remove_path", { path: "/workspace/partial", recursive: true });
  entries.set("/workspace/delete-unknown", Buffer.from("temp"));
  fixture.dropDeletePath = "/workspace/delete-unknown";
  const deleteCalls = fixture.deleteCalls;
  const unknownDelete = await call("remove_path", {
    path: "/workspace/delete-unknown",
  });
  fixture.dropDeletePath = null;
  check(
    "Lost delete response reports unknown outcome without retry",
    unknownDelete.error &&
      unknownDelete.value.includes("outcome unknown") &&
      fixture.deleteCalls === deleteCalls + 1 &&
      !entries.has("/workspace/delete-unknown"),
  );
  check(
    "Cleanup can be safely retried after inspecting unknown outcome",
    (await value("remove_path", { path: "/workspace/delete-unknown" })).missing,
  );
  const terminal = await value("terminal", {
    action: "open",
    cols: 100,
    rows: 30,
  });
  check(
    "Configured session limit rejects additional terminals",
    (await call("terminal", { action: "open" })).error,
  );
  await value("terminal", {
    action: "write",
    session_id: terminal.session_id,
    content: "cd /workspace/kept\n",
  });
  await value("terminal", {
    action: "write",
    session_id: terminal.session_id,
    content: "pwd\n",
  });
  let output = "";
  for (let n = 0; n < 30; n++) {
    const r = await value("terminal", {
      action: "read",
      session_id: terminal.session_id,
      cursor: 0,
    });
    output = r.stdout;
    if (output.includes("/workspace/kept")) break;
    await new Promise((r) => setTimeout(r, 50));
  }
  check(
    "Persistent SSH PTY retains working directory between tool calls",
    output.includes("/workspace/kept"),
  );
  const terminals = await value("terminal", { action: "list" });
  check(
    "Discover persistent terminal sessions",
    terminals.some((t) => t.id === terminal.session_id),
  );
  await value("terminal", {
    action: "resize",
    session_id: terminal.session_id,
    cols: 120,
    rows: 40,
  });
  await value("terminal", {
    action: "close",
    session_id: terminal.session_id,
  });
  check(
    "Resize and close persistent terminal",
    (await value("terminal", { action: "list" })).length === 0,
  );
  const limitedTerminal = await value("terminal", {
    action: "open",
    server_id: "limited-host",
    cols: 100,
    rows: 30,
  });
  const visibleTerminals = await value("terminal", { action: "list" });
  check(
    "Terminal discovery respects independent list permission",
    !visibleTerminals.some(
      (session) => session.id === limitedTerminal.session_id,
    ),
  );
  const limitedOutput = await value("terminal", {
    action: "read",
    server_id: "limited-host",
    session_id: limitedTerminal.session_id,
    cursor: 0,
  });
  check(
    "Terminal output can be read without input permission",
    typeof limitedOutput.stdout === "string" &&
      !Object.hasOwn(limitedOutput, "base64"),
  );
  const deniedInput = await call("terminal", {
    action: "write",
    server_id: "limited-host",
    session_id: limitedTerminal.session_id,
    content: "pwd\n",
  });
  check(
    "Existing terminal input is denied without permission",
    deniedInput.error && deniedInput.value.includes("Missing MCP permission"),
  );
  await value("terminal", {
    action: "close",
    server_id: "limited-host",
    session_id: limitedTerminal.session_id,
  });
  if (!jobsEnabled) {
    check(
      "Command timeout enforced",
      (await call("exec", { command: "fixture:timeout", timeout_secs: 1 }))
        .error,
    );
    check(
      "Configured default command timeout applies when omitted",
      (await call("exec", { command: "fixture:timeout" })).error,
    );
    const timed = await call("exec", {
      command: "fixture:job-disabled",
      timeout_secs: 1,
    });
    check(
      "Timeout error returns only its concise reason",
      timed.error &&
        timed.value.includes("job") &&
        !timed.value.includes("before ✓"),
    );
  } else {
    const notifications = [];
    const result = await mcp.callTool(
      {
        name: "exec",
        arguments: {
          server_id: "test-host",
          command: "fixture:job-incremental",
          timeout_secs: 1,
        },
      },
      undefined,
      { onprogress: (p) => notifications.push(p) },
    );
    check("Timeout handoff is a successful tool result", !result.isError);
    const retained = JSON.parse(result.content[0].text);
    check(
      "Retained execution returns output, byte cursors and running state",
      retained.status === "running" &&
        retained.stdout === "before ✓\n" &&
        retained.stderr === "error before ✓\n" &&
        retained.next_stdout_cursor === Buffer.byteLength(retained.stdout),
    );
    check(
      "Foreground exec sends bounded progress notifications",
      notifications.length > 0 &&
        notifications.every((p) => p.message.length < 60000),
    );
    const progressCount = notifications.length;
    const held = heldExecutions.get("fixture:job-incremental");
    setTimeout(() => {
      held.stream.write("after ✓\n");
      held.stream.stderr.write("error after ✓\n");
    }, 100);
    const readStarted = Date.now();
    const incremental = await value("job", {
      action: "read",
      job_id: retained.job_id,
      stdout_cursor: retained.next_stdout_cursor,
      stderr_cursor: retained.next_stderr_cursor,
      wait_secs: 10,
    });
    check(
      "job.read wakes early and returns only incremental bytes",
      Date.now() - readStarted < 2000 &&
        incremental.stdout === "after ✓\n" &&
        incremental.stderr === "error after ✓\n",
    );
    held.stream.exit(9);
    held.stream.end();
    const final = await value("job", {
      action: "read",
      job_id: retained.job_id,
      stdout_cursor: incremental.next_stdout_cursor,
      stderr_cursor: incremental.next_stderr_cursor,
      wait_secs: 10,
    });
    check(
      "Final nonzero exit code remains available without repeated execution",
      final.status === "exited" &&
        final.exit_code === 9 &&
        final.stdout === "" &&
        executionCounts.get("fixture:job-incremental") === 1,
    );
    check(
      "Original exec progress stops at handoff",
      notifications.length === progressCount,
    );
    check(
      "Finished sessions remain listed",
      (await value("job", { action: "list" })).jobs.some(
        (j) => j.job_id === retained.job_id && j.status === "exited",
      ),
    );
    check(
      "Future cursors are rejected",
      (
        await call("job", {
          action: "read",
          job_id: retained.job_id,
          stdout_cursor: final.next_stdout_cursor + 1,
        })
      ).error,
    );
    check(
      "Job access checks server permission",
      (
        await call("job", {
          server_id: "no-job-host",
          action: "read",
          job_id: retained.job_id,
        })
      ).error,
    );
    check(
      "Job IDs cannot be accessed through another server",
      (
        await call("job", {
          server_id: "wrong-fingerprint",
          action: "read",
          job_id: retained.job_id,
        })
      ).error,
    );
    check(
      "Invalid wait duration is rejected",
      (
        await call("job", {
          action: "read",
          job_id: retained.job_id,
          wait_secs: 31,
        })
      ).error,
    );
    const denied = await call("exec", {
      server_id: "no-job-host",
      command: "fixture:job-denied",
      timeout_secs: 1,
    });
    check(
      "Handoff without job authorization closes with explicit reason",
      denied.error && denied.value.includes("job"),
    );
    const high = await value("exec", {
      command: "fixture:job-high",
      timeout_secs: 1,
    });
    const highStream = heldExecutions.get("fixture:job-high").stream;
    const block = Buffer.alloc(5 * 1024 * 1024, "x");
    const write = (stream, bytes) =>
      new Promise((resolve) => stream.write(bytes, resolve));
    let highTimer;
    await Promise.race([
      (async () => {
        // The ssh2 fixture server can starve a pending stderr write when two
        // multi-megabyte writes exhaust the same SSH window. Produce realistic
        // interleaved chunks while leaving job.read entirely idle.
        for (let offset = 0; offset < block.length; offset += 8192) {
          const chunk = block.subarray(offset, offset + 8192);
          await write(highStream, chunk);
          await write(highStream.stderr, chunk);
          await new Promise((resolve) => setTimeout(resolve, 2));
        }
      })(),
      new Promise((_, reject) => {
        highTimer = setTimeout(
          async () =>
            reject(
              new Error(
                "High output write stalled: " +
                  JSON.stringify(await value("job", { action: "list" })),
              ),
            ),
          15000,
        );
      }),
    ]).finally(() => clearTimeout(highTimer));
    highStream.exit(0);
    highStream.end();
    let highFinal;
    for (let i = 0; i < 100; i++) {
      highFinal = await value("job", {
        action: "read",
        job_id: high.job_id,
        stdout_cursor: 0,
        stderr_cursor: 0,
      });
      if (highFinal.status !== "running") break;
      await new Promise((r) => setTimeout(r, 20));
    }
    check(
      "Both streams drain without readers and retain only 4 MiB each",
      highFinal.status === "exited" &&
        highFinal.stdout.length === 4 * 1024 * 1024 &&
        highFinal.stderr.length === 4 * 1024 * 1024 &&
        highFinal.stdout_truncated &&
        highFinal.stderr_truncated &&
        highFinal.next_stdout_cursor === high.next_stdout_cursor + block.length,
    );
    const active = await Promise.all(
      Array.from({ length: 4 }, (_, i) =>
        value("exec", { command: "fixture:job-active-" + i, timeout_secs: 1 }),
      ),
    );
    const full = await call("exec", {
      command: "fixture:job-over-limit",
      timeout_secs: 1,
    });
    check(
      "Per-server active job limit is enforced with reason",
      full.error && full.value.includes("4"),
    );
    const closed = await value("job", {
      action: "close",
      job_id: active[0].job_id,
    });
    check(
      "Close confirms closed with unknown exit code and preserves output",
      closed.status === "closed" &&
        closed.exit_code === null &&
        closed.stdout === "before ✓\n",
    );
    check(
      "Closing a completed session preserves its result",
      (await value("job", { action: "close", job_id: retained.job_id }))
        .exit_code === 9,
    );
    const disconnectedPromise = value("job", {
      action: "read",
      job_id: active[1].job_id,
      stdout_cursor: active[1].next_stdout_cursor,
      stderr_cursor: active[1].next_stderr_cursor,
      wait_secs: 10,
    });
    heldExecutions.get("fixture:job-active-1").client.end();
    const disconnected = await disconnectedPromise;
    check(
      "SSH disconnect has unknown final result",
      disconnected.status === "disconnected" && disconnected.exit_code === null,
    );
    await Promise.all(
      active
        .slice(2)
        .map((j) => value("job", { action: "close", job_id: j.job_id })),
    );
    await new Promise((r) => setTimeout(r, 50));
  }
  const auditDB = new DatabaseSync(resolve(work, "remote-desk.sqlite3"), {
    readOnly: true,
  });
  const logs = auditDB
    .prepare("SELECT * FROM audit_entries ORDER BY sequence")
    .all()
    .map((row) => ({
      ...row,
      response: row.response_json
        ? JSON.parse(String(row.response_json))
        : undefined,
    }));
  auditDB.close();
  check(
    "Shared audit records success and failed operations",
    logs.some((l) => l.kind === "write_file" && l.success) &&
      logs.some((l) => !l.success) &&
      logs.some((l) => l.kind === "terminal_open"),
  );
  check(
    "Audit excludes written file contents and terminal keystrokes",
    !JSON.stringify(logs).includes("原子写入") &&
      !JSON.stringify(logs).includes("cd /workspace/kept") &&
      !JSON.stringify(logs).includes("never-committed-secret") &&
      !JSON.stringify(logs.filter((l) => l.kind === "edit_file")).includes(
        "+port=90",
      ),
  );
  check(
    "Forward audit records mapping metadata without traffic contents",
    logs.some(
      (l) => l.kind === "port_forward_open" && l.response?.data?.local_port > 0,
    ) &&
      logs.some((l) => l.kind === "port_forward_close") &&
      !JSON.stringify(logs).includes("forward-secret-payload"),
  );
  const recordedExec = logs.find(
    (entry) => entry.kind === "exec" && entry.response?.data?.exit_code === 7,
  );
  check(
    "Audit persists SSH stdout, stderr and exit code",
    recordedExec?.response.data.stdout.includes("stdout ✓") &&
      recordedExec?.response.data.stderr.includes("stderr ✓"),
  );
  if (jobsEnabled) {
    const handoff = logs.find(
      (l) =>
        l.kind === "exec" &&
        l.response?.data?.job_id &&
        l.detail === "fixture:job-incremental",
    );
    const finished = logs.find(
      (l) =>
        l.kind === "job" &&
        l.response?.data?.job_id === handoff?.response?.data?.job_id &&
        l.response?.data?.status === "exited",
    );
    check(
      "Audit links successful handoff to nonzero final result",
      handoff?.success &&
        !finished?.success &&
        finished?.response?.data?.exit_code === 9 &&
        handoff.sequence < finished.sequence,
    );
    check(
      "Audit records close and disconnect without invented exit codes",
      ["closed", "disconnected"].every((status) =>
        logs.some(
          (l) =>
            l.kind === "job" &&
            l.response?.data?.status === status &&
            l.response.data.exit_code === null,
        ),
      ),
    );
  }
  const deniedTransport = new StreamableHTTPClientTransport(url, {
    requestInit: { headers: { Authorization: "Bearer wrong-token" } },
  });
  let denied = false;
  try {
    await new Client({ name: "bad", version: "1" }).connect(deniedTransport);
  } catch {
    denied = true;
  } finally {
    await deniedTransport.close();
  }
  check("MCP SDK rejects unauthorized connection", denied);
  const { runTransferChecks } = await import("./transfer-checks.mjs");
  await runTransferChecks({
    binary,
    work,
    server: configServer,
    entries,
    fixture,
    check,
  });
  await writeFile(
    resolve(work, "report.json"),
    JSON.stringify({ passed: checks.length, checks }, null, 2),
  );
  console.log(
    "\n" +
      checks.length +
      " protocol integration checks passed. Report: " +
      work,
  );
} finally {
  await mcp.close().catch(() => {});
  proc.kill();
  for (const client of clients) client.end();
  await new Promise((r) => ssh.close(r));
}
