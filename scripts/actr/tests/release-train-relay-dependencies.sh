#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
python3 - "$repo_root" <<'PY'
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib

repo = Path(sys.argv[1])
script = (repo / "scripts/actr/release-train.sh").read_text()
groups = ["FOUNDATION_CRATES", "PROTOC_CRATES", "SDK_CRATES", "CLI_CRATES", "LORAN_CRATES"]
order = []
for group in groups:
    block = re.search(r"readonly " + group + r"=\((.*?)\n\)", script, re.S).group(1)
    order.extend(re.findall(r'^\s*"([^"\n]+)"', block, re.M))
assert len(set(order)) == len(order), "duplicate release packages"
positions = {name: index for index, name in enumerate(order)}
new_packages = {"actr-relay-security", "loran-relay-protocol", "loran-relay"}
assert new_packages <= set(order), "relay crates missing from release train"

manifest_paths = []
for pattern in ["crates/*/Cargo.toml", "tools/actr/protoc-gen/*/Cargo.toml", "packages/actr-web/crates/*/Cargo.toml"]:
    manifest_paths.extend(repo.glob(pattern))
packages = {}
for path in manifest_paths:
    manifest = tomllib.loads(path.read_text())
    if "package" in manifest:
        packages[manifest["package"]["name"]] = (path, manifest)


def check_dependencies(root, expected_version=None):
    workspace = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["dependencies"]
    for name in order:
        original_path, _ = packages[name]
        path = root / original_path.relative_to(repo)
        manifest = tomllib.loads(path.read_text())
        version = manifest["package"]["version"]
        if expected_version and name in new_packages:
            if isinstance(version, dict):
                version = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"]
            assert version == expected_version, (name, version)
        sections = [manifest, *manifest.get("target", {}).values()]
        for section in sections:
            for table in ["dependencies", "build-dependencies"]:
                for alias, dependency in section.get(table, {}).items():
                    if not isinstance(dependency, dict):
                        continue
                    if dependency.get("workspace"):
                        dependency = workspace.get(alias, dependency)
                    if not isinstance(dependency, dict):
                        continue
                    dep_name = dependency.get("package", alias)
                    if dep_name not in positions:
                        continue
                    assert dependency.get("version"), (name, alias, "missing publishable version")
                    assert positions[dep_name] < positions[name], (name, "must follow", dep_name)
                    if expected_version and (dep_name in new_packages or name == "actr-hyper"):
                        assert dependency["version"] == expected_version, (name, alias, dependency["version"])


check_dependencies(repo)
upstream_names = ("webrtc", "rtc", "rtc-rtp", "turn_crate", "webrtc-stun", "stun-protocol", "webrtc-util")
workspace_deps = tomllib.loads((repo / "Cargo.toml").read_text())["workspace"]["dependencies"]
upstream_sources = {name: {key: workspace_deps[name].get(key) for key in ("version", "git", "rev")} for name in upstream_names}
# Execute the real release version updater against isolated copies, never the checkout.
updater = script.split("update_versions() {", 1)[1].split("<<'PY'\n", 1)[1].split("\nPY\n", 1)[0]
with tempfile.TemporaryDirectory(prefix="actr-relay-release-") as directory:
    copied = Path(directory)
    for relative in set(re.findall(r'repo / "([^"]+)"', updater)):
        source = repo / relative
        if source.is_file():
            destination = copied / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)
    for name in order:
        source, _ = packages[name]
        destination = copied / source.relative_to(repo)
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
    version = "0.11.0-review-test"
    subprocess.run([sys.executable, "-", str(copied), version, "true"], input=updater, text=True, check=True)
    check_dependencies(copied, version)
    updated_deps = tomllib.loads((copied / "Cargo.toml").read_text())["workspace"]["dependencies"]
    for name, source in upstream_sources.items():
        assert {key: updated_deps[name].get(key) for key in source} == source, (name, "release bump changed upstream source")
print("Release dependency order, publishable versions and isolated version bump passed.")
PY
