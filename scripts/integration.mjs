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
const fixture = {
  rejectCommit: false,
  dropCommitResponse: false,
  createRace: false,
  commitCalls: 0,
  failDeletePath: null,
  dropDeletePath: null,
  deleteCalls: 0,
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
            channel.data(id, bytes.subarray(offset, offset + length));
          });
          channel.on("WRITE", (id, h, offset, data) => {
            const handle = handles.get(h.toString("hex"));
            if (!handle) return fail(id);
            const old = entries.get(handle.path);
            const dest = Buffer.alloc(
              Math.max(old.length, offset + data.length),
            );
            old.copy(dest);
            data.copy(dest, offset);
            entries.set(handle.path, dest);
            ok(id);
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
    "list_files",
    "read_file",
    "preview_file",
    "download",
    "write_file",
    "edit_file",
    "remove_path",
    "upload",
    "mkdir",
    "rename",
    "remove",
    "exec",
    "list_terminals",
    "terminal_open",
    "terminal_read",
    "terminal_write",
    "terminal_resize",
    "terminal_close",
  ],
  root: "/workspace",
};
await writeFile(
  resolve(work, "servers.json"),
  JSON.stringify({
    servers: [
      configServer,
      {
        ...configServer,
        id: "limited-host",
        name: "Limited host",
        mcp_permissions: [
          "list_files",
          "read_file",
          "upload",
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
    ],
  }),
);
const settingsDB = new DatabaseSync(resolve(work, "remote-desk.sqlite3"));
settingsDB.exec(
  "CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
);
settingsDB
  .prepare("INSERT INTO metadata(key,value) VALUES('runtime_settings',?)")
  .run(
    JSON.stringify({
      deletion_protection: true,
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
      max_terminals: 1,
    }),
  );
settingsDB.close();
const binary =
  process.env.REMOTE_DESK_TEST_BINARY ||
  resolve(root, "src-tauri", "target", "debug", "remote-desk-headless.exe");
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
const mcp = new Client({ name: "remote-desk-integration", version: "1.0.0" });
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
  await mcp.connect(transport);
  check(
    "MCP reports the new service name and title",
    mcp.getServerVersion()?.name === "remote-ssh" &&
      mcp.getServerVersion()?.title === "Remote SSH MCP",
  );
  const tools = await mcp.listTools();
  check(
    "MCP exposes exactly 8 focused tools",
    JSON.stringify(tools.tools.map((t) => t.name).sort()) ===
      JSON.stringify(
        [
          "list_servers",
          "exec",
          "read_file",
          "write_file",
          "edit_file",
          "remove_path",
          "preview_file",
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
      return {
        error: result.isError,
        value: result.content?.map((c) => c.text || "").join(""),
        metadata: result.structuredContent,
      };
    } catch (error) {
      return { error: true, value: String(error) };
    }
  };
  const value = async (name, args) => {
    const r = await call(name, args);
    assert.ok(!r.error, r.value);
    return name === "read_file"
      ? { content: r.value, ...r.metadata }
      : JSON.parse(r.value);
  };
  for (const [name, expected] of Object.entries({
    list_servers: [],
    read_file: ["server_id", "path", "offset", "limit"],
    write_file: ["server_id", "path", "content", "mode"],
    edit_file: ["server_id", "path", "edits"],
    exec: ["server_id", "command", "timeout_secs"],
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
    "list_files",
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
      result.error && result.value.includes("高危"),
    );
  }
  check(
    "Discover configured host without credentials",
    hosts.length === 3 && !JSON.stringify(hosts).includes("client.pem"),
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
    hidden.error && hidden.value.includes("未授权"),
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
    "Legacy file grants do not imply shell permission",
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
      denied.error && JSON.parse(denied.value).code === "PERMISSION_DENIED",
    );
  }
  const mismatch = await call("read_file", {
    server_id: "wrong-fingerprint",
    path: "/workspace/hello.txt",
  });
  check(
    "Reject changed SSH host fingerprint before authentication",
    mismatch.error && mismatch.value.includes("指纹不匹配"),
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
  const preview = await value("preview_file", { path: "/workspace/hello.txt" });
  check(
    "Preview remote text without changing the file",
    preview.kind === "text" &&
      preview.content === "hello SSH\n" &&
      preview.truncated === false,
  );
  await value("write_file", {
    path: "/workspace/new.txt",
    content: "原子写入 ✓",
  });
  const written = await value("read_file", { path: "/workspace/new.txt" });
  check("Atomic SFTP create and read", written.content === "原子写入 ✓");
  await value("write_file", { path: "/workspace/new.txt", content: "updated" });
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
      page.value.startsWith("一\r\n二\r\n") &&
      page.value.includes("offset=3") &&
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
      failedBatch.value.includes("EDIT_NOT_FOUND") &&
      beforeFailure.equals(entries.get("/workspace/edit.txt")),
  );
  const deniedEdit = await call("edit_file", {
    server_id: "limited-host",
    path: "/workspace/limited.txt",
    edits: [{ old_text: "limited", new_text: "denied-edit" }],
  });
  check(
    "Edit permission is independent of shell access",
    deniedEdit.error && deniedEdit.value.includes("未授权"),
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
      unsupported.value.includes("ATOMIC_WRITE_UNSUPPORTED") &&
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
      uncertain.value.includes("COMMIT_STATUS_UNKNOWN") &&
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
    ![...entries.keys()].some((p) => p.includes(".remote-desk-")),
  );
  entries.set("/workspace/binary.bin", Buffer.from([0, 255, 16, 128]));
  check(
    "Binary preview returns metadata",
    (await value("preview_file", { path: "/workspace/binary.bin" })).kind ===
      "unsupported",
  );
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
    "Safe removal deletes an empty test file while command and desktop protection stay on",
    cleanup.removed &&
      cleanup.removed_entries === 1 &&
      !entries.has("/workspace/mcp_test_remote_ssh.txt") &&
      hosts[0].security.deletion_protection &&
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
    "Strict missing mode reports NOT_FOUND",
    JSON.parse(
      (
        await call("remove_path", {
          path: "/workspace/mcp_test_remote_ssh.txt",
          missing_ok: false,
        })
      ).value,
    ).code === "NOT_FOUND",
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
      JSON.parse(nonEmpty.value).code === "DIRECTORY_NOT_EMPTY" &&
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
      JSON.parse(deniedRemoval.value).code === "PERMISSION_DENIED" &&
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
  const partialDetails = JSON.parse(partial.value);
  check(
    "Recursive deletion reports partial progress on server denial",
    partial.error &&
      partialDetails.code === "PERMISSION_DENIED" &&
      partialDetails.details.removed_entries === 1 &&
      partialDetails.details.partial &&
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
      JSON.parse(unknownDelete.value).code === "COMMIT_STATUS_UNKNOWN" &&
      JSON.parse(unknownDelete.value).details.outcome_unknown &&
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
    output = Buffer.from(r.base64, "base64").toString();
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
  await value("terminal", { action: "close", session_id: terminal.session_id });
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
    typeof limitedOutput.base64 === "string",
  );
  const deniedInput = await call("terminal", {
    action: "write",
    server_id: "limited-host",
    session_id: limitedTerminal.session_id,
    content: "pwd\n",
  });
  check(
    "Existing terminal input is denied without permission",
    deniedInput.error && deniedInput.value.includes("未授权"),
  );
  await value("terminal", {
    action: "close",
    server_id: "limited-host",
    session_id: limitedTerminal.session_id,
  });
  check(
    "Command timeout enforced",
    (await call("exec", { command: "fixture:timeout", timeout_secs: 1 })).error,
  );
  check(
    "Configured default command timeout applies when omitted",
    (await call("exec", { command: "fixture:timeout" })).error,
  );
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
  const recordedExec = logs.find(
    (entry) => entry.kind === "exec" && entry.response?.data?.exit_code === 7,
  );
  check(
    "Audit persists SSH stdout, stderr and exit code",
    recordedExec?.response.data.stdout.includes("stdout ✓") &&
      recordedExec?.response.data.stderr.includes("stderr ✓"),
  );
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
