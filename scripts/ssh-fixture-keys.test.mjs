import assert from "node:assert/strict";
import { createHash, sign, verify } from "node:crypto";
import test from "node:test";
import ssh2 from "ssh2";
import { createSshFixtureKey } from "./ssh-fixture-keys.mjs";

test("fixture keys use native PEM generation and support SSH authentication and fingerprints", () => {
  const host = createSshFixtureKey();
  const client = createSshFixtureKey();
  for (const identity of [host, client]) {
    assert.match(identity.privateKey, /^-----BEGIN RSA PRIVATE KEY-----/);
    assert.equal(identity.key.type, "ssh-rsa");
    assert(Buffer.isBuffer(identity.publicKey));
    const publicKey = ssh2.utils.parseKey(identity.publicKey);
    assert(!(publicKey instanceof Error));
    assert(publicKey.getPublicSSH().equals(identity.publicKey));
    const challenge = Buffer.from("SSH fixture authentication challenge");
    const signature = sign("sha256", challenge, identity.privateKey);
    assert.equal(identity.key.verify(challenge, signature, "sha256"), true);
    assert.equal(
      verify("sha256", challenge, identity.key.getPublicPEM(), signature),
      true,
    );
    assert.equal(
      createHash("sha256").update(identity.publicKey).digest().length,
      32,
    );
  }
  assert(!host.publicKey.equals(client.publicKey));
});
