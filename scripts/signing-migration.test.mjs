import { spawnSync } from "node:child_process";
import assert from "node:assert/strict";
import test from "node:test";
import { existsSync } from "node:fs";

test("signing migration preserves exact bytes and rejects untrusted inputs", () => {
  assert.ok(existsSync("scripts/signing_migration_test.py"), "migration behavior suite is missing");
  const result = spawnSync("python3", ["-I", "-S", "scripts/signing_migration_test.py"], {
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr || result.stdout);
});

test("owner receipt validation rejects a changed run, recipient, or plaintext field", async () => {
  const { validateReceipt } = await import("./signing-migration-receipt.mjs");
  const expected = {
    repository: "fallow-rs/fallow",
    source_sha: "a".repeat(40),
    run_id: "123",
    run_attempt: "1",
  };
  const recipient = { key_id: "12345", key: Buffer.alloc(32, 1).toString("base64") };
  const receipt = {
    schema: 1,
    mode: "export",
    ...expected,
    environment: "release",
    signing_public_key: "g05v13Mz5u7fd5NHxxCstAPS2CNNVZ9e18h+VSreC9E=",
    key_id: recipient.key_id,
    recipient_public_key: recipient.key,
    encrypted_value: Buffer.alloc(92, 2).toString("base64"),
  };
  assert.deepEqual(validateReceipt(receipt, expected, recipient), {
    encrypted_value: receipt.encrypted_value,
    key_id: recipient.key_id,
  });
  for (const change of [
    { run_id: "999" },
    { key_id: "old" },
    { private_key: "plaintext" },
    { encrypted_value: "bad" },
  ]) {
    assert.throws(() => validateReceipt({ ...receipt, ...change }, expected, recipient));
  }
});

test("owner proof verification accepts a valid signature and rejects tampering", async () => {
  const { generateKeyPairSync, sign } = await import("node:crypto");
  const { verifyProofSignature } = await import("./signing-migration-receipt.mjs");
  const { privateKey, publicKey } = generateKeyPairSync("ed25519");
  const raw = publicKey.export({ format: "der", type: "spki" }).subarray(-32).toString("base64");
  const challenge = "synthetic-public-challenge";
  const signature = sign(null, Buffer.from(challenge), privateKey).toString("base64");
  verifyProofSignature(signature, challenge, raw);
  assert.throws(() => verifyProofSignature(signature, "changed", raw));
  assert.throws(() => verifyProofSignature(Buffer.alloc(64).toString("base64"), challenge, raw));
});
