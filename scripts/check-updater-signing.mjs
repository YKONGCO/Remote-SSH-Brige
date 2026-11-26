import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash, createPublicKey, verify } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createRequire } from "node:module";

const config = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"));
const publicKeyText = Buffer.from(config.plugins.updater.pubkey, "base64")
  .toString("utf8")
  .trim()
  .split(/\r?\n/);
assert(
  publicKeyText[0].startsWith("untrusted comment:"),
  "Invalid Tauri public key encoding",
);
const publicKey = Buffer.from(publicKeyText[1], "base64");
assert.equal(publicKey.length, 42, "Invalid minisign public key length");
assert.equal(
  publicKey.subarray(0, 2).toString(),
  "Ed",
  "Unsupported public key algorithm",
);

if (process.env.TAURI_SIGNING_PRIVATE_KEY) {
  const directory = mkdtempSync(join(tmpdir(), "updater-signing-"));
  try {
    const file = join(directory, "check.txt");
    const content = Buffer.from("Remote SSH Brige updater signing check\n");
    writeFileSync(file, content);
    const require = createRequire(import.meta.url);
    const result = spawnSync(
      process.execPath,
      [require.resolve("@tauri-apps/cli/tauri.js"), "signer", "sign", file],
      { encoding: "utf8", windowsHide: true, timeout: 30_000 },
    );
    // Keep CLI output private: signing failures must never print key material.
    assert(
      !result.error && result.status === 0,
      "Updater signing failed; check the private key and password Secrets",
    );
    const signatureText = Buffer.from(
      readFileSync(`${file}.sig`, "utf8").trim(),
      "base64",
    )
      .toString("utf8")
      .trim()
      .split(/\r?\n/);
    const signature = Buffer.from(signatureText[1], "base64");
    assert.equal(signature.length, 74, "Invalid minisign signature length");
    assert(
      signature.subarray(2, 10).equals(publicKey.subarray(2, 10)),
      "Updater private key does not match the configured public key",
    );
    const algorithm = signature.subarray(0, 2).toString();
    assert(["Ed", "ED"].includes(algorithm), "Unsupported signature algorithm");
    const key = createPublicKey({
      key: Buffer.concat([
        Buffer.from("302a300506032b6570032100", "hex"),
        publicKey.subarray(10),
      ]),
      format: "der",
      type: "spki",
    });
    const message =
      algorithm === "ED"
        ? createHash("blake2b512").update(content).digest()
        : content;
    assert(
      verify(null, message, key, signature.subarray(10)),
      "Updater signature verification failed",
    );
    console.log("Updater signing key and public key verified.");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
} else {
  assert(
    !process.env.GITHUB_REF?.startsWith("refs/tags/"),
    "Release tags require TAURI_SIGNING_PRIVATE_KEY",
  );
  console.log("Updater public key verified; building without updater signing.");
}
