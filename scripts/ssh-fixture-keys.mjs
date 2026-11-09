import { generateKeyPairSync } from "node:crypto";
import ssh2 from "ssh2";

// ssh2's Ed25519 generator strips leading zero bytes from random public keys,
// intermittently producing an invalid OpenSSH key. Generate PEM with Node instead.
export function createSshFixtureKey() {
  const { privateKey } = generateKeyPairSync("rsa", {
    modulusLength: 2048,
    privateKeyEncoding: { type: "pkcs1", format: "pem" },
    publicKeyEncoding: { type: "spki", format: "pem" },
  });
  const parsed = ssh2.utils.parseKey(privateKey);
  if (parsed instanceof Error) {
    throw new Error(`Cannot parse SSH fixture key: ${parsed.message}`, {
      cause: parsed,
    });
  }
  const key = Array.isArray(parsed) ? parsed[0] : parsed;
  if (!key || typeof key.getPublicSSH !== "function") {
    throw new Error("SSH fixture did not produce a usable key");
  }
  return { privateKey, key, publicKey: key.getPublicSSH() };
}
