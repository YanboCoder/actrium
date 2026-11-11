#!/usr/bin/env python3
"""Fail mobile packaging on Realm private keys/invites without printing secrets.

Accept source asset/resource directories or APK/IPA zip files. This is a
packaging gate for structured credentials, not a general secret scanner.
"""
import json
import pathlib
import re
import sys
import zipfile


MAX_RESOURCE_BYTES = 16 * 1024 * 1024
# Compiled code contains protocol/parser constants, not packaged credential
# documents. Recognize it by content so resource names/extensions cannot bypass
# scanning, while native libraries and DEX files do not cause false positives.
CODE_MAGIC = (
    b"\x7fELF", b"dex\n", b"\x00asm", b"!<arch>\n",
    b"\xfe\xed\xfa\xce", b"\xce\xfa\xed\xfe",
    b"\xfe\xed\xfa\xcf", b"\xcf\xfa\xed\xfe",
    b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca",
    b"\xca\xfe\xba\xbf", b"\xbf\xba\xfe\xca",
)


def sensitive(data):
    text = data.decode("utf-8", errors="ignore")
    if re.search(r"(?m)^\s*realm_client_key\s*=", text):
        return True
    if re.search(r"\bri_[A-Za-z0-9_-]{40,}", text):
        return True
    if "-----BEGIN PRIVATE KEY-----" in text:
        return True
    try:
        value = json.loads(text)
    except (ValueError, RecursionError):
        return False

    def walk(value):
        if isinstance(value, dict):
            if value.get("schema") in ("actr.realm-client-key/v1", "actr.realm-invite/v1"):
                return True
            if any(key in value for key in ("private_key", "secret_key", "signing_key")):
                return True
            return any(walk(child) for child in value.values())
        return isinstance(value, list) and any(walk(child) for child in value)

    return walk(value)


def check_resource(stream, label):
    prefix = stream.read(8)
    if prefix.startswith(CODE_MAGIC):
        return False
    data = prefix + stream.read(MAX_RESOURCE_BYTES + 1 - len(prefix))
    if len(data) > MAX_RESOURCE_BYTES:
        print(f"Unscanned mobile resource (exceeds 16 MiB): {label}", file=sys.stderr)
        return True
    if sensitive(data):
        print(f"Unsafe mobile resource: {label}", file=sys.stderr)
        return True
    return False


def main(paths):
    failed = False
    for name in paths:
        path = pathlib.Path(name)
        try:
            if not path.exists():
                raise FileNotFoundError
            if path.suffix.lower() in (".apk", ".ipa"):
                with zipfile.ZipFile(path) as archive:
                    for entry in archive.infolist():
                        if not entry.is_dir():
                            with archive.open(entry) as stream:
                                failed = check_resource(stream, f"{path.name}/{entry.filename}") or failed
            else:
                entries = [path] if path.is_file() else path.rglob("*")
                for entry in entries:
                    if entry.is_file():
                        with entry.open("rb") as stream:
                            failed = check_resource(stream, str(entry)) or failed
        except (OSError, ValueError, RuntimeError, zipfile.BadZipFile):
            # Archive/I/O errors must fail the gate without echoing exception
            # details, which can contain credential data from malformed inputs.
            print(f"Cannot inspect mobile resources: {path}", file=sys.stderr)
            failed = True
    return int(failed)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
