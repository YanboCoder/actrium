#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from fnmatch import fnmatch


TARGETS = (
    "rust_core",
    "e2e_rust",
    "e2e_package_runtime",
    "ts_binding",
    "ts_workload",
    "python_codegen",
    "python_workload",
    "swift_binding",
    "kotlin_binding",
    "csharp_binding",
    "web_binding",
    "c_go_workloads",
    "release_related",
)

FULL_TRIGGER_PATTERNS = (
    ".github/actions/**",
    ".github/scripts/**",
    ".github/workflows/**",
    ".cargo/config.toml",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain",
    "rust-toolchain.toml",
    "rustfmt.toml",
    "clippy.toml",
    "deny.toml",
    "CHANGELOG.md",
    "scripts/actr/release-train.sh",
    "scripts/actr/check-upstream-patches.py",
    "scripts/actr/apply-upstream-patches.py",
    "scripts/actr/tests/test_apply_upstream_patches.py",
    "scripts/actr/generate-changelog.py",
    "scripts/actr/merge-release-pr.py",
)

CSHARP_CORE_DEPENDENCY_PREFIXES = (
    "crates/actr-config/",
    "crates/actr-framework/",
    "crates/actr-hyper/",
    "crates/actr-pack/",
    "crates/actr-platform-traits/",
    "crates/actr-protocol/",
    "crates/actr-runtime/",
    "crates/actr-runtime-mailbox/",
    "crates/actr-service-compat/",
)

ACTR_CORE_PREFIXES = (
    "crates/actr/",
    "crates/actr-config/",
    "crates/actr-framework/",
    "crates/actr-hlc/",
    "crates/actr-hyper/",
    "crates/actr-pack/",
    "crates/actr-platform-native/",
    "crates/actr-platform-traits/",
    "crates/actr-protocol/",
    "crates/actr-runtime/",
    "crates/actr-runtime-mailbox/",
    "crates/actr-service-compat/",
)


