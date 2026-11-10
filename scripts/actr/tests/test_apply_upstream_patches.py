#!/usr/bin/env python3
"""Check that registry consumers can apply upstream overrides safely."""

import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "apply-upstream-patches.py"
spec = importlib.util.spec_from_file_location("apply_upstream_patches", SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ApplyUpstreamPatchesTest(unittest.TestCase):
    def test_source_consumer_resolves_without_workspace_overrides(self):
        with tempfile.TemporaryDirectory(prefix="actr-source-consumer-") as directory:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "src/main.rs").write_text("fn main() {}\n")
            manifest = root / "Cargo.toml"
            manifest.write_text(f'''[package]
name = "actr-source-consumer-check"
version = "0.1.0"
edition = "2024"

[workspace]

[dependencies]
actr-hyper = {{ path = {json.dumps((module.REPO / 'crates/actr-hyper').as_posix())}, features = ["test-utils"] }}
loran-stun = {{ path = {json.dumps((module.REPO / 'crates/loran-stun').as_posix())} }}
loran-turn = {{ path = {json.dumps((module.REPO / 'crates/loran-turn').as_posix())} }}
''')
            shutil.copyfile(module.REPO / "Cargo.lock", root / "Cargo.lock")
            subprocess.run(
                ["cargo", "metadata", "--format-version", "1", "--manifest-path", str(manifest)],
                stdout=subprocess.DEVNULL,
                check=True,
            )
            subprocess.run(
                [sys.executable, str(module.REPO / "scripts/actr/check-upstream-patches.py"), "--manifest", str(manifest)],
                check=True,
            )

    def test_generated_overrides_follow_workspace_dependencies(self):
        patches = tomllib.loads(module.PATCH_SOURCE.read_text())["patch"]["crates-io"]
        dependencies = tomllib.loads((module.REPO / "Cargo.toml").read_text())["workspace"]["dependencies"]
        for package, dependency in (
            ("webrtc", "webrtc"), ("rtc", "rtc"), ("rtc-turn", "rtc"),
            ("rtc-rtp", "rtc-rtp"),
        ):
            with self.subTest(package=package):
                self.assertEqual(patches[package]["git"], dependencies[dependency]["git"])
                self.assertEqual(patches[package]["rev"], dependencies[dependency]["rev"])

    def test_loran_consumer_sqlx_override_resolves_with_patched_sqlite(self):
        dependencies = tomllib.loads((module.REPO / "Cargo.toml").read_text())["workspace"]["dependencies"]
        patch = dependencies["sqlx"]
        rusqlite = dependencies["rusqlite"]
        with tempfile.TemporaryDirectory(prefix="actr-registry-sqlx-") as directory:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "src/main.rs").write_text("fn main() {}\n")
            manifest = root / "Cargo.toml"
            manifest.write_text(f'''[package]
name = "actr-registry-sqlx-check"
version = "0.1.0"
edition = "2024"

[workspace]

[dependencies]
sqlx = {{ version = {json.dumps(patch["version"])}, default-features = false, features = ["runtime-tokio", "sqlite"] }}
rusqlite = {{ version = {json.dumps(rusqlite["version"])}, features = ["bundled"] }}

[patch.crates-io]
sqlx = {{ git = {json.dumps(patch["git"])}, rev = {json.dumps(patch["rev"])} }}
''')
            subprocess.run(["cargo", "generate-lockfile", "--manifest-path", str(manifest)], check=True)
            packages = tomllib.loads((root / "Cargo.lock").read_text())["package"]
            sqlite = [package for package in packages if package["name"] == "libsqlite3-sys"]
            self.assertEqual(len(sqlite), 1)
            self.assertGreaterEqual(tuple(map(int, sqlite[0]["version"].split("."))), (0, 39, 0))
            for name in ("sqlx", "sqlx-sqlite"):
                matches = [package for package in packages if package["name"] == name]
                self.assertEqual(len(matches), 1)
                self.assertTrue(matches[0]["source"].startswith(f'git+{patch["git"]}?rev={patch["rev"]}#'))

    def test_merges_and_is_idempotent(self):
        with tempfile.TemporaryDirectory() as directory:
            manifest = Path(directory) / "Cargo.toml"
            manifest.write_text('[package]\nname = "demo"\nversion = "0.1.0"\n\n[patch.crates-io]\nactr = { path = "../actr" }\n')
            module.apply(module.PATCH_SOURCE, manifest)
            updated = manifest.read_text()
            parsed = tomllib.loads(updated)
            self.assertEqual(parsed["patch"]["crates-io"]["actr"]["path"], "../actr")
            source = tomllib.loads(module.PATCH_SOURCE.read_text())["patch"]
            for registry, entries in source.items():
                for name, expected in entries.items():
                    self.assertEqual(parsed["patch"][registry][name], expected)
            module.apply(module.PATCH_SOURCE, manifest)
            self.assertEqual(manifest.read_text(), updated)

    def test_rejects_conflicting_patch_without_writing(self):
        with tempfile.TemporaryDirectory() as directory:
            manifest = Path(directory) / "Cargo.toml"
            original = '[package]\nname = "demo"\nversion = "0.1.0"\n\n[patch.crates-io]\nwebrtc = { path = "../other" }\n'
            manifest.write_text(original)
            with self.assertRaisesRegex(ValueError, "conflicting patch for webrtc"):
                module.apply(module.PATCH_SOURCE, manifest)
            self.assertEqual(manifest.read_text(), original)


if __name__ == "__main__":
    unittest.main()
