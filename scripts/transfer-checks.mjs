// Real MCP -> Rust -> SSH/SFTP -> disk checks. The SSH fixture is provided by integration.mjs.
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { createHash } from "node:crypto";
import {
  mkdir,
  readFile,
  writeFile,
  open,
  readdir,
  symlink,
} from "node:fs/promises";
import { resolve } from "node:path";
import { spawn } from "node:child_process";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";

export async function runTransferChecks({
  binary,
  work,
  server,
  entries,
  fixture,
  check,
}) {
  const dir = resolve(work, "file-transfers"),
    uploads = resolve(dir, "sources"),
    downloads = resolve(dir, "downloads");
  await mkdir(uploads, { recursive: true });
  await mkdir(downloads, { recursive: true });
  const databasePath = resolve(dir, "remote-desk.sqlite3");
  const db = new DatabaseSync(databasePath);
  db.exec(
    await readFile(
      new URL("../src-tauri/src/schema.sql", import.meta.url),
      "utf8",
    ),
  );
  db.exec("PRAGMA user_version=2");
  const insert = db.prepare(
    "INSERT INTO servers (id,name,host,port,username,auth,key_path,fingerprint,group_name,color,enabled,allow_exec,allow_write,allow_terminal,root,mcp_enabled,mcp_permissions_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
  );
  for (const [id, grants] of [
    ["test-host", ["upload_file", "download_file", "transfer"]],
    ["other-host", ["upload_file", "download_file", "transfer"]],
    ["no-manager", ["upload_file"]],
    ["no-roots", ["upload_file", "download_file", "transfer"]],
    ["delete-protected", ["remove_path"]],
  ]) {
    insert.run(
      id,
      server.name,
      server.host,
      server.port,
      server.username,
      server.auth,
      server.key_path,
      server.fingerprint,
      server.group,
      server.color,
      1,
      0,
      0,
      0,
      server.root,
      1,
      JSON.stringify(grants),
    );
  }
  db.prepare(
    "INSERT INTO metadata(key,value) VALUES('runtime_settings',?)",
  ).run(
    JSON.stringify({
      upload_limit_mib: 0,
      download_limit_mib: 0,
      local_upload_roots: [uploads],
      local_download_roots: [downloads],
      io_timeout_secs: 5,
    }),
  );
  db.close();
  const proc = spawn(binary, [dir, "18930"], {
    windowsHide: true,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stderr = "";
  proc.stderr.on("data", (b) => (stderr += b));
  proc.on("error", (e) => (stderr += e.message));
  const started = Date.now();
  while (!stderr.includes('"token"')) {
    if (proc.exitCode !== null || Date.now() - started > 15000)
      throw Error("Transfer service failed: " + stderr);
    await new Promise((r) => setTimeout(r, 25));
  }
  const status = JSON.parse(
    stderr.split("\n").find((l) => l.includes('"token"')),
  );
  const client = new Client({ name: "streamed-file-tests", version: "1" });
  const url = new URL(`http://127.0.0.1:${status.port}/mcp`);
  const transport = new StreamableHTTPClientTransport(url, {
    requestInit: { headers: { Authorization: "Bearer " + status.token } },
  });
  const call = async (name, args) =>
    client.callTool({ name, arguments: { server_id: "test-host", ...args } });
  const value = async (name, args) => {
    const r = await call(name, args);
    assert.ok(!r.isError, r.content[0].text);
    return JSON.parse(r.content[0].text);
  };
  const wait = async (
    id,
    predicate = (s) => ["completed", "failed", "cancelled"].includes(s.status),
  ) => {
    const deadline = Date.now() + 180000;
    let current = await value("transfer", { action: "read", transfer_id: id });
    let lastPrinted = Date.now();
    while (!predicate(current)) {
      assert.ok(
        Date.now() < deadline,
        "Transfer did not reach expected state: " + JSON.stringify(current),
      );
      current = await value("transfer", {
        action: "read",
        transfer_id: id,
        version: current.version,
        wait_secs: 10,
      });
      if (
        process.env.REMOTE_SSH_TEST_LARGE_FILES === "1" &&
        Date.now() - lastPrinted > 10000
      ) {
        console.log(
          `TRANSFER ${current.direction}: ${current.transferred_bytes}/${current.total_bytes} bytes (${current.phase})`,
        );
        lastPrinted = Date.now();
      }
    }
    return current;
  };
  let rssTimer,
    peakRss = 0,
    baselineRss = 0;
  const readRss = async () => {
    if (process.platform !== "linux") return 0;
    const text = await readFile(`/proc/${proc.pid}/status`, "utf8");
    return Number(text.match(/^VmRSS:\s+(\d+)/m)?.[1] || 0) * 1024;
  };
  try {
    await client.connect(transport);
    const discovered = await client.callTool({
      name: "list_servers",
      arguments: {},
    });
    const protectedServer = JSON.parse(discovered.content[0].text).find(
      (s) => s.id === "delete-protected",
    );
    check(
      "Deletion protection hides effective MCP deletion permission",
      protectedServer.security.deletion_protection &&
        !protectedServer.mcp_permissions.includes("remove_path") &&
        protectedServer.security.deletion_protection_scope === "sftp_remove",
    );
    const tools = await client.listTools();
    check(
      "Protected deletion tool stays registered for stable client discovery",
      tools.tools.some((tool) => tool.name === "remove_path"),
    );
    entries.set("/workspace/protected-delete.txt", Buffer.from("keep"));
    const deleteCalls = fixture.deleteCalls;
    const protectedResult = await call("remove_path", {
      server_id: "delete-protected",
      path: "/workspace/protected-delete.txt",
    });
    check(
      "Deletion protection rejects previously authorized MCP deletion before SFTP",
      protectedResult.isError &&
        entries.has("/workspace/protected-delete.txt") &&
        fixture.deleteCalls === deleteCalls,
    );
    const tinySource = resolve(uploads, "tiny.bin");
    await writeFile(tinySource, Buffer.alloc(28, 65));
    const tiny = await value("upload_file", {
      local_path: tinySource,
      remote_path: "/workspace/tiny-upload.bin",
    });
    const tinyCompleted = await value("transfer", {
      action: "read",
      transfer_id: tiny.transfer_id,
      wait_secs: 10,
    });
    check(
      "Unversioned transfer.read waits for tiny upload completion and preserves speed",
      tinyCompleted.status === "completed" &&
        tinyCompleted.transferred_bytes === 28 &&
        tinyCompleted.bytes_per_second > 0,
    );
    const source = resolve(uploads, "source.bin"),
      destination = resolve(downloads, "saved.bin");
    const size =
      process.env.REMOTE_SSH_TEST_LARGE_FILES === "1"
        ? 272 * 1024 * 1024
        : 512 * 1024;
    const fd = await open(source, "w");
    await fd.truncate(size);
    const marker = Buffer.from("FILE_CONTENT_MUST_NOT_ENTER_AUDIT");
    await fd.write(marker, 0, marker.length, 0);
    await fd.close();
    baselineRss = await readRss();
    peakRss = baselineRss;
    if (process.platform === "linux")
      rssTimer = setInterval(
        () =>
          void readRss()
            .then((r) => (peakRss = Math.max(peakRss, r)))
            .catch(() => {}),
        100,
      );
    const upload = await value("upload_file", {
      local_path: source,
      remote_path: "/workspace/stream-upload.bin",
    });
    check(
      "MCP upload starts with metadata only",
      upload.status === "queued" &&
        !Object.hasOwn(upload, "content") &&
        !Object.hasOwn(upload, "base64"),
    );
    const uploaded = await wait(upload.transfer_id);
    check(
      "Streamed upload commits the complete file",
      uploaded.status === "completed" &&
        uploaded.transferred_bytes === size &&
        entries.get("/workspace/stream-upload.bin").length === size,
    );
    const expectedHash = createHash("sha256")
      .update(entries.get("/workspace/stream-upload.bin"))
      .digest("hex");
    const download = await value("download_file", {
      remote_path: "/workspace/stream-upload.bin",
      local_path: destination,
    });
    const downloaded = await wait(download.transfer_id);
    const read = await open(destination, "r");
    const hash = createHash("sha256");
    for await (const chunk of read.createReadStream()) hash.update(chunk);
    await read.close();
    check(
      "Download streams to the requested disk location with matching content",
      downloaded.status === "completed" &&
        downloaded.transferred_bytes === size &&
        hash.digest("hex") === expectedHash,
    );
    if (size > 256 * 1024 * 1024)
      check(
        "Real files over 256 MiB transfer with bounded Rust memory",
        process.platform !== "linux" ||
          peakRss - baselineRss < 64 * 1024 * 1024,
      );
    check(
      "Transfer listing contains final metadata",
      (await value("transfer", { action: "list" })).transfers.some(
        (t) =>
          t.transfer_id === download.transfer_id && t.status === "completed",
      ),
    );
    check(
      "Existing local destination is refused by default",
      (
        await call("download_file", {
          remote_path: "/workspace/stream-upload.bin",
          local_path: destination,
        })
      ).isError,
    );
    const noOverwrite = await value("upload_file", {
      local_path: source,
      remote_path: "/workspace/stream-upload.bin",
    });
    check(
      "Existing remote destination is refused by default",
      (await wait(noOverwrite.transfer_id)).status === "failed",
    );
    const small = resolve(uploads, "small.txt");
    await writeFile(small, "replacement");
    const replace = await value("upload_file", {
      local_path: small,
      remote_path: "/workspace/stream-upload.bin",
      overwrite: true,
    });
    check(
      "Explicit remote overwrite commits atomically",
      (await wait(replace.transfer_id)).status === "completed" &&
        entries.get("/workspace/stream-upload.bin").toString() ===
          "replacement",
    );
    const saveAgain = await value("download_file", {
      local_path: destination,
      remote_path: "/workspace/stream-upload.bin",
      overwrite: true,
    });
    check(
      "Explicit local overwrite commits atomically",
      (await wait(saveAgain.transfer_id)).status === "completed" &&
        (await readFile(destination, "utf8")) === "replacement",
    );
    check(
      "Upload cannot read outside local upload roots",
      (
        await call("upload_file", {
          local_path: server.key_path,
          remote_path: "/workspace/denied.bin",
        })
      ).isError,
    );
    check(
      "Download cannot write outside local download roots",
      (
        await call("download_file", {
          local_path: resolve(dir, "denied.bin"),
          remote_path: "/workspace/stream-upload.bin",
        })
      ).isError,
    );
    check(
      "Transfer start requires direction and management grants",
      (
        await call("upload_file", {
          server_id: "no-manager",
          local_path: small,
          remote_path: "/workspace/denied.bin",
        })
      ).isError,
    );
    check(
      "Transfer IDs are scoped to their server",
      (
        await call("transfer", {
          server_id: "other-host",
          action: "read",
          transfer_id: upload.transfer_id,
        })
      ).isError,
    );
    check(
      "Future transfer versions are rejected",
      (
        await call("transfer", {
          action: "read",
          transfer_id: upload.transfer_id,
          version: uploaded.version + 100,
        })
      ).isError,
    );
    for (const [name, args] of [
      [
        "upload_file",
        {
          local_path: small,
          remote_path: "/workspace/bad",
          content: "forbidden",
        },
      ],
      [
        "download_file",
        {
          local_path: destination,
          remote_path: "/workspace/bad",
          base64: "forbidden",
        },
      ],
      ["transfer", { action: "status", transfer_id: upload.transfer_id }],
      [
        "transfer",
        { action: "read", transfer_id: upload.transfer_id, wait_secs: 31 },
      ],
      [
        "transfer",
        { action: "cancel", transfer_id: upload.transfer_id, version: 1 },
      ],
    ]) {
      check(
        "Transfer schemas reject unrelated or invalid fields: " +
          name +
          " " +
          JSON.stringify(args),
        (await call(name, args)).isError,
      );
    }
    if (process.platform !== "win32") {
      const escaped = resolve(uploads, "escaped");
      await symlink(server.key_path, escaped);
      check(
        "Local upload symlink escape is rejected",
        (
          await call("upload_file", {
            local_path: escaped,
            remote_path: "/workspace/denied.bin",
          })
        ).isError,
      );
      const escapedDownload = resolve(downloads, "escaped");
      await symlink(small, escapedDownload);
      check(
        "Local download symlink is never overwritten",
        (
          await call("download_file", {
            local_path: escapedDownload,
            remote_path: "/workspace/stream-upload.bin",
            overwrite: true,
          })
        ).isError,
      );
    }
    entries.set(
      "/workspace/stream-slow.bin",
      Buffer.alloc(2 * 1024 * 1024, 0x5a),
    );
    // Keep the transfer active beyond the 200 ms progress publication interval;
    // otherwise the first nonzero snapshot can already be entering commit.
    fixture.transferReadDelay = 150;
    const cancelled = await value("download_file", {
      local_path: destination,
      remote_path: "/workspace/stream-slow.bin",
      overwrite: true,
    });
    const running = await wait(
      cancelled.transfer_id,
      (s) => s.transferred_bytes > 0 || s.status === "failed",
    );
    check(
      "Waiting read returns a new version and streamed byte counts",
      running.status === "running" &&
        running.version > cancelled.version &&
        running.transferred_bytes > 0,
    );
    check(
      "Cancel acknowledges the request",
      (
        await value("transfer", {
          action: "cancel",
          transfer_id: cancelled.transfer_id,
        })
      ).cancel_requested,
    );
    check(
      "Cancelled download preserves the original local destination",
      (await wait(cancelled.transfer_id)).status === "cancelled" &&
        (await readFile(destination, "utf8")) === "replacement",
    );
    check(
      "Failed and cancelled downloads clean up local temporary files",
      (await readdir(downloads)).every((name) => !name.startsWith(".tmp")),
    );
    fixture.transferReadDelay = 0;
    fixture.dropTransferRead = true;
    const disconnected = await value("download_file", {
      local_path: destination,
      remote_path: "/workspace/stream-slow.bin",
      overwrite: true,
    });
    check(
      "Disconnect fails without publishing a partial local file",
      (await wait(disconnected.transfer_id)).status === "failed" &&
        (await readFile(destination, "utf8")) === "replacement",
    );
    fixture.rejectCommit = true;
    await writeFile(small, "THIS_REPLACEMENT_MUST_NOT_BE_COMMITTED");
    const failedUpload = await value("upload_file", {
      local_path: small,
      remote_path: "/workspace/stream-upload.bin",
      overwrite: true,
    });
    check(
      "Rejected remote commit preserves the destination",
      (await wait(failedUpload.transfer_id)).status === "failed" &&
        entries.get("/workspace/stream-upload.bin").toString() ===
          "replacement",
    );
    fixture.rejectCommit = false;
    check(
      "Remote upload temporary files are cleaned up",
      ![...entries.keys()].some((k) =>
        k.startsWith("/workspace/.remote-ssh-brige-"),
      ),
    );
    await new Promise((r) => setTimeout(r, 100));
    const audit = new DatabaseSync(databasePath, { readOnly: true });
    const logs = audit
      .prepare("SELECT response_json FROM audit_entries")
      .all()
      .map((row) => JSON.parse(row.response_json).data);
    audit.close();
    check(
      "Database stores transfer parameters and final results",
      logs.some(
        (l) =>
          l.transfer_id === download.transfer_id &&
          l.status === "completed" &&
          l.transferred_bytes === size &&
          l.local_path === destination,
      ),
    );
    check(
      "Database never stores file content, Base64 or chunks",
      !JSON.stringify(logs).includes("FILE_CONTENT_MUST_NOT_ENTER_AUDIT") &&
        logs.every(
          (l) =>
            !["base64", "content", "chunks"].some((k) => Object.hasOwn(l, k)),
        ),
    );
  } finally {
    clearInterval(rssTimer);
    fixture.transferReadDelay = 0;
    fixture.transferWriteDelay = 0;
    fixture.dropTransferRead = false;
    fixture.rejectCommit = false;
    await client.close().catch(() => {});
    proc.kill();
  }
}
