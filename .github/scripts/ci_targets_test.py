#!/usr/bin/env python3
from __future__ import annotations

import unittest

import ci_targets


class CiTargetsTest(unittest.TestCase):
    def test_mobile_fixture_compile_regression_runs_the_rust_gate(self) -> None:
        path = "scripts/actr/tests/test_mobile_fixture_cfg.py"
        targets, reasons = ci_targets.detect_targets([path], full_run=False)

        self.assertTrue(targets["rust_core"])
        self.assertIn(f"mobile_fixture_cfg:{path}", reasons)

    def test_upstream_overrides_run_the_full_gate(self) -> None:
        for path in (
            "Cargo.toml",
            "scripts/actr/check-upstream-patches.py",
            "scripts/actr/apply-upstream-patches.py",
            "scripts/actr/tests/test_apply_upstream_patches.py",
        ):
            with self.subTest(path=path):
                targets, reasons = ci_targets.detect_targets([path], full_run=False)
                self.assertTrue(all(targets.values()))
                self.assertIn(f"full_trigger:{path}", reasons)

    def test_committed_kotlin_binding_runs_native_checksum_and_kotlin_checks(self) -> None:
        path = "packages/actr-kotlin/actr-kotlin/src/main/kotlin/io/actrium/actr/actr.kt"
        targets, reasons = ci_targets.detect_targets([path], full_run=False)

        self.assertTrue(targets["rust_core"])
        self.assertTrue(targets["kotlin_binding"])
        self.assertTrue(targets["release_related"])
        self.assertFalse(targets["swift_binding"])
        self.assertIn(f"binding_checksums:{path}", reasons)
        self.assertIn(f"kotlin:{path}", reasons)

    def test_checksum_checker_runs_native_checksum_gate(self) -> None:
        path = "scripts/actr/check-binding-checksums.py"
        targets, reasons = ci_targets.detect_targets([path], full_run=False)

        self.assertTrue(targets["rust_core"])
        self.assertTrue(targets["release_related"])
        self.assertFalse(targets["kotlin_binding"])
        self.assertIn(f"binding_checksums:{path}", reasons)

    def test_other_kotlin_sources_keep_sdk_only_checks(self) -> None:
        path = "packages/actr-kotlin/actr-kotlin/src/main/kotlin/io/actrium/actr/dsl/Manifest.kt"
        targets, reasons = ci_targets.detect_targets([path], full_run=False)

        self.assertTrue(targets["kotlin_binding"])
        self.assertFalse(targets["rust_core"])
        self.assertIn(f"kotlin:{path}", reasons)

    def test_error_message_generation_changes_check_both_sdks(self) -> None:
        for path in (
            "scripts/actr/patch-error-messages.py",
            "scripts/actr/tests/test_patch_error_messages.py",
            "scripts/actr/tests/fixtures/ActrError.swift",
        ):
            with self.subTest(path=path):
                targets, reasons = ci_targets.detect_targets([path], full_run=False)

                self.assertTrue(targets["swift_binding"])
                self.assertTrue(targets["kotlin_binding"])
                self.assertTrue(targets["release_related"])
                self.assertFalse(targets["rust_core"])
                self.assertIn(f"sdk_error_messages:{path}", reasons)

    def test_e2e_rust_sources_and_manifests_trigger_compile_check(self) -> None:
        paths = (
            "tests/e2e/actr/Cargo.toml",
            "tests/e2e/actr/Cargo.lock",
            "tests/e2e/actr/polyglot-echo/clients/rust/Cargo.toml",
            "tests/e2e/actr/polyglot-echo/clients/rust/src/main.rs",
        )

        for path in paths:
            with self.subTest(path=path):
                targets, reasons = ci_targets.detect_targets([path], full_run=False)

                self.assertTrue(targets["e2e_rust"])
                self.assertIn(f"e2e_rust:{path}", reasons)

    def test_package_runtime_shell_changes_gate_the_scenario(self) -> None:
        targets, reasons = ci_targets.detect_targets(
            ["tests/e2e/actr/package-runtime-echo/run.sh"],
            full_run=False,
        )

        self.assertTrue(targets["e2e_package_runtime"])
        self.assertFalse(targets["e2e_rust"])
        self.assertIn("e2e_package_runtime:tests/e2e/actr/package-runtime-echo/run.sh", reasons)

    def test_package_runtime_rust_sources_keep_the_compile_check(self) -> None:
        targets, reasons = ci_targets.detect_targets(
            ["tests/e2e/actr/package-runtime-echo/Cargo.toml"],
            full_run=False,
        )

        self.assertTrue(targets["e2e_package_runtime"])
        self.assertTrue(targets["e2e_rust"])
        self.assertIn("e2e_rust:tests/e2e/actr/package-runtime-echo/Cargo.toml", reasons)

    def test_c_and_go_echo_workloads_trigger_component_builds(self) -> None:
        paths = (
            "examples/actr/c/echo-workload/src/main.c",
            "examples/actr/go/echo-workload/build.sh",
        )

        for path in paths:
            with self.subTest(path=path):
                targets, reasons = ci_targets.detect_targets([path], full_run=False)

                self.assertTrue(targets["c_go_workloads"])
                self.assertIn(f"c_go_workload:{path}", reasons)

    def test_v2_wit_change_rebuilds_c_and_go_components(self) -> None:
        path = "crates/actr-framework/wit-v2/actr-workload.wit"
        targets, reasons = ci_targets.detect_targets([path], full_run=False)

        self.assertTrue(targets["c_go_workloads"])
        self.assertTrue(targets["rust_core"])
        self.assertTrue(targets["e2e_rust"])
        self.assertIn(f"core_dependency:{path}", reasons)

    def test_native_workload_smoke_change_rebuilds_components(self) -> None:
        path = "crates/actr-hyper/tests/native_workload_smoke.rs"
        targets, reasons = ci_targets.detect_targets([path], full_run=False)

        self.assertTrue(targets["c_go_workloads"])
        self.assertTrue(targets["rust_core"])
        self.assertIn(f"core_dependency:{path}", reasons)

    def test_loran_change_triggers_rust_checks(self) -> None:
        path = "crates/loran-turn/src/authenticator.rs"
        targets, reasons = ci_targets.detect_targets([path], full_run=False)

        self.assertTrue(targets["rust_core"])
        self.assertFalse(targets["csharp_binding"])
        self.assertIn(f"loran_workspace:{path}", reasons)

    def test_csharp_runtime_dependencies_trigger_csharp_checks(self) -> None:
        paths = (
            "crates/actr-config/src/lib.rs",
            "crates/actr-framework/src/lib.rs",
            "crates/actr-hyper/src/lib.rs",
            "crates/actr-pack/src/lib.rs",
            "crates/actr-platform-traits/src/lib.rs",
            "crates/actr-protocol/src/lib.rs",
            "crates/actr-runtime/src/lib.rs",
            "crates/actr-runtime-mailbox/src/lib.rs",
            "crates/actr-service-compat/src/lib.rs",
        )

        for path in paths:
            with self.subTest(path=path):
                targets, reasons = ci_targets.detect_targets([path], full_run=False)

                self.assertTrue(targets["rust_core"])
                self.assertTrue(targets["csharp_binding"])
                self.assertIn(f"core_dependency:{path}", reasons)

    def test_unrelated_core_change_does_not_trigger_csharp_checks(self) -> None:
        for path in ("crates/actr-hlc/src/lib.rs", "crates/actr-platform-native/src/lib.rs"):
            with self.subTest(path=path):
                targets, reasons = ci_targets.detect_targets([path], full_run=False)

                self.assertTrue(targets["rust_core"])
                self.assertFalse(targets["csharp_binding"])
                self.assertIn(f"core_dependency:{path}", reasons)

    def test_actr_cabi_change_triggers_native_and_binding_checks(self) -> None:
        path = "crates/actr-cabi/src/lib.rs"
        targets, reasons = ci_targets.detect_targets([path], full_run=False)

        self.assertTrue(targets["rust_core"])
        self.assertTrue(targets["swift_binding"])
        self.assertTrue(targets["csharp_binding"])
        self.assertIn(f"actr_cabi:{path}", reasons)

    def test_cabi_swift_change_triggers_swift_checks(self) -> None:
        path = "packages/actr-cabi/swift/Sources/ActrCabi/ActrCabi.swift"
        targets, reasons = ci_targets.detect_targets([path], full_run=False)

        self.assertTrue(targets["swift_binding"])
        self.assertIn(f"swift:{path}", reasons)

    def test_winui_e2e_change_triggers_rust_and_csharp_checks(self) -> None:
        for path in (
            "tests/e2e/actr/winui-echo-app/WinUiEchoApp/ActrService.cs",
            "tests/e2e/actr/winui-echo-app/workload/src/lib.rs",
            "tests/e2e/actr/winui-echo-app/workload/Cargo.toml",
        ):
            with self.subTest(path=path):
                targets, reasons = ci_targets.detect_targets([path], full_run=False)

                self.assertTrue(targets["rust_core"])
                self.assertTrue(targets["csharp_binding"])
                self.assertIn(f"winui_e2e:{path}", reasons)

    def test_typescript_echo_workload_triggers_workload_checks(self) -> None:
        targets, reasons = ci_targets.detect_targets(
            ["examples/actr/typescript/echo-workload/src/actr_service.ts"],
            full_run=False,
        )

        self.assertTrue(targets["ts_workload"])
        self.assertFalse(targets["web_binding"])
        self.assertIn(
            "typescript_workload:examples/actr/typescript/echo-workload/src/actr_service.ts",
            reasons,
        )

    def test_generated_typescript_browser_e2e_source_triggers_light_checks(self) -> None:
        targets, reasons = ci_targets.detect_targets(
            ["crates/actr-cli/tests/e2e_typescript_generated_echo_web.rs"],
            full_run=False,
        )

        self.assertTrue(targets["rust_core"])
        self.assertTrue(targets["ts_workload"])
        self.assertTrue(targets["web_binding"])
        self.assertIn(
            "generated_typescript_web_e2e_source:crates/actr-cli/tests/e2e_typescript_generated_echo_web.rs",
            reasons,
        )

    def test_kotlin_codegen_paths_trigger_rust_and_kotlin_checks(self) -> None:
        paths = (
            "crates/actr-cli/src/commands/codegen/kotlin.rs",
            "crates/actr-cli/tests/kotlin_echo.rs",
            "crates/actr-cli/fixtures/kotlin/echo/MainActivity.kt",
        )

        for path in paths:
            with self.subTest(path=path):
                targets, reasons = ci_targets.detect_targets([path], full_run=False)

                self.assertTrue(targets["rust_core"])
                self.assertTrue(targets["kotlin_binding"])
                self.assertIn(f"kotlin_codegen:{path}", reasons)

    def test_v2_workload_wit_triggers_sdk_checks(self) -> None:
        path = "crates/actr-framework/wit-v2/actr-workload.wit"
        targets, reasons = ci_targets.detect_targets([path], full_run=False)

        self.assertTrue(targets["rust_core"])
        self.assertTrue(targets["python_workload"])
        self.assertTrue(targets["ts_workload"])
        self.assertIn(f"core_dependency:{path}", reasons)

    def test_retained_web_wit_does_not_trigger_workload_sdk_checks(self) -> None:
        path = "crates/actr-framework/wit/actr-workload.wit"
        targets, _ = ci_targets.detect_targets([path], full_run=False)

        self.assertTrue(targets["rust_core"])
        self.assertFalse(targets["python_workload"])
        self.assertFalse(targets["ts_workload"])


if __name__ == "__main__":
    unittest.main()
