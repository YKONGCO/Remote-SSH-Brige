import assert from "node:assert/strict";
import test from "node:test";
import { createTransferReadGate } from "./transfer-read-gate.mjs";

test("prefetch beyond a cutoff waits for explicit checkpoint confirmation", async () => {
  const gate = createTransferReadGate("/file", 2 * 1024 * 1024);
  let disconnects = 0;
  let replies = 0;
  const client = { end: () => disconnects++ };
  const request = (path, offset) => ({
    path,
    offset,
    client,
    reply: () => replies++,
  });
  // The server can receive this before the application has written block one.
  assert.equal(gate.hold(request("/file", 4 * 1024 * 1024)), true);
  await gate.waitUntilBlocked();
  assert.equal(disconnects, 0);
  assert.equal(replies, 0);
  assert.equal(gate.hold(request("/other", 4 * 1024 * 1024)), false);
  assert.equal(gate.hold(request("/file", 0)), false);
  assert.equal(gate.hold(request("/file", 3 * 1024 * 1024)), true);
  // The caller has now observed a verified client block via transfer.read.
  gate.disconnect();
  gate.disconnect();
  gate.release();
  assert.equal(disconnects, 1);
  assert.equal(replies, 0);
  assert.equal(gate.hold(request("/file", 4 * 1024 * 1024)), false);
});

test("release drains held replies once and leaves subsequent reads unblocked", async () => {
  const gate = createTransferReadGate("/file", 1024);
  let replies = 0;
  const client = {
    end: () => assert.fail("released reads must not disconnect"),
  };
  for (let i = 0; i < 3; i++) {
    assert.equal(
      gate.hold({
        path: "/file",
        offset: 1024 + i,
        client,
        reply: () => replies++,
      }),
      true,
    );
  }
  await gate.waitUntilBlocked();
  gate.release();
  gate.release();
  gate.disconnect();
  assert.equal(replies, 3);
  assert.equal(gate.hold({ path: "/file", offset: 4096, client }), false);
});

test("an unused gate reports a bounded timeout instead of hanging CI", async () => {
  const gate = createTransferReadGate("/missing", 1024);
  await assert.rejects(gate.waitUntilBlocked(10), /No held SFTP read/);
  gate.release();
});