def run_git_diff(base_sha: str, head_sha: str) -> list[str]:
    result = subprocess.run(
        ["git", "diff", "--name-only", base_sha, head_sha],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    return [line.strip() for line in result.stdout.splitlines() if line.strip()]


def matches_any(path: str, patterns: tuple[str, ...]) -> bool:
    return any(fnmatch(path, pattern) for pattern in patterns)


def set_all(targets: dict[str, bool], value: bool = True) -> None:
    for key in targets:
        targets[key] = value


def detect_targets(changed_files: list[str], full_run: bool) -> tuple[dict[str, bool], list[str]]:
    targets = {name: False for name in TARGETS}
    reasons: list[str] = []

    if full_run:
        set_all(targets, True)
        reasons.append("full_run")
        return targets, reasons

    for path in changed_files:
        if matches_any(path, FULL_TRIGGER_PATTERNS):
            set_all(targets, True)
            reasons.append(f"full_trigger:{path}")
            continue

        if path == "crates/actr-cli/tests/e2e_typescript_generated_echo_web.rs":
            targets["rust_core"] = True
            targets["ts_workload"] = True
            targets["web_binding"] = True
            reasons.append(f"generated_typescript_web_e2e_source:{path}")
            continue

        # Keep this before the generic e2e Rust-workspace rule below. The
        # WinUI workload is a standalone Cargo workspace and also exercises
        # the C# binding, so e2e_rust alone does not cover it.
        if path.startswith("tests/e2e/actr/winui-echo-app/"):
            targets["rust_core"] = True
            targets["csharp_binding"] = True
            reasons.append(f"winui_e2e:{path}")
            continue

        # Gate the package-runtime scenario shell path on PRs (#618): every
        # file in this directory feeds the scenario (shell, fixtures, python
        # heredocs), not just its Rust workspace files. Rust sources keep the
        # workspace compile check on top of the scenario run.
        if path.startswith("tests/e2e/actr/package-runtime-echo/"):
            targets["e2e_package_runtime"] = True
            reasons.append(f"e2e_package_runtime:{path}")
            if path.endswith((".rs", "Cargo.toml", "Cargo.lock")):
                targets["e2e_rust"] = True
                reasons.append(f"e2e_rust:{path}")
            continue

        if path.startswith("tests/e2e/actr/") and path.endswith((".rs", "Cargo.toml", "Cargo.lock")):
            targets["e2e_rust"] = True
            reasons.append(f"e2e_rust:{path}")
            continue

        if path.startswith(ACTR_CORE_PREFIXES):
            targets["rust_core"] = True
            targets["e2e_rust"] = True
            targets["ts_binding"] = True
            targets["swift_binding"] = True
            targets["web_binding"] = True
            if path.startswith(CSHARP_CORE_DEPENDENCY_PREFIXES):
                targets["csharp_binding"] = True
            if path == "crates/actr-framework/wit-v2/actr-workload.wit":
                targets["c_go_workloads"] = True
                targets["python_workload"] = True
                targets["ts_workload"] = True
            elif path == "crates/actr-hyper/tests/native_workload_smoke.rs":
                targets["c_go_workloads"] = True
            reasons.append(f"core_dependency:{path}")
            continue

        if path.startswith("crates/loran-") or path.startswith("loran/"):
            targets["rust_core"] = True
            reasons.append(f"loran_workspace:{path}")
            continue

        if path.startswith("crates/actr-cabi/"):
            targets["rust_core"] = True
            targets["swift_binding"] = True
            targets["csharp_binding"] = True
            reasons.append(f"actr_cabi:{path}")
            continue

        if path.startswith("crates/actr-cli/assets/web-runtime/"):
            targets["rust_core"] = True
            targets["web_binding"] = True
            reasons.append(f"web_runtime_asset:{path}")
            continue

        if path in (
            "crates/actr-cli/src/commands/codegen/kotlin.rs",
            "crates/actr-cli/tests/kotlin_echo.rs",
        ) or path.startswith("crates/actr-cli/fixtures/kotlin/"):
            targets["rust_core"] = True
            targets["kotlin_binding"] = True
            reasons.append(f"kotlin_codegen:{path}")
            continue

        if path.startswith(("crates/actr-cli/", "crates/actr-ffi/")):
            targets["rust_core"] = True
            reasons.append(f"rust_workspace:{path}")
            continue

        if path.startswith("tools/actr/protoc-gen/rust/"):
            targets["rust_core"] = True
            reasons.append(f"rust_codegen:{path}")
            continue

        if path.startswith("tools/actr/protoc-gen/web/"):
            targets["rust_core"] = True
            targets["web_binding"] = True
            reasons.append(f"web_codegen:{path}")
            continue

        if path.startswith("packages/actr-typescript/actr-workload/"):
            targets["ts_workload"] = True
            reasons.append(f"typescript_workload:{path}")
            continue

        if path.startswith(("packages/actr-typescript/", "tools/actr/protoc-gen/typescript/")):
            targets["ts_binding"] = True
            reasons.append(f"typescript:{path}")
            continue

        if path.startswith("packages/actr-python/actr-workload/"):
            targets["python_workload"] = True
            reasons.append(f"python_workload:{path}")
            continue

        if path.startswith("tools/actr/protoc-gen/python/"):
            targets["python_codegen"] = True
            reasons.append(f"python_codegen:{path}")
            continue

        if path.startswith("examples/actr/python/"):
            targets["python_workload"] = True
            reasons.append(f"python_workload:{path}")
            continue

        if path.startswith("examples/actr/typescript/echo-workload/"):
            targets["ts_workload"] = True
            reasons.append(f"typescript_workload:{path}")
            continue

        if path.startswith(
            (
                "examples/actr/c/echo-workload/",
                "examples/actr/go/echo-workload/",
            )
        ):
            targets["c_go_workloads"] = True
            reasons.append(f"c_go_workload:{path}")
            continue

        if path.startswith("tests/e2e/actr/typescript-stream/"):
            targets["ts_workload"] = True
            reasons.append(f"typescript_stream_e2e:{path}")
            continue

        if path == "scripts/actr/tests/test_mobile_fixture_cfg.py":
            targets["rust_core"] = True
            reasons.append(f"mobile_fixture_cfg:{path}")
            continue

        # These inputs must run the native checksum gate even without Rust edits.
        # Fall through so the committed binding also selects its normal SDK job.
        if path in (
            "packages/actr-kotlin/actr-kotlin/src/main/kotlin/io/actrium/actr/actr.kt",
            "scripts/actr/check-binding-checksums.py",
        ):
            targets["rust_core"] = True
            reasons.append(f"binding_checksums:{path}")

        if path in (
            "scripts/actr/patch-error-messages.py",
            "scripts/actr/tests/test_patch_error_messages.py",
            "scripts/actr/tests/fixtures/ActrError.swift",
        ):
            targets["swift_binding"] = True
            targets["kotlin_binding"] = True
            reasons.append(f"sdk_error_messages:{path}")
            continue

        if path.startswith(
            ("packages/actr-swift/", "packages/actr-cabi/swift/", "tools/actr/protoc-gen/swift/")
        ):
            targets["swift_binding"] = True
            reasons.append(f"swift:{path}")
            continue

        if path.startswith(("packages/actr-kotlin/", "tools/actr/protoc-gen/kotlin/")):
            targets["kotlin_binding"] = True
            reasons.append(f"kotlin:{path}")
            continue

        if path.startswith("packages/actr-cabi/csharp/"):
            targets["csharp_binding"] = True
            reasons.append(f"csharp:{path}")
            continue

        if path.startswith("packages/actr-web/"):
            targets["web_binding"] = True
            reasons.append(f"web_binding:{path}")
            continue

    if any(targets.values()):
        targets["release_related"] = targets["release_related"] or any(
            target for name, target in targets.items() if name != "release_related"
        )

    return targets, reasons


def write_output(name: str, value: str) -> None:
    output_path = os.environ.get("GITHUB_OUTPUT")
    if not output_path:
        return

    with open(output_path, "a", encoding="utf-8") as output:
        output.write(f"{name}={value}\n")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--event-name", required=True)
    parser.add_argument("--base-sha", default="")
    parser.add_argument("--head-sha", default="")
    parser.add_argument("--manual-full", default="false")
    args = parser.parse_args()

    event_name = args.event_name
    manual_full = args.manual_full.lower() == "true"
    full_run = manual_full or event_name != "pull_request"

    if full_run:
        changed_files: list[str] = []
    else:
        if not args.base_sha or not args.head_sha:
            print("Missing base/head SHA for pull_request diff", file=sys.stderr)
            return 1
        changed_files = run_git_diff(args.base_sha, args.head_sha)

    targets, reasons = detect_targets(changed_files, full_run=full_run)

    write_output("mode", "full" if full_run else "affected")
    write_output("changed_files_json", json.dumps(changed_files))
    write_output("reasons_json", json.dumps(reasons))

    for name, value in targets.items():
        write_output(name, "true" if value else "false")

    summary = {
        "mode": "full" if full_run else "affected",
        "changed_files": changed_files,
        "reasons": reasons,
        "targets": targets,
    }
    print(json.dumps(summary, indent=2, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
