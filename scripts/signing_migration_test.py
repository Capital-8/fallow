"""Synthetic native-crypto checks. No account or signing credentials are used."""
import base64
import ctypes
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location("migration", Path(__file__).with_name("signing_migration.py"))
MODULE = None
if SPEC and Path(SPEC.origin).exists():
    MODULE = importlib.util.module_from_spec(SPEC)
    SPEC.loader.exec_module(MODULE)


class MigrationTests(unittest.TestCase):
    def test_preserves_exact_secret_bytes(self):
        self.assertTrue(callable(getattr(MODULE, "seal_checked", None)), "checked secret sealing is unavailable")
        path = Path("/opt/homebrew/Cellar/libsodium/1.0.22/lib/libsodium.26.dylib")
        sodium = MODULE.Sodium(path if path.exists() else MODULE.discover_library())
        pk = (ctypes.c_ubyte * 32)()
        sk = (ctypes.c_ubyte * 32)()
        self.assertEqual(sodium.lib.crypto_box_keypair(pk, sk), 0)
        seed = bytes(range(32))
        anchor = sodium.public_key(seed)
        secret = b" \t" + base64.b64encode(seed) + b"\r\n\n"
        ciphertext = MODULE.seal_checked(sodium, secret, anchor, bytes(pk))
        output = (ctypes.c_ubyte * len(secret))()
        self.assertEqual(sodium.lib.crypto_box_seal_open(output, ciphertext, len(ciphertext), pk, sk), 0)
        self.assertEqual(bytes(output), secret)
        with self.assertRaises(ValueError):
            MODULE.seal_checked(sodium, secret, bytes(32), bytes(pk))
        with self.assertRaises(ValueError):
            MODULE.seal_checked(sodium, b"invalid", anchor, bytes(pk))
        with self.assertRaises(ValueError):
            MODULE.seal_checked(sodium, secret, anchor, b"short")
        tampered = bytes([ciphertext[0] ^ 1]) + ciphertext[1:]
        self.assertNotEqual(sodium.lib.crypto_box_seal_open(output, tampered, len(tampered), pk, sk), 0)

    def test_context_rejects_untrusted_runs(self):
        environ = {"GITHUB_REPOSITORY": "fallow-rs/fallow", "GITHUB_REPOSITORY_ID": "1184090452", "GITHUB_REPOSITORY_OWNER_ID": "269229952", "GITHUB_REF": "refs/heads/main", "GITHUB_EVENT_NAME": "workflow_dispatch", "RUNNER_ENVIRONMENT": "github-hosted", "GITHUB_SHA": "a" * 40, "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1"}
        self.assertEqual(MODULE.context(environ)["run_id"], "123")
        for key in environ:
            with self.assertRaises(ValueError):
                MODULE.context({**environ, key: "untrusted"})

    def test_unreviewed_library_is_not_loaded(self):
        path = Path(__file__)
        with patch.object(MODULE, "Sodium", side_effect=AssertionError("unreviewed native code was loaded")):
            for pin in [None, {"path": str(path), "sha256": "0" * 64, "version": "untrusted"}]:
                with self.assertRaises(ValueError):
                    MODULE.reviewed_sodium(path, pin)

    def test_signature_proof_rejects_wrong_key(self):
        path = Path("/opt/homebrew/Cellar/libsodium/1.0.22/lib/libsodium.26.dylib")
        sodium = MODULE.Sodium(path if path.exists() else MODULE.discover_library())
        seed = bytes(range(32))
        anchor = sodium.public_key(seed)
        challenge = b"synthetic-run-bound-challenge"
        signature = base64.b64decode(sodium.proof(seed, challenge, anchor))
        self.assertEqual(sodium.lib.crypto_sign_verify_detached(signature, challenge, len(challenge), anchor), 0)
        self.assertNotEqual(sodium.lib.crypto_sign_verify_detached(signature, b"changed", 7, anchor), 0)
        with self.assertRaises(ValueError):
            sodium.proof(seed, challenge, bytes(32))


if __name__ == "__main__":
    unittest.main()
