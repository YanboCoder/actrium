#!/usr/bin/env python3
"""Compile the mobile Issuer fixture on a non-Unix target without its dependencies."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[3]
SOURCE = ROOT / "crates/loran-issuer/tests/device_membership/mod.rs"


class MobileFixtureCfgTests(unittest.TestCase):
    def test_unix_only_fixture_is_excluded_on_non_unix_targets(self):
        # The standalone fixture is the final item in this test module. Compile
        # its original attributes and body, including the Unix-only imports;
        # removing its platform guard must fail before any mobile tests run.
        source = SOURCE.read_text()
        fixture = source[source.index("/// Started explicitly by the mobile SDK integration harness."):]
        with tempfile.TemporaryDirectory() as directory:
            fixture_file = Path(directory) / "mobile_fixture.rs"
            fixture_file.write_text(fixture)
            result = subprocess.run([
                os.environ.get("RUSTC", "rustc"), "--edition=2024", "--crate-type=lib",
                "--target=wasm32-wasip2", "--emit=metadata", str(fixture_file),
                "-o", str(Path(directory) / "mobile_fixture.rmeta"),
            ], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
