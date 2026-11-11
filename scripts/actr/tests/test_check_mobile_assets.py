#!/usr/bin/env python3
"""Exercise the packaging gate with synthetic credentials, never real secrets."""

import base64
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile


ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location(
    "check_mobile_assets", ROOT / "scripts/actr/check-mobile-assets.py"
)
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)
# Intentionally invalid key material. Only the document shape is significant.
KEY = json.dumps({
    "schema": "actr.realm-client-key/v1", "private_key": "SYNTHETIC_REVIEW_DATA",
}).encode()
INVITE = b"ri_" + base64.urlsafe_b64encode(json.dumps({
    "schema": "actr.realm-invite/v1", "realm": "review@example.com",
    "secret": "SYNTHETIC_REVIEW_DATA",
}).encode()).rstrip(b"=")


class MobileAssetTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)

    def scan(self, *paths):
        diagnostics = io.StringIO()
        with contextlib.redirect_stderr(diagnostics):
            result = GATE.main([str(path) for path in paths])
        return result, diagnostics.getvalue()

    def archive(self, suffix, entries):
        archive = self.root / ("test." + suffix)
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as output:
            for name, content in entries.items():
                output.writestr(name, content)
        return archive

    def test_renamed_credentials_in_all_mobile_resource_locations_are_rejected(self):
        for suffix, entry, content in (
            ("ipa", "Payload/Test.app/realm-client.json", KEY),
            ("ipa", "Payload/Test.app/realm-client.key", KEY),
            ("ipa", "Payload/Test.app/enrollment.txt", INVITE),
            ("ipa", "Payload/Test.app/Resources/Bundle.bundle/identity", KEY),
            ("apk", "assets/realm-client.key", KEY),
            ("apk", "res/raw/enrollment.txt", INVITE),
            ("apk", "res/raw/identity", KEY),
        ):
            with self.subTest(entry=entry):
                result, diagnostics = self.scan(self.archive(suffix, {entry: content}))
                self.assertEqual(result, 1)
                self.assertIn(entry, diagnostics)
                self.assertNotIn("SYNTHETIC_REVIEW_DATA", diagnostics)
                self.assertNotIn(INVITE.decode(), diagnostics)

    def test_source_directories_and_individual_files_reject_credentials(self):
        resource = self.root / "resources" / "identity"
        resource.parent.mkdir()
        resource.write_bytes(KEY)
        for source in (resource, resource.parent):
            with self.subTest(source=source):
                self.assertEqual(self.scan(source)[0], 1)

    def test_existing_credential_formats_are_rejected(self):
        for content in (
            b'[deployment]\nrealm_client_key = "identity.key"\n',
            b"-----BEGIN PRIVATE KEY-----\nSYNTHETIC_REVIEW_DATA\n",
            json.dumps({"nested": [{"signing_key": "SYNTHETIC_REVIEW_DATA"}]}).encode(),
            json.dumps({"schema": "actr.realm-invite/v1", "secret": "SYNTHETIC_REVIEW_DATA"}).encode(),
        ):
            with self.subTest(content_type=content[:20]):
                resource = self.root / "resource"
                resource.write_bytes(content)
                self.assertEqual(self.scan(resource)[0], 1)

    def test_safe_resources_and_compiled_code_are_allowed(self):
        # Parser/protocol constants inside compiled code are not credential files.
        for suffix, entries in (
            ("ipa", {"Payload/Test.app/config": b'{"realm":"review@example.com"}',
                     "Payload/Test.app/Test": b"\xcf\xfa\xed\xfe" + KEY,
                     "Payload/Test.app/Frameworks/Test.framework/Test": b"\xca\xfe\xba\xbe" + KEY}),
            ("apk", {"res/raw/readme.txt": b"No credentials here",
                     "classes.dex": b"dex\n035\0" + INVITE,
                     "lib/arm64-v8a/libactr.so": b"\x7fELF" + KEY}),
        ):
            with self.subTest(suffix=suffix):
                result, diagnostics = self.scan(self.archive(suffix, entries))
                self.assertEqual((result, diagnostics), (0, ""))

    def test_oversized_resources_fail_in_archives_and_source_directories(self):
        # Use a small limit to exercise bounded reads without a huge test fixture.
        content = b" " * 64 + INVITE
        resource = self.root / "resource.txt"
        resource.write_bytes(content)
        archive = self.archive("ipa", {"Payload/Test.app/resource.txt": content})
        with patch.object(GATE, "MAX_RESOURCE_BYTES", 64):
            for source in (archive, resource.parent):
                with self.subTest(source=source):
                    result, diagnostics = self.scan(source)
                    self.assertEqual(result, 1)
                    self.assertIn("exceeds", diagnostics)
                    self.assertNotIn(INVITE.decode(), diagnostics)

    def test_resource_at_the_size_limit_is_scanned(self):
        resource = self.root / "resource"
        resource.write_bytes(INVITE)
        with patch.object(GATE, "MAX_RESOURCE_BYTES", len(INVITE)):
            result, diagnostics = self.scan(resource)
        self.assertEqual(result, 1)
        self.assertIn("Unsafe", diagnostics)

    def test_missing_and_invalid_archives_fail_without_echoing_contents(self):
        archive = self.root / "invalid.apk"
        archive.write_bytes(INVITE)
        for source in (archive, self.root / "missing"):
            with self.subTest(source=source):
                result, diagnostics = self.scan(source)
                self.assertEqual(result, 1)
                self.assertIn("Cannot inspect", diagnostics)
                self.assertNotIn(INVITE.decode(), diagnostics)

    def test_read_errors_fail_closed(self):
        resource = self.root / "resource"
        resource.write_bytes(b"safe")
        with patch.object(Path, "open", side_effect=OSError("SYNTHETIC_REVIEW_DATA")):
            result, diagnostics = self.scan(resource)
        self.assertEqual(result, 1)
        self.assertNotIn("SYNTHETIC_REVIEW_DATA", diagnostics)

    def test_every_input_is_checked_after_an_unsafe_resource(self):
        first = self.root / "first"
        second = self.root / "second"
        first.write_bytes(KEY)
        second.write_bytes(INVITE)
        result, diagnostics = self.scan(first, second)
        self.assertEqual(result, 1)
        self.assertIn(str(first), diagnostics)
        self.assertIn(str(second), diagnostics)


if __name__ == "__main__":
    unittest.main()
