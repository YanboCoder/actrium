#!/usr/bin/env python3
"""Verify the RTC and TURN dependency sources and RTC submodule."""

import argparse
import json
from pathlib import Path
import subprocess
import tomllib


REPO = Path(__file__).resolve().parents[2]
CLIENT_CRATES = ("webrtc", "rtc", "rtc-turn", "rtc-rtp")
SERVER_CRATES = ("turn", "stun", "webrtc-util")
RTC_SUBMODULE_REV = "cd3f08a243ced3714c965070b4fa7105dfe89c0e"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=REPO / "Cargo.toml")
    parser.add_argument("--client-only", action="store_true")
    args = parser.parse_args()
    manifest = args.manifest.resolve()
    config = tomllib.loads((REPO / "Cargo.toml").read_text())
    dependencies = config["workspace"]["dependencies"]
    client_source = dependencies["webrtc"]
    server_source = dependencies["turn_crate"]
    for name in ("rtc", "rtc-rtp"):
        dependency = dependencies[name]
        if (dependency.get("git"), dependency.get("rev")) != (client_source["git"], client_source["rev"]):
            raise SystemExit(f"{name} must use the same Git snapshot as webrtc")
    for name in ("turn_crate", "webrtc-stun", "stun-protocol", "webrtc-util"):
        dependency = dependencies[name]
        if dependency.get("version") != server_source["version"] or "git" in dependency or "path" in dependency:
            raise SystemExit(f"{name} must use the same registry version as turn")
    if not server_source["version"].startswith("="):
        raise SystemExit("turn must use an exact registry version")
    output = subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1", "--manifest-path", str(manifest)],
        cwd=manifest.parent,
        text=True,
    )
    metadata = json.loads(output)
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    names = CLIENT_CRATES if args.client_only else CLIENT_CRATES + SERVER_CRATES
    resolved = {}
    for name in names:
        matching = [package for package in packages.values() if package["name"] == name and package["id"] in nodes]
        if len(matching) != 1:
            raise SystemExit(f"expected one resolved {name}, found {len(matching)}")
        package = matching[0]
        if name in CLIENT_CRATES:
            expected = f'git+{client_source["git"]}?rev={client_source["rev"]}#{client_source["rev"]}'
        else:
            expected = "registry+https://github.com/rust-lang/crates.io-index"
            if package["version"] != server_source["version"].removeprefix("="):
                raise SystemExit(f'{name} resolved to version {package["version"]}, expected {server_source["version"]}')
        if package["source"] != expected:
            raise SystemExit(f'{name} resolved to {package["source"]}, expected {expected}')
        resolved[name] = package["id"]

    rtc_root = Path(packages[resolved["rtc-turn"]]["manifest_path"]).parents[1]
    submodule_rev = subprocess.check_output(
        ["git", "-C", str(rtc_root), "rev-parse", "HEAD"], text=True
    ).strip()
    if submodule_rev != RTC_SUBMODULE_REV:
        raise SystemExit(f"RTC submodule is {submodule_rev}, expected {RTC_SUBMODULE_REV}")

    consumers = ("actr-hyper",) if args.client_only else ("actr-hyper", "loran-turn")
    for name in consumers:
        matching = [package for package in packages.values() if package["name"] == name and package["id"] in nodes]
        if len(matching) != 1:
            raise SystemExit(f"expected one resolved {name}, found {len(matching)}")
        resolved[name] = matching[0]["id"]

    edges = [
        ("actr-hyper", "webrtc"),
        ("actr-hyper", "rtc"),
        ("actr-hyper", "rtc-rtp"),
        ("webrtc", "rtc"),
        ("rtc", "rtc-turn"),
        ("rtc", "rtc-rtp"),
    ]
    if not args.client_only:
        edges += [
            ("loran-turn", "turn"),
            ("loran-turn", "webrtc-util"),
            ("turn", "stun"),
            ("turn", "webrtc-util"),
        ]
    for parent, dependency in edges:
        actual = {dep["pkg"] for dep in nodes[resolved[parent]]["deps"]}
        if resolved[dependency] not in actual:
            raise SystemExit(f"{parent} does not use the expected {dependency} source")
    print(f"Verified RTC and TURN sources for {manifest}")


if __name__ == "__main__":
    main()
