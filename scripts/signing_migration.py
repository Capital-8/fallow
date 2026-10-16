"""Temporary ciphertext-only transfer for the existing binary signing seed."""
import base64
import ctypes
import ctypes.util
import hashlib
import json
import os
from pathlib import Path
import sys

ANCHOR = "g05v13Mz5u7fd5NHxxCstAPS2CNNVZ9e18h+VSreC9E="
SECRET = "ED25519_BINARY_SIGNING_PRIVATE_KEY"
METADATA = Path(__file__).with_name("signing-migration.json")


def discover_library():
    """Locate the system library without installing packages."""
    if sys.platform == "linux":
        return Path("/usr/lib/x86_64-linux-gnu/libsodium.so.23").resolve(strict=True)
    name = ctypes.util.find_library("sodium")
    if not name:
        raise ValueError("System libsodium is unavailable")
    return Path(name).resolve(strict=True)


class Sodium:
    """Call only the native libsodium primitives needed for migration."""
    def __init__(self, path):
        self.lib = ctypes.CDLL(str(path))
        self.lib.sodium_init.restype = ctypes.c_int
        if self.lib.sodium_init() < 0:
            raise ValueError("libsodium initialization failed")
        self.lib.sodium_version_string.restype = ctypes.c_char_p
        self.lib.crypto_sign_seed_keypair.argtypes = [ctypes.c_void_p] * 3
        self.lib.crypto_box_keypair.argtypes = [ctypes.c_void_p] * 2
        self.lib.crypto_box_seal.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_ulonglong, ctypes.c_void_p]
        self.lib.crypto_box_seal_open.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_ulonglong, ctypes.c_void_p, ctypes.c_void_p]
        self.lib.crypto_sign_detached.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_ulonglong, ctypes.c_void_p]
        self.lib.crypto_sign_verify_detached.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_ulonglong, ctypes.c_void_p]
        self.lib.sodium_memzero.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
        self.lib.sodium_memzero.restype = None

    def signing_keys(self, seed):
        if len(seed) != 32:
            raise ValueError("Invalid signing seed length")
        public = (ctypes.c_ubyte * 32)()
        private = (ctypes.c_ubyte * 64)()
        if self.lib.crypto_sign_seed_keypair(public, private, seed) != 0:
            raise ValueError("Signing key derivation failed")
        return public, private

    def public_key(self, seed):
        public, private = self.signing_keys(seed)
        try:
            return bytes(public)
        finally:
            self.lib.sodium_memzero(private, len(private))

    def seal(self, secret, recipient):
        if len(recipient) != 32:
            raise ValueError("Invalid recipient public key")
        output = (ctypes.c_ubyte * (len(secret) + 48))()
        if self.lib.crypto_box_seal(output, secret, len(secret), recipient) != 0:
            raise ValueError("Sealing failed")
        return bytes(output)

    def proof(self, seed, challenge, anchor):
        public, private = self.signing_keys(seed)
        signature = (ctypes.c_ubyte * 64)()
        length = ctypes.c_ulonglong()
        try:
            if bytes(public) != anchor:
                raise ValueError("Signing public key differs from anchor")
            if self.lib.crypto_sign_detached(signature, ctypes.byref(length), challenge, len(challenge), private) != 0:
                raise ValueError("Challenge signing failed")
            if length.value != 64 or self.lib.crypto_sign_verify_detached(signature, challenge, len(challenge), anchor) != 0:
                raise ValueError("Challenge verification failed")
            return base64.b64encode(signature).decode("ascii")
        finally:
            self.lib.sodium_memzero(private, len(private))


def seed_bytes(secret):
    return base64.b64decode(secret.strip(), validate=True)


def seal_checked(sodium, secret, anchor, recipient):
    """Validate the seed anchor, then seal the original bytes without trimming."""
    if sodium.public_key(seed_bytes(secret)) != anchor:
        raise ValueError("Signing public key differs from anchor")
    return sodium.seal(secret, recipient)


def context(environ):
    """Reject any export or verification outside the reviewed main dispatch."""
    required = {
        "GITHUB_REPOSITORY": "fallow-rs/fallow",
        "GITHUB_REPOSITORY_ID": "1184090452",
        "GITHUB_REPOSITORY_OWNER_ID": "269229952",
        "GITHUB_REF": "refs/heads/main",
        "GITHUB_EVENT_NAME": "workflow_dispatch",
        "RUNNER_ENVIRONMENT": "github-hosted",
    }
    if any(environ.get(key) != value for key, value in required.items()):
        raise ValueError("Untrusted migration context")
    sha = environ.get("GITHUB_SHA", "")
    if len(sha) != 40 or any(char not in "0123456789abcdef" for char in sha):
        raise ValueError("Invalid source SHA")
    run_id = environ.get("GITHUB_RUN_ID", "")
    attempt = environ.get("GITHUB_RUN_ATTEMPT", "")
    if not run_id.isdecimal() or not attempt.isdecimal():
        raise ValueError("Invalid run identity")
    return {"repository": required["GITHUB_REPOSITORY"], "source_sha": sha, "run_id": run_id, "run_attempt": attempt}


def reviewed_sodium(path, pin):
    """Check bytes before loading native code into a credential-bearing process."""
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    if not pin or pin["path"] != str(path) or pin["sha256"] != digest:
        raise ValueError("Native crypto toolchain differs from reviewed pin")
    sodium = Sodium(path)
    if sodium.lib.sodium_version_string().decode("ascii") != pin["version"]:
        raise ValueError("Native crypto version differs from reviewed pin")
    return sodium


def main():
    if len(sys.argv) != 2 or sys.argv[1] not in {"preflight", "export", "verify"}:
        raise ValueError("Use preflight, export or verify")
    mode = sys.argv[1]
    path = discover_library()
    if mode == "preflight":
        sodium = Sodium(path)
        toolchain = {"path": str(path), "version": sodium.lib.sodium_version_string().decode("ascii"), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
        print(json.dumps(toolchain, sort_keys=True))
        return
    source = context(os.environ)
    metadata = json.loads(METADATA.read_text())
    sodium = reviewed_sodium(path, metadata["toolchain"])
    if metadata["repository"] != source["repository"] or metadata["environment"] != "release":
        raise ValueError("Untrusted recipient metadata")
    secret = os.environ.pop(SECRET, "").encode("utf-8")
    if not secret:
        raise ValueError("Signing secret is absent")
    anchor = base64.b64decode(ANCHOR, validate=True)
    receipt = {"schema": 1, "mode": mode, **source, "environment": "release", "signing_public_key": ANCHOR}
    if mode == "export":
        recipient = base64.b64decode(metadata["key"], validate=True)
        receipt.update({"key_id": metadata["key_id"], "recipient_public_key": metadata["key"], "encrypted_value": base64.b64encode(seal_checked(sodium, secret, anchor, recipient)).decode("ascii")})
    else:
        challenge = ("fallow-signing-environment-proof-v1:" + json.dumps(source, sort_keys=True, separators=(",", ":"))).encode("ascii")
        receipt.update({"challenge": challenge.decode("ascii"), "signature": sodium.proof(seed_bytes(secret), challenge, anchor)})
    output = Path("signing-migration-output")
    output.mkdir(mode=0o700)
    (output / "receipt.json").write_text(json.dumps(receipt, sort_keys=True) + "\n")


if __name__ == "__main__":
    try:
        main()
    except Exception:
        # Never print exception inputs: a library or decoder can include secret bytes.
        print("Signing migration failed closed; inspect public inputs and toolchain.", file=sys.stderr)
        sys.exit(1)
