import assert from "node:assert/strict";
import { mkdir, writeFile, readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { createTransferReadGate } from "./transfer-read-gate.mjs";

export async function runRecoveryChecks({
  value,
  wait,
  entries,
  fixture,
  uploads,
  downloads,
  check,
  restart,
  readTask,
  readSavedSnapshot,
}) {
  const groupTasks = async (groupId) =>
    (await value("transfer", { action: "list" })).transfers.filter(
      (task) => task.group_id === groupId,
    );
  const bytes = Buffer.alloc(8 * 1024 * 1024 + 17, 0x73);
  const source = resolve(uploads, "recover.bin");
  await writeFile(source, bytes);
  fixture.dropTransferWriteAfter = 2 * 1024 * 1024;
  const upload = await value("upload_file", {
    items: [{ local_path: source, remote_path: "/workspace/recover.bin" }],
  });
  const uploaded = await wait(upload.transfer_id);
  assert.equal(uploaded.status, "completed", JSON.stringify(uploaded));
  const uploadRecord = await readSavedSnapshot(upload.transfer_id, "completed");
  check(
    "Interrupted upload reconnects and reuses verified blocks",
    uploadRecord.retry_count > 0 &&
      uploadRecord.resumed_bytes >= 1024 * 1024 &&
      entries.get("/workspace/recover.bin").equals(bytes),
  );

  for (const delay of [0, 50]) {
    const gate = createTransferReadGate(
      "/workspace/recover.bin",
      2 * 1024 * 1024,
    );
    fixture.transferReadGate = gate;
    fixture.transferReadDelay = delay;
    try {
      const destination = resolve(
        downloads,
        delay ? "recover-delayed.bin" : "recover.bin",
      );
      const task = await value("download_file", {
        items: [
          { local_path: destination, remote_path: "/workspace/recover.bin" },
        ],
      });
      const retained = await wait(
        task.transfer_id,
        (s) =>
          s.transferred_bytes >= 1024 * 1024 ||
          ["failed", "completed", "cancelled"].includes(s.status),
      );
      assert.equal(retained.status, "running", JSON.stringify(retained));
      assert.ok(
        retained.transferred_bytes >= 1024 * 1024,
        JSON.stringify(retained),
      );
      await gate.waitUntilBlocked();
      gate.disconnect();
      const downloaded = await wait(task.transfer_id);
      assert.equal(downloaded.status, "completed", JSON.stringify(downloaded));
      const record = await readSavedSnapshot(task.transfer_id, "completed");
      const checksumMatches = (await readFile(destination)).equals(bytes);
      check(
        `Interrupted download reuses verified blocks and validates complete checksum (reply delay ${delay} ms)`,
        record.retry_count > 0 &&
          record.resumed_bytes >= 1024 * 1024 &&
          checksumMatches,
        { ...record, checksum_matches: checksumMatches },
      );
    } finally {
      fixture.transferReadGate = null;
      fixture.transferReadDelay = 0;
      gate.release();
    }
  }
  check(
    "Completed download preserves the final local file",
    (await readFile(resolve(downloads, "recover.bin"))).equals(bytes),
  );

  fixture.dropCommitResponse = true;
  const uncertain = await value("upload_file", {
    items: [
      { local_path: source, remote_path: "/workspace/uncertain-upload.bin" },
    ],
  });
  const unknown = await wait(uncertain.transfer_id);
  fixture.dropCommitResponse = false;
  check(
    "Lost upload commit acknowledgement stops automatic replay",
    unknown.status === "failed" &&
      unknown.error.startsWith("COMMIT_STATUS_UNKNOWN:") &&
      !unknown.resume_available &&
      (await readSavedSnapshot(uncertain.transfer_id, "failed")).retry_count ===
        0,
  );
  const commits = fixture.commitCalls;
  await value("transfer", {
    action: "resume",
    transfer_id: uncertain.transfer_id,
  });
  check(
    "Manual recovery recognizes an already committed upload without overwriting",
    (await wait(uncertain.transfer_id)).status === "completed" &&
      fixture.commitCalls === commits,
  );

  const pauseGate = createTransferReadGate(
    "/workspace/recover.bin",
    2 * 1024 * 1024,
  );
  fixture.transferReadGate = pauseGate;
  const paused = await value("download_file", {
    items: [
      {
        local_path: resolve(downloads, "paused.bin"),
        remote_path: "/workspace/recover.bin",
      },
    ],
  });
  await wait(
    paused.transfer_id,
    (s) => s.transferred_bytes >= 1024 * 1024 || s.status === "failed",
  );
  await pauseGate.waitUntilBlocked();
  check(
    "Pause is acknowledged before the worker stops",
    (await value("transfer", {
      action: "pause",
      transfer_id: paused.transfer_id,
    })) === "Pause requested",
  );
  fixture.transferReadGate = null;
  pauseGate.release();
  const stopped = await wait(paused.transfer_id, (s) =>
    ["paused", "failed", "completed"].includes(s.status),
  );
  check(
    "Paused task retains progress without publishing a target",
    stopped.status === "paused" && stopped.transferred_bytes > 0,
  );
  fixture.transferReadDelay = 0;
  await value("transfer", {
    action: "resume",
    transfer_id: paused.transfer_id,
  });
  const continued = await wait(paused.transfer_id);
  check(
    "Resume preserves file content and checkpoint progress",
    continued.status === "completed" &&
      (await readSavedSnapshot(paused.transfer_id, "completed")).resumed_bytes >
        0 &&
      (await readFile(resolve(downloads, "paused.bin"))).equals(bytes),
  );

  const tree = resolve(uploads, "tree");
  await mkdir(resolve(tree, "nested"), { recursive: true });
  await mkdir(resolve(tree, "empty"), { recursive: true });
  await writeFile(resolve(tree, "nested", "hello.txt"), "recursive content");
  const batch = await value("upload_file", {
    items: [{ local_path: tree, remote_path: "/workspace/batch-tree" }],
    recursive: true,
    skip_existing: true,
  });
  const batchTasks = await groupTasks(batch.group_id);
  check(
    "Batch queues files and allows discovery of child task IDs",
    batch.queued === batchTasks.length &&
      batchTasks.every((task) => task.transfer_id),
  );
  const results = await Promise.all(batchTasks.map((t) => wait(t.transfer_id)));
  check(
    "Recursive batch uploads files and empty directories through the queue",
    results.every((t) => t.status === "completed") &&
      entries.get("/workspace/batch-tree/empty") === null &&
      entries.get("/workspace/batch-tree/nested/hello.txt").toString() ===
        "recursive content",
  );
  const groupRead = await value("transfer", {
    action: "read",
    group_id: batch.group_id,
  });
  check(
    "Group read reports completed count without child snapshots",
    groupRead.completed === batch.queued &&
      groupRead.status === "completed" &&
      !Object.hasOwn(groupRead, "tasks"),
  );
  const groupPause = await value("transfer", {
    action: "pause",
    group_id: batch.group_id,
  });
  const groupResume = await value("transfer", {
    action: "resume",
    group_id: batch.group_id,
  });
  const groupCancel = await value("transfer", {
    action: "cancel",
    group_id: batch.group_id,
  });
  check(
    "Group pause, resume and cancel route by group_id",
    groupPause.pause_requested === 0 &&
      groupResume.resumed === 0 &&
      groupCancel.cancel_requested === 0,
  );
  const batchDownload = await value("download_file", {
    items: [
      {
        local_path: resolve(downloads, "tree"),
        remote_path: "/workspace/batch-tree",
      },
    ],
    recursive: true,
    skip_existing: true,
  });
  const downloadTasks = await groupTasks(batchDownload.group_id);
  assert.equal(downloadTasks.length, batchDownload.queued);
  const saved = await Promise.all(
    downloadTasks.map((t) => wait(t.transfer_id)),
  );
  check(
    "Recursive download creates directories and verifies child files",
    saved.every((t) => t.status === "completed") &&
      (await readFile(
        resolve(downloads, "tree", "nested", "hello.txt"),
        "utf8",
      )) === "recursive content",
  );
  const skip = await value("download_file", {
    items: [
      {
        local_path: resolve(downloads, "tree", "nested", "hello.txt"),
        remote_path: "/workspace/batch-tree/nested/hello.txt",
      },
    ],
    skip_existing: true,
  });
  check(
    "Existing identical content is skipped after checksum verification",
    (await wait(skip.transfer_id)).status === "completed" &&
      (await readSavedSnapshot(skip.transfer_id, "completed")).phase ===
        "skipped",
  );

  fixture.transferReadDelay = 50;
  const queued = [];
  for (let i = 0; i < 6; i++)
    queued.push(
      await value("download_file", {
        items: [
          {
            local_path: resolve(downloads, `queued-${i}.bin`),
            remote_path: "/workspace/recover.bin",
          },
        ],
      }),
    );
  const listing = (
    await value("transfer", { action: "list" })
  ).transfers.filter((t) =>
    queued.some((q) => q.transfer_id === t.transfer_id),
  );
  check(
    "Excess tasks queue instead of failing at the concurrency limit",
    listing.length === 6 &&
      listing.filter((t) => t.status === "queued").length >= 2,
  );
  for (const task of queued)
    await value("transfer", {
      action: "cancel",
      transfer_id: task.transfer_id,
    });
  await Promise.all(queued.map((t) => wait(t.transfer_id)));

  const restartGate = createTransferReadGate(
    "/workspace/recover.bin",
    6 * 1024 * 1024,
  );
  fixture.transferReadGate = restartGate;
  const recovery = await value("download_file", {
    items: [
      {
        local_path: resolve(downloads, "restart.bin"),
        remote_path: "/workspace/recover.bin",
      },
    ],
  });
  await wait(
    recovery.transfer_id,
    (s) => s.transferred_bytes >= 4 * 1024 * 1024 || s.status === "failed",
  );
  const checkpointDeadline = Date.now() + 30000;
  while ((readTask(recovery.transfer_id)?.checkpoint.hashes.length ?? 0) < 4) {
    assert.ok(
      Date.now() < checkpointDeadline,
      "The transfer did not persist four verified blocks before restart",
    );
    await new Promise((r) => setTimeout(r, 10));
  }
  await restartGate.waitUntilBlocked();
  await restart();
  fixture.transferReadGate = null;
  restartGate.release();
  const restored = await value("transfer", {
    action: "read",
    transfer_id: recovery.transfer_id,
  });
  check(
    "Process restart restores interrupted tasks as paused",
    restored.status === "paused" &&
      restored.transferred_bytes >= 4 * 1024 * 1024,
  );
  fixture.transferReadDelay = 0;
  await value("transfer", {
    action: "resume",
    transfer_id: recovery.transfer_id,
  });
  const recovered = await wait(recovery.transfer_id);
  check(
    "Restarted process resumes durable checkpoints with matching content",
    recovered.status === "completed" &&
      (await readSavedSnapshot(recovery.transfer_id, "completed"))
        .resumed_bytes >=
        4 * 1024 * 1024 &&
      (await readFile(resolve(downloads, "restart.bin"))).equals(bytes),
  );
}
