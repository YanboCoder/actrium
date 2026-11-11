#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo_root"

if ! command -v rg >/dev/null 2>&1; then
  echo "error: ripgrep (rg) is required for Rust boundary hygiene checks" >&2
  exit 127
fi

failures=0

record_failure() {
  failures=$((failures + 1))
}

print_ok() {
  echo "[ok] $1"
}

print_failure() {
  echo
  echo "[fail] $1"
  record_failure
}

browser_sync_product_direction_requirements=(
  "docs/architecture/actrium/browser-sync-cloud-drive.md|Actrium should not operate a browser sync service|Browser sync architecture rejects an Actrium-operated product sync service"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|must not embed a sync server|Browser sync architecture rejects an embedded browser sync server"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|Provider Model|Browser sync architecture defines a cloud provider boundary"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|BrowserStorageSyncCloudStore|Browser sync architecture names the Rust cloud store trait boundary"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|cloud_store_contract_json|Browser sync architecture names the cloud store binding contract"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_provider_connection_status_policy_json|Browser sync architecture names the Rust provider connection status policy"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_provider_error_policy_json|Browser sync architecture names the Rust provider error policy"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_provider_error_classification_json|Browser sync architecture names the Rust provider error classifier"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|providerErrorClassification|Browser sync architecture records provider error diagnostics source mapping"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|nested round/client result shapes|Browser sync architecture records nested provider round error extraction"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|localFolderProviderRoundErrors|Browser sync architecture records the local-folder provider round error source"
  "docs/architecture/actrium/rust-core.md|nested round/client result shapes|Rust core doc records nested provider round error extraction"
  "docs/architecture/actrium/rust-core.md|localFolderProviderRoundErrors|Rust core doc records the local-folder provider round error source"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_store_capability_preflight_json|Browser sync architecture names the Rust cloud store capability preflight"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_push_plan_json|Browser sync architecture names the Rust cloud push write planner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_push_result_plan_json|Browser sync architecture names the Rust cloud push result planner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|writeSummary|Browser sync architecture records cloud push write summaries"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|ackReason|Browser sync architecture records cloud push ack reasons"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_pull_plan_json|Browser sync architecture names the Rust cloud pull read planner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|readSummary|Browser sync architecture records cloud pull read summaries"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_pull_apply_plan_json|Browser sync architecture names the Rust cloud pull apply planner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|skippedSnapshots|Browser sync architecture records cloud pull apply skipped-snapshot diagnostics"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|skippedLogs|Browser sync architecture records cloud pull apply skipped-log diagnostics"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|applyAction|Browser sync architecture records cloud pull apply action diagnostics"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|applySummary|Browser sync architecture records cloud pull apply summaries"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_pull_continuation_plan_json|Browser sync architecture names the Rust cloud pull continuation planner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|continuationSummary|Browser sync architecture records cloud pull continuation summaries"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_execute_planned_reads_with_continuations|Browser sync architecture records Rust-backed pull pagination execution"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|paginationLimitExceeded|Browser sync architecture records pull pagination safety diagnostics"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_compatibility_status_json|Browser sync architecture names the Rust cloud compatibility gate"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_compatibility_preflight_json|Browser sync architecture names the Rust batched cloud compatibility preflight"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_schema_rollout_plan_json|Browser sync architecture names the Rust cloud schema rollout plan"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_schema_migration_playbook_json|Browser sync architecture names the Rust cloud schema migration playbook"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_recovery_snapshot_plan_json|Browser sync architecture names the Rust cloud recovery snapshot planner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_recovery_snapshot_result_plan_json|Browser sync architecture names the Rust cloud recovery snapshot result handoff"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_local_folder_recovery_snapshot_round_json|Browser sync architecture names the Rust local-folder recovery snapshot round"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|BrowserStorageSyncCloudRecoverySnapshotRoundSummary|Browser sync architecture names the Rust cloud recovery snapshot round summary"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|writeRecoveryCopy|Browser sync architecture records cloud recovery snapshot copy operations"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_membership_plan_json|Browser sync architecture names the Rust cloud device membership planner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_membership_result_plan_json|Browser sync architecture names the Rust cloud device membership result planner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|writeReason|Browser sync architecture records cloud planned write reasons"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|readReason|Browser sync architecture records cloud planned read reasons"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_compaction_plan_json|Browser sync architecture names the Rust cloud compaction planner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_compaction_result_plan_json|Browser sync architecture names the Rust cloud compaction result planner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_local_folder_compaction_round_json|Browser sync architecture names the Rust local-folder compaction round"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|BrowserStorageSyncCloudCompactionRoundSummary|Browser sync architecture names the Rust cloud compaction round summary"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|deleteReason|Browser sync architecture records cloud planned delete reasons"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_high_watermark_policy_json|Browser sync architecture names the Rust cloud high-watermark policy"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_high_watermark_plan_json|Browser sync architecture names the Rust cloud high-watermark retention planner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_cleanup_result_plan_json|Browser sync architecture names the Rust cloud cleanup result planner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_local_folder_cleanup_round_json|Browser sync architecture names the Rust local-folder cleanup round"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|BrowserStorageSyncCloudCleanupRoundSummary|Browser sync architecture names the Rust cloud cleanup round summary"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|BrowserStorageSyncMemoryCloudStore|Browser sync architecture names the Rust in-memory cloud store fixture"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|BrowserStorageSyncLocalFolderCloudStore|Browser sync architecture names the Rust local-folder cloud store fixture"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|not a per-operation FFI surface|Browser sync architecture keeps local-folder store coarse-grained"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_local_folder_capabilities_json|Browser sync architecture names the Rust local-folder capability helper"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_local_folder_capability_preflight_json|Browser sync architecture names the Rust local-folder capability preflight helper"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_local_folder_compatibility_preflight_json|Browser sync architecture names the Rust local-folder compatibility preflight helper"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_local_folder_cycle_round_json|Browser sync architecture names the Rust local-folder cycle JSON entry"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_local_folder_membership_round_json|Browser sync architecture names the Rust local-folder membership JSON entry"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_execute_planned_writes|Browser sync architecture names the Rust cloud planned write executor"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_run_membership_round|Browser sync architecture names the Rust cloud membership round runner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|BrowserStorageSyncCloudMembershipRoundSummary|Browser sync architecture names the Rust cloud membership round summary"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_run_push_round|Browser sync architecture names the Rust cloud push round runner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|BrowserStorageSyncCloudPushRoundSummary|Browser sync architecture names the Rust cloud push round summary"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_run_pull_round|Browser sync architecture names the Rust cloud pull round runner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|BrowserStorageSyncCloudPullRoundSummary|Browser sync architecture names the Rust cloud pull round summary"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|roundSummary|Browser sync architecture records cloud pull round summaries"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_run_cycle_round|Browser sync architecture names the Rust cloud cycle round runner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|BrowserStorageSyncCloudCycleRoundSummary|Browser sync architecture names the Rust cloud cycle round summary"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|client-step categories|Browser sync architecture records cloud cycle round summaries"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|BrowserStorageSyncCloudCycleClientPlan|Browser sync architecture names the Rust cloud cycle client plan"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|stepReason|Browser sync architecture records cloud cycle client step reasons"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_cycle_status_policy_json|Browser sync architecture names the Rust cloud cycle status policy"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_membership_status_policy_json|Browser sync architecture names the Rust cloud membership status policy"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_cycle_schedule_state_json|Browser sync architecture names the Rust cloud cycle schedule state"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_membership_schedule_state_json|Browser sync architecture names the Rust cloud membership schedule state"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_membership_attempt_plan_json|Browser sync architecture names the Rust cloud membership attempt plan"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_membership_loop_plan_json|Browser sync architecture names the Rust cloud membership loop plan"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|BrowserStorageSyncCloudMembershipLoopSummary|Browser sync architecture names the Rust cloud membership loop summary"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|membership \`loopSummary\`|Browser sync architecture records cloud membership loop summaries"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_provider_diagnostics_json|Browser sync architecture names the Rust cloud provider diagnostics summary"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_observability_event_json|Browser sync architecture names the Rust cloud observability event contract"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_cycle_attempt_plan_json|Browser sync architecture names the Rust cloud cycle attempt plan"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_sync_cloud_cycle_loop_plan_json|Browser sync architecture names the Rust cloud cycle loop plan"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|BrowserStorageSyncCloudCycleLoopSummary|Browser sync architecture names the Rust cloud cycle loop summary"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|cycle \`loopSummary\`|Browser sync architecture records cloud cycle loop summaries"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_address_canonicalization_policy_json|Browser sync architecture names the Rust address canonicalization policy"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_canonicalize_addresses_json|Browser sync architecture names the Rust batched address canonicalizer"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|credentialStripped|Browser sync architecture records Rust-owned URL credential stripping diagnostics"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|canonicalizationSummary|Browser sync architecture records address canonicalization summaries"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_bookmark_folder_tree_integrity_policy_json|Browser sync architecture names the Rust bookmark folder tree integrity policy"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_bookmark_folder_tree_integrity_plan_json|Browser sync architecture names the Rust bookmark folder tree integrity planner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|integrityAction|Browser sync architecture records bookmark folder integrity diagnostics"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|integritySummary|Browser sync architecture records bookmark folder integrity summaries"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_bookmark_restore_placement_policy_json|Browser sync architecture names the Rust bookmark restore placement policy"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_bookmark_restore_placement_plan_json|Browser sync architecture names the Rust bookmark restore placement planner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|placementReason|Browser sync architecture records bookmark restore placement reasons"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|stateConflictAction|Browser sync architecture records bookmark delete/restore conflict diagnostics"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|stateConflictReason|Browser sync architecture records bookmark delete/restore conflict reasons"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|stateConflicts|Browser sync architecture records bookmark delete/restore conflict details"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|stateConflictSummary|Browser sync architecture records bookmark delete/restore conflict summaries"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_bookmark_ownership_policy_json|Browser sync architecture names the Rust bookmark ownership policy"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_bookmark_ownership_plan_json|Browser sync architecture names the Rust bookmark ownership planner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|ownershipSummary|Browser sync architecture records bookmark ownership summaries"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_history_visit_aggregation_policy_json|Browser sync architecture names the Rust history visit aggregation policy"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_history_visit_aggregation_plan_json|Browser sync architecture names the Rust history visit aggregation planner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|aggregationAction|Browser sync architecture records history visit aggregation diagnostics"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|aggregationSummary|Browser sync architecture records history visit aggregation summaries"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_session_tab_ordering_policy_json|Browser sync architecture names the Rust session tab ordering policy"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_session_tab_ordering_plan_json|Browser sync architecture names the Rust session tab ordering planner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|orderingAction|Browser sync architecture records session tab ordering diagnostics"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|orderingSummary|Browser sync architecture records session tab ordering summaries"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_session_active_tab_consistency_policy_json|Browser sync architecture names the Rust session active-tab consistency policy"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_session_active_tab_consistency_plan_json|Browser sync architecture names the Rust session active-tab consistency planner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|consistencyReason|Browser sync architecture records session active-tab consistency diagnostics"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|consistencySummary|Browser sync architecture records session active-tab consistency summaries"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_retention_pruning_policy_json|Browser sync architecture names the Rust retention pruning policy"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|browser_storage_retention_pruning_plan_json|Browser sync architecture names the Rust retention pruning planner"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|retentionReason|Browser sync architecture records retention pruning diagnostics"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|retentionSummary|Browser sync architecture records retention pruning summaries"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|maxHistoryPagesPerOriginDevice|Browser sync architecture records Rust-owned per-origin-device history pruning"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|maxSessionTabsPerOriginDevice|Browser sync architecture records Rust-owned per-origin-device session-tab pruning"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|manifest.json|Browser sync architecture defines cloud manifest/log storage"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|No Actrium product server allocates device identity|Browser sync architecture rejects server-side device registration"
  "docs/architecture/actrium/browser-sync-cloud-drive.md|Sharing a data structure and syncing files is necessary, but not sufficient|Browser sync architecture keeps deterministic sync rules in Rust"
  "docs/architecture/actrium/overview.md|browser-sync-cloud-drive.md|Architecture overview links the cloud-drive browser sync direction"
  "internal/pending-expansion.md|replacement memory for the quoted 2026-07-03 seven-point|Pending expansion tracks cloud-drive browser sync instead of an Actrium-operated sync path"
  "internal/pending-expansion.md|provider-owned file sync|Pending expansion records provider-owned file sync as the replacement mainline"
  "internal/pending-expansion.md|Retired from the mainline|Pending expansion explicitly retires the old hosted-sync backlog"
  "internal/pending-expansion.md|not an Actrium service API|Pending expansion rejects service-side device registration and revocation"
  "internal/pending-expansion.md|browser sync backend plus clients|Pending expansion rejects the old backend-plus-clients framing"
  "internal/pending-expansion.md|P2P/direct-device sync is not the current mainline|Pending expansion rejects P2P/signaling as an Actrium backend track"
  "internal/browser-sync-roadmap.md|P2P/direct-device sync is not the current mainline|Browser sync roadmap rejects P2P/signaling as an Actrium backend track"
  "internal/pending-expansion.md|synchronization substrate only|Pending expansion records that shared files do not replace Rust sync semantics"
  "internal/browser-sync-roadmap.md|synchronization substrate only|Browser sync roadmap records that shared files do not replace Rust sync semantics"
  "internal/browser-sync-roadmap.md|browser sync backend plus clients|Browser sync roadmap rejects the old backend-plus-clients framing"
  "internal/pending-expansion.md|selected provider SDK or cloud-storage adapter surface|Pending expansion records provider auth/TLS/quota as adapter concerns"
  "internal/browser-sync-roadmap.md|selected provider SDK or cloud-storage adapter surface|Browser sync roadmap records provider auth/TLS/quota as adapter concerns"
  "internal/pending-expansion.md|browser_storage_session_tab_ordering_plan_json|Pending expansion records Rust-owned session tab ordering"
  "internal/browser-sync-roadmap.md|browser_storage_session_tab_ordering_plan_json|Browser sync roadmap records Rust-owned session tab ordering"
  "internal/pending-expansion.md|orderingAction|Pending expansion records session tab ordering diagnostics"
  "internal/pending-expansion.md|orderingSummary|Pending expansion records session tab ordering summaries"
  "internal/browser-sync-roadmap.md|orderingAction|Browser sync roadmap records session tab ordering diagnostics"
  "internal/browser-sync-roadmap.md|orderingSummary|Browser sync roadmap records session tab ordering summaries"
  "internal/pending-expansion.md|browser_storage_bookmark_restore_placement_plan_json|Pending expansion records Rust-owned bookmark restore placement"
  "internal/browser-sync-roadmap.md|browser_storage_bookmark_restore_placement_plan_json|Browser sync roadmap records Rust-owned bookmark restore placement"
  "internal/pending-expansion.md|stateConflictAction|Pending expansion records bookmark delete/restore conflict diagnostics"
  "internal/pending-expansion.md|stateConflicts|Pending expansion records bookmark delete/restore conflict details"
  "internal/pending-expansion.md|stateConflictSummary|Pending expansion records bookmark delete/restore conflict summaries"
  "internal/pending-expansion.md|placementReason|Pending expansion records bookmark restore placement reasons"
  "internal/pending-expansion.md|stateConflictReason|Pending expansion records bookmark delete/restore conflict reasons"
  "internal/browser-sync-roadmap.md|stateConflictAction|Browser sync roadmap records bookmark delete/restore conflict diagnostics"
  "internal/browser-sync-roadmap.md|stateConflicts|Browser sync roadmap records bookmark delete/restore conflict details"
  "internal/browser-sync-roadmap.md|stateConflictSummary|Browser sync roadmap records bookmark delete/restore conflict summaries"
  "internal/browser-sync-roadmap.md|placementReason|Browser sync roadmap records bookmark restore placement reasons"
  "internal/browser-sync-roadmap.md|stateConflictReason|Browser sync roadmap records bookmark delete/restore conflict reasons"
  "internal/pending-expansion.md|browser_storage_bookmark_ownership_plan_json|Pending expansion records Rust-owned bookmark ownership/source resolution"
  "internal/pending-expansion.md|retentionReason|Pending expansion records bookmark ownership retention reason diagnostics"
  "internal/pending-expansion.md|suppressedBookmarkIds|Pending expansion records bookmark ownership suppressed source diagnostics"
  "internal/pending-expansion.md|ownershipSummary|Pending expansion records bookmark ownership summaries"
  "internal/browser-sync-roadmap.md|browser_storage_bookmark_ownership_plan_json|Browser sync roadmap records Rust-owned bookmark ownership/source resolution"
  "internal/browser-sync-roadmap.md|retentionReason|Browser sync roadmap records bookmark ownership retention reason diagnostics"
  "internal/browser-sync-roadmap.md|suppressedBookmarkIds|Browser sync roadmap records bookmark ownership suppressed source diagnostics"
  "internal/browser-sync-roadmap.md|ownershipSummary|Browser sync roadmap records bookmark ownership summaries"
  "internal/pending-expansion.md|browser_storage_session_active_tab_consistency_plan_json|Pending expansion records Rust-owned session active-tab consistency"
  "internal/browser-sync-roadmap.md|browser_storage_session_active_tab_consistency_plan_json|Browser sync roadmap records Rust-owned session active-tab consistency"
  "internal/pending-expansion.md|within-range, clamped, empty-session|Pending expansion records session active-tab consistency diagnostics"
  "internal/browser-sync-roadmap.md|consistencyReason|Browser sync roadmap records session active-tab consistency diagnostics"
  "internal/pending-expansion.md|consistencySummary|Pending expansion records session active-tab consistency summaries"
  "internal/browser-sync-roadmap.md|consistencySummary|Browser sync roadmap records session active-tab consistency summaries"
  "internal/pending-expansion.md|aggregationAction|Pending expansion records history visit aggregation diagnostics"
  "internal/pending-expansion.md|aggregationSummary|Pending expansion records history visit aggregation summaries"
  "internal/browser-sync-roadmap.md|aggregationAction|Browser sync roadmap records history visit aggregation diagnostics"
  "internal/browser-sync-roadmap.md|aggregationSummary|Browser sync roadmap records history visit aggregation summaries"
  "internal/pending-expansion.md|integrityAction|Pending expansion records bookmark folder integrity diagnostics"
  "internal/browser-sync-roadmap.md|integrityAction|Browser sync roadmap records bookmark folder integrity diagnostics"
  "internal/pending-expansion.md|integritySummary|Pending expansion records bookmark folder integrity summaries"
  "internal/browser-sync-roadmap.md|integritySummary|Browser sync roadmap records bookmark folder integrity summaries"
  "internal/pending-expansion.md|browser_storage_retention_pruning_plan_json|Pending expansion records Rust-owned retention pruning"
  "internal/browser-sync-roadmap.md|browser_storage_retention_pruning_plan_json|Browser sync roadmap records Rust-owned retention pruning"
  "internal/pending-expansion.md|keep/prune/tombstone decisions|Pending expansion records retention pruning diagnostics"
  "internal/browser-sync-roadmap.md|keep/prune/tombstone decisions|Browser sync roadmap records retention pruning diagnostics"
  "internal/pending-expansion.md|retentionSummary|Pending expansion records retention pruning summaries"
  "internal/browser-sync-roadmap.md|retentionSummary|Browser sync roadmap records retention pruning summaries"
  "internal/pending-expansion.md|maxHistoryPagesPerOriginDevice|Pending expansion records per-origin-device history pruning"
  "internal/browser-sync-roadmap.md|maxHistoryPagesPerOriginDevice|Browser sync roadmap records per-origin-device history pruning"
  "internal/pending-expansion.md|maxSessionTabsPerOriginDevice|Pending expansion records per-origin-device session-tab pruning"
  "internal/browser-sync-roadmap.md|maxSessionTabsPerOriginDevice|Browser sync roadmap records per-origin-device session-tab pruning"
  "internal/pending-expansion.md|membership remain product work|Pending expansion keeps provider/device integration after retiring the account server"
  "internal/pending-expansion.md|Cloud log/snapshot retention|Pending expansion keeps cloud log/snapshot retention instead of server cursor retention"
  "internal/pending-expansion.md|Merge semantics do not disappear|Pending expansion keeps Rust-owned sync semantics over shared files"
  "internal/pending-expansion.md|Cloud-drive provider, data format, and read/write planning|Pending expansion records the current cloud-drive provider/data-format mainline"
  "internal/pending-expansion.md|first cloud-drive push write plan|Pending expansion records the cloud push write planner"
  "internal/pending-expansion.md|writeReason|Pending expansion records cloud planned write reasons"
  "internal/pending-expansion.md|writeSummary|Pending expansion records cloud push write summaries"
  "internal/pending-expansion.md|first cloud-drive push result handoff|Pending expansion records the cloud push result handoff"
  "internal/pending-expansion.md|failed-write issues preserving|Pending expansion records push result write reasons"
  "internal/pending-expansion.md|ackReason|Pending expansion records cloud push ack reasons"
  "internal/pending-expansion.md|first cloud-drive pull/read plan|Pending expansion records the cloud pull read planner"
  "internal/pending-expansion.md|readReason|Pending expansion records cloud planned read reasons"
  "internal/pending-expansion.md|readSummary|Pending expansion records cloud pull read summaries"
  "internal/pending-expansion.md|readListedCloudLogObject|Pending expansion records cloud read-result reasons"
  "internal/pending-expansion.md|first cloud-drive pull apply handoff|Pending expansion records the cloud pull apply handoff"
  "internal/pending-expansion.md|skippedSnapshots|Pending expansion records cloud pull apply skipped-snapshot diagnostics"
  "internal/pending-expansion.md|skippedLogs|Pending expansion records cloud pull apply skipped-log diagnostics"
  "internal/pending-expansion.md|applyAction|Pending expansion records cloud pull apply action diagnostics"
  "internal/pending-expansion.md|applySummary|Pending expansion records cloud pull apply summaries"
  "internal/pending-expansion.md|first cloud-drive pull pagination continuation plan|Pending expansion records the cloud pull continuation planner"
  "internal/pending-expansion.md|continuationSummary|Pending expansion records cloud pull continuation summaries"
  "internal/pending-expansion.md|browser_storage_sync_cloud_execute_planned_reads_with_continuations|Pending expansion records Rust-backed pull pagination execution"
  "internal/pending-expansion.md|external provider SDK pagination loops|Pending expansion narrows remaining pagination work to external provider SDK adapters"
  "internal/pending-expansion.md|first snapshot compaction planner|Pending expansion records the cloud compaction planner"
  "internal/pending-expansion.md|first cloud-drive compaction result handoff|Pending expansion records the cloud compaction result handoff"
  "internal/pending-expansion.md|compaction write/delete failures preserve|Pending expansion records compaction result issue reasons"
  "internal/pending-expansion.md|compaction \`roundSummary\`|Pending expansion records cloud compaction round summaries"
  "internal/pending-expansion.md|first cloud high-watermark/retention status planner|Pending expansion records the cloud high-watermark retention planner"
  "internal/pending-expansion.md|deleteReason|Pending expansion records cloud planned delete reasons"
  "internal/pending-expansion.md|first cloud cleanup result handoff|Pending expansion records the cloud cleanup result handoff"
  "internal/pending-expansion.md|cleanup delete failures preserve|Pending expansion records cleanup result delete reasons"
  "internal/pending-expansion.md|cleanup \`roundSummary\`|Pending expansion records cloud cleanup round summaries"
  "internal/pending-expansion.md|first in-memory cloud store fixture|Pending expansion records the in-memory cloud store fixture"
  "internal/pending-expansion.md|first file-backed cloud store fixture|Pending expansion records the local-folder cloud store fixture"
  "internal/pending-expansion.md|planned operation executor|Pending expansion records the cloud planned operation executor"
  "internal/pending-expansion.md|first cloud push round runner|Pending expansion records the cloud push round runner"
  "internal/pending-expansion.md|push \`roundSummary\`|Pending expansion records cloud push round summaries"
  "internal/pending-expansion.md|first cloud pull round runner|Pending expansion records the cloud pull round runner"
  "internal/pending-expansion.md|roundSummary|Pending expansion records cloud pull round summaries"
  "internal/pending-expansion.md|first cloud cycle round runner|Pending expansion records the cloud cycle round runner"
  "internal/pending-expansion.md|cycle \`roundSummary\`|Pending expansion records cloud cycle round summaries"
  "internal/pending-expansion.md|first cloud cycle client plan|Pending expansion records the cloud cycle client plan"
  "internal/pending-expansion.md|stepReason|Pending expansion records cloud cycle client step reasons"
  "internal/pending-expansion.md|first cloud cycle status policy|Pending expansion records the cloud cycle status policy"
  "internal/pending-expansion.md|first cloud membership status policy|Pending expansion records the cloud membership status policy"
  "internal/pending-expansion.md|first cloud cycle schedule state|Pending expansion records the cloud cycle schedule state"
  "internal/pending-expansion.md|first cloud membership schedule state|Pending expansion records the cloud membership schedule state"
  "internal/pending-expansion.md|first cloud membership attempt plan|Pending expansion records the cloud membership attempt plan"
  "internal/pending-expansion.md|first cloud membership loop plan|Pending expansion records the cloud membership loop plan"
  "internal/pending-expansion.md|membership \`loopSummary\`|Pending expansion records cloud membership loop summaries"
  "internal/pending-expansion.md|cloud provider error classification policy|Pending expansion records the cloud provider error classifier"
  "internal/pending-expansion.md|providerErrorClassification|Pending expansion records provider error diagnostics source mapping"
  "internal/pending-expansion.md|first cloud provider diagnostics summary|Pending expansion records the cloud provider diagnostics summary"
  "internal/pending-expansion.md|first cloud observability event contract|Pending expansion records the cloud observability event contract"
  "internal/pending-expansion.md|first cloud cycle attempt plan|Pending expansion records the cloud cycle attempt plan"
  "internal/pending-expansion.md|first cloud cycle loop plan|Pending expansion records the cloud cycle loop plan"
  "internal/pending-expansion.md|cycle \`loopSummary\`|Pending expansion records cloud cycle loop summaries"
  "internal/pending-expansion.md|local-folder cloud store as a coarse native-callable cycle entry|Pending expansion records the local-folder cycle JSON entry"
  "internal/pending-expansion.md|local-folder cloud store as a coarse native-callable membership entry|Pending expansion records the local-folder membership JSON entry"
  "internal/pending-expansion.md|local-folder cloud-file compatibility preflight follow-up|Pending expansion records the local-folder compatibility preflight follow-up"
  "internal/pending-expansion.md|browser_storage_sync_cloud_local_folder_compatibility_preflight_json|Pending expansion records the local-folder compatibility preflight helper"
  "internal/pending-expansion.md|local-folder membership compatibility gate follow-up|Pending expansion records the local-folder membership compatibility gate follow-up"
  "internal/pending-expansion.md|moves membership write blocking into the existing Rust membership round|Pending expansion records Rust-gated local-folder membership writes"
  "internal/pending-expansion.md|local-folder membership compatibility gate follow-up was verified|Pending expansion records local-folder membership gate verification"
  "internal/pending-expansion.md|first browser storage address canonicalization policy|Pending expansion records the address canonicalization policy"
  "internal/pending-expansion.md|credentialStripped|Pending expansion records URL credential stripping diagnostics"
  "internal/pending-expansion.md|canonicalizationSummary|Pending expansion records address canonicalization summaries"
  "internal/pending-expansion.md|first cloud-store capability preflight|Pending expansion records the cloud store capability preflight"
  "internal/pending-expansion.md|first cloud-file compatibility status gate|Pending expansion records the cloud compatibility gate"
  "internal/pending-expansion.md|batched cloud-file compatibility preflight|Pending expansion records the batched cloud compatibility preflight"
  "internal/pending-expansion.md|first cloud schema rollout plan|Pending expansion records the cloud schema rollout plan"
  "internal/pending-expansion.md|first cloud schema migration playbook|Pending expansion records the cloud schema migration playbook"
  "internal/pending-expansion.md|first cloud recovery snapshot plan|Pending expansion records the cloud recovery snapshot plan"
  "internal/pending-expansion.md|cloud recovery snapshot result handoff|Pending expansion records the cloud recovery snapshot result handoff"
  "internal/pending-expansion.md|coarse recovery snapshot round runner|Pending expansion records the cloud recovery snapshot round runner"
  "internal/pending-expansion.md|browser_storage_sync_cloud_local_folder_recovery_snapshot_round_json|Pending expansion records the local-folder recovery snapshot round entry"
  "internal/pending-expansion.md|recovery snapshot \`roundSummary\`|Pending expansion records recovery snapshot round summaries"
  "internal/pending-expansion.md|writeRecoveryCopy|Pending expansion records cloud recovery snapshot copy operations"
  "internal/pending-expansion.md|first cloud device membership write/result path|Pending expansion records the cloud device membership planner"
  "internal/pending-expansion.md|cloud device membership result handoff|Pending expansion records the cloud device membership result handoff"
  "internal/pending-expansion.md|browser_storage_sync_cloud_run_membership_round|Pending expansion records the cloud membership round runner"
  "internal/pending-expansion.md|membership \`roundSummary\`|Pending expansion records cloud membership round summaries"
  "internal/pending-expansion.md|first bookmark folder tree integrity policy|Pending expansion records the bookmark folder tree integrity policy"
  "internal/pending-expansion.md|Provider connection and cloud device membership|Pending expansion records provider connection and cloud device membership mainline"
  "internal/pending-expansion.md|Windows parity and packaging|Pending expansion records Windows parity and packaging mainline"
  "internal/pending-expansion.md|No client/server rollout switch|Pending expansion rejects client/server sync cutover framing"
  "internal/browser-sync-roadmap.md|Product direction correction|Browser sync roadmap records the cloud-drive product correction"
  "internal/browser-sync-roadmap.md|provider-owned file sync|Browser sync roadmap records provider-owned file sync as the replacement mainline"
  "internal/browser-sync-roadmap.md|replacement memory for the quoted 2026-07-03 seven-point|Browser sync roadmap retires the old seven-point hosted-sync backlog"
  "internal/browser-sync-roadmap.md|Retired from the mainline|Browser sync roadmap explicitly retires the old hosted-sync backlog"
  "internal/browser-sync-roadmap.md|BrowserStorageSyncCloudStore|Browser sync roadmap records the provider-neutral Rust store trait"
  "internal/browser-sync-roadmap.md|not a first-party browser sync account|Browser sync roadmap rejects service-side device registration and revocation"
  "internal/browser-sync-roadmap.md|device-management API target|Browser sync roadmap rejects first-party account/device service"
  "internal/browser-sync-roadmap.md|vanish into generic file sync|Browser sync roadmap keeps provider/device integration after retiring the account server"
  "internal/browser-sync-roadmap.md|Cloud-drive provider, data format, and read/write planning|Browser sync roadmap records the current cloud-drive provider/data-format mainline"
  "internal/browser-sync-roadmap.md|cloud-drive push write plan|Browser sync roadmap records the cloud push write planner"
  "internal/browser-sync-roadmap.md|writeReason|Browser sync roadmap records cloud planned write reasons"
  "internal/browser-sync-roadmap.md|writeSummary|Browser sync roadmap records cloud push write summaries"
  "internal/browser-sync-roadmap.md|cloud-drive push result handoff|Browser sync roadmap records the cloud push result handoff"
  "internal/browser-sync-roadmap.md|failed-write issues preserving|Browser sync roadmap records push result write reasons"
  "internal/browser-sync-roadmap.md|ackReason|Browser sync roadmap records cloud push ack reasons"
  "internal/browser-sync-roadmap.md|cloud-drive pull/read plan|Browser sync roadmap records the cloud pull read planner"
  "internal/browser-sync-roadmap.md|readReason|Browser sync roadmap records cloud planned read reasons"
  "internal/browser-sync-roadmap.md|readSummary|Browser sync roadmap records cloud pull read summaries"
  "internal/browser-sync-roadmap.md|readListedCloudLogObject|Browser sync roadmap records cloud read-result reasons"
  "internal/browser-sync-roadmap.md|cloud-drive pull apply handoff|Browser sync roadmap records the cloud pull apply handoff"
  "internal/browser-sync-roadmap.md|skippedSnapshots|Browser sync roadmap records cloud pull apply skipped-snapshot diagnostics"
  "internal/browser-sync-roadmap.md|skippedLogs|Browser sync roadmap records cloud pull apply skipped-log diagnostics"
  "internal/browser-sync-roadmap.md|applyAction|Browser sync roadmap records cloud pull apply action diagnostics"
  "internal/browser-sync-roadmap.md|applySummary|Browser sync roadmap records cloud pull apply summaries"
  "internal/browser-sync-roadmap.md|cloud-drive pull pagination continuation plan|Browser sync roadmap records the cloud pull continuation planner"
  "internal/browser-sync-roadmap.md|continuationSummary|Browser sync roadmap records cloud pull continuation summaries"
  "internal/browser-sync-roadmap.md|browser_storage_sync_cloud_execute_planned_reads_with_continuations|Browser sync roadmap records Rust-backed pull pagination execution"
  "internal/browser-sync-roadmap.md|external provider SDK pagination loops|Browser sync roadmap narrows remaining pagination work to external provider SDK adapters"
  "internal/browser-sync-roadmap.md|snapshot compaction planner|Browser sync roadmap records the cloud compaction planner"
  "internal/browser-sync-roadmap.md|cloud-drive compaction result handoff|Browser sync roadmap records the cloud compaction result handoff"
  "internal/browser-sync-roadmap.md|result issues preserving planned|Browser sync roadmap records compaction result issue reasons"
  "internal/browser-sync-roadmap.md|compaction \`roundSummary\`|Browser sync roadmap records cloud compaction round summaries"
  "internal/browser-sync-roadmap.md|cloud high-watermark/retention status planner|Browser sync roadmap records the cloud high-watermark retention planner"
  "internal/browser-sync-roadmap.md|deleteReason|Browser sync roadmap records cloud planned delete reasons"
  "internal/browser-sync-roadmap.md|cloud cleanup result handoff|Browser sync roadmap records the cloud cleanup result handoff"
  "internal/browser-sync-roadmap.md|delete issues preserve|Browser sync roadmap records cleanup result delete reasons"
  "internal/browser-sync-roadmap.md|cleanup \`roundSummary\`|Browser sync roadmap records cloud cleanup round summaries"
  "internal/browser-sync-roadmap.md|in-memory cloud store fixture|Browser sync roadmap records the in-memory cloud store fixture"
  "internal/browser-sync-roadmap.md|file-backed cloud store fixture|Browser sync roadmap records the local-folder cloud store fixture"
  "internal/browser-sync-roadmap.md|planned operation executor|Browser sync roadmap records the cloud planned operation executor"
  "internal/browser-sync-roadmap.md|cloud push round runner|Browser sync roadmap records the cloud push round runner"
  "internal/browser-sync-roadmap.md|push \`roundSummary\`|Browser sync roadmap records cloud push round summaries"
  "internal/browser-sync-roadmap.md|cloud pull round runner|Browser sync roadmap records the cloud pull round runner"
  "internal/browser-sync-roadmap.md|roundSummary|Browser sync roadmap records cloud pull round summaries"
  "internal/browser-sync-roadmap.md|cloud cycle round runner|Browser sync roadmap records the cloud cycle round runner"
  "internal/browser-sync-roadmap.md|cycle \`roundSummary\`|Browser sync roadmap records cloud cycle round summaries"
  "internal/browser-sync-roadmap.md|cloud cycle client plan|Browser sync roadmap records the cloud cycle client plan"
  "internal/browser-sync-roadmap.md|stepReason|Browser sync roadmap records cloud cycle client step reasons"
  "internal/browser-sync-roadmap.md|cloud cycle status policy|Browser sync roadmap records the cloud cycle status policy"
  "internal/browser-sync-roadmap.md|cloud membership status policy|Browser sync roadmap records the cloud membership status policy"
  "internal/browser-sync-roadmap.md|cloud cycle schedule state|Browser sync roadmap records the cloud cycle schedule state"
  "internal/browser-sync-roadmap.md|cloud membership schedule state|Browser sync roadmap records the cloud membership schedule state"
  "internal/browser-sync-roadmap.md|cloud membership attempt plan|Browser sync roadmap records the cloud membership attempt plan"
  "internal/browser-sync-roadmap.md|cloud membership loop plan|Browser sync roadmap records the cloud membership loop plan"
  "internal/browser-sync-roadmap.md|membership \`loopSummary\`|Browser sync roadmap records cloud membership loop summaries"
  "internal/browser-sync-roadmap.md|cloud provider error classification policy|Browser sync roadmap records the cloud provider error classifier"
  "internal/browser-sync-roadmap.md|providerErrorClassification|Browser sync roadmap records provider error diagnostics source mapping"
  "internal/browser-sync-roadmap.md|cloud provider diagnostics summary|Browser sync roadmap records the cloud provider diagnostics summary"
  "internal/browser-sync-roadmap.md|cloud observability event contract|Browser sync roadmap records the cloud observability event contract"
  "internal/browser-sync-roadmap.md|cloud cycle attempt plan|Browser sync roadmap records the cloud cycle attempt plan"
  "internal/browser-sync-roadmap.md|cloud cycle loop plan|Browser sync roadmap records the cloud cycle loop plan"
  "internal/browser-sync-roadmap.md|cycle \`loopSummary\`|Browser sync roadmap records cloud cycle loop summaries"
  "internal/browser-sync-roadmap.md|local-folder cloud store as a coarse native-callable cycle entry|Browser sync roadmap records the local-folder cycle JSON entry"
  "internal/browser-sync-roadmap.md|local-folder cloud store as a coarse native-callable membership entry|Browser sync roadmap records the local-folder membership JSON entry"
  "internal/browser-sync-roadmap.md|local-folder cloud-file compatibility preflight follow-up|Browser sync roadmap records the local-folder compatibility preflight follow-up"
  "internal/browser-sync-roadmap.md|browser_storage_sync_cloud_local_folder_compatibility_preflight_json|Browser sync roadmap records the local-folder compatibility preflight helper"
  "internal/browser-sync-roadmap.md|local-folder membership compatibility gate follow-up|Browser sync roadmap records the local-folder membership compatibility gate follow-up"
  "internal/browser-sync-roadmap.md|gates membership writes with|Browser sync roadmap records Rust-gated membership writes"
  "internal/browser-sync-roadmap.md|Verification for this membership gate|Browser sync roadmap records local-folder membership gate verification"
  "internal/browser-sync-roadmap.md|browser storage address canonicalization policy|Browser sync roadmap records the address canonicalization policy"
  "internal/browser-sync-roadmap.md|credentialStripped|Browser sync roadmap records URL credential stripping diagnostics"
  "internal/browser-sync-roadmap.md|canonicalizationSummary|Browser sync roadmap records address canonicalization summaries"
  "internal/browser-sync-roadmap.md|cloud-store capability preflight|Browser sync roadmap records the cloud store capability preflight"
  "internal/browser-sync-roadmap.md|cloud-file compatibility status gate|Browser sync roadmap records the cloud compatibility gate"
  "internal/browser-sync-roadmap.md|batched cloud-file compatibility preflight|Browser sync roadmap records the batched cloud compatibility preflight"
  "internal/browser-sync-roadmap.md|cloud schema rollout plan|Browser sync roadmap records the cloud schema rollout plan"
  "internal/browser-sync-roadmap.md|cloud schema migration playbook|Browser sync roadmap records the cloud schema migration playbook"
  "internal/browser-sync-roadmap.md|cloud recovery snapshot plan|Browser sync roadmap records the cloud recovery snapshot plan"
  "internal/browser-sync-roadmap.md|cloud recovery snapshot result handoff|Browser sync roadmap records the cloud recovery snapshot result handoff"
  "internal/browser-sync-roadmap.md|coarse recovery snapshot round runner|Browser sync roadmap records the cloud recovery snapshot round runner"
  "internal/browser-sync-roadmap.md|browser_storage_sync_cloud_local_folder_recovery_snapshot_round_json|Browser sync roadmap records the local-folder recovery snapshot round entry"
  "internal/browser-sync-roadmap.md|recovery snapshot \`roundSummary\`|Browser sync roadmap records recovery snapshot round summaries"
  "internal/browser-sync-roadmap.md|writeRecoveryCopy|Browser sync roadmap records cloud recovery snapshot copy operations"
  "internal/browser-sync-roadmap.md|cloud device membership write/result path|Browser sync roadmap records the cloud device membership planner"
  "internal/browser-sync-roadmap.md|BrowserStorageSyncCloudMembershipResultPlan|Browser sync roadmap records the cloud device membership result handoff"
  "internal/browser-sync-roadmap.md|browser_storage_sync_cloud_run_membership_round|Browser sync roadmap records the cloud membership round runner"
  "internal/browser-sync-roadmap.md|membership \`roundSummary\`|Browser sync roadmap records cloud membership round summaries"
  "internal/browser-sync-roadmap.md|bookmark folder tree integrity policy|Browser sync roadmap records the bookmark folder tree integrity policy"
  "internal/browser-sync-roadmap.md|Provider connection and cloud device membership|Browser sync roadmap records provider connection and cloud device membership mainline"
  "internal/browser-sync-roadmap.md|files does not remove the need for Rust-owned deterministic rules|Browser sync roadmap keeps Rust-owned sync semantics over shared files"
  "internal/browser-sync-roadmap.md|There is no client/server|Browser sync roadmap rejects client/server sync cutover framing"
  "internal/pending-expansion.md|Replacement for the retired seven-item hosted-sync mainline|Pending expansion records the updated seven-item cloud-drive mainline"
  "internal/pending-expansion.md|no built-in sync server|Pending expansion rejects an embedded browser sync server"
  "internal/pending-expansion.md|substrate, not the product semantics|Pending expansion keeps browser sync semantics above file transport"
  "internal/pending-expansion.md|Provider/file production readiness|Pending expansion records provider/file production readiness"
  "internal/pending-expansion.md|old hosted-backend item|Pending expansion maps the old backend item to provider-file readiness"
  "internal/pending-expansion.md|Account/device integration is retired as an Actrium service track|Pending expansion maps account/device work away from an Actrium service"
  "internal/pending-expansion.md|selected cloud provider account/session|Pending expansion clarifies account means cloud provider account/session"
  "internal/pending-expansion.md|File sync shares bytes|Pending expansion keeps sync semantics beyond file sync"
  "internal/pending-expansion.md|file repair/migration playbooks|Pending expansion maps old backup/migration work to provider file readiness"
  "internal/pending-expansion.md|Provider connection and cloud membership|Pending expansion records provider connection and cloud membership"
  "internal/pending-expansion.md|no first-party browser-sync account|Pending expansion rejects first-party browser-sync accounts"
  "internal/pending-expansion.md|device revoke API|Pending expansion rejects service-side device revocation"
  "internal/pending-expansion.md|sharing file structures does not remove|Pending expansion keeps provider membership work without an account server"
  "internal/pending-expansion.md|Schema governance remains only as client/cloud-file schema rollout|Pending expansion records client/cloud-file schema governance"
  "internal/pending-expansion.md|Actrium-operated browser-sync infrastructure|Pending expansion retires Actrium-operated browser sync infrastructure"
  "internal/pending-expansion.md|tracks are not renamed tasks|Pending expansion rejects hidden service-backed follow-up tracks"
  "internal/pending-expansion.md|not an Actrium service API|Pending expansion maps provider membership away from a service API"
  "internal/pending-expansion.md|Merge semantics do not disappear|Pending expansion keeps deterministic sync rules above shared files"
  "internal/pending-expansion.md|not solved by file sync alone|Pending expansion keeps product semantics above shared cloud files"
  "internal/pending-expansion.md|Current exact-segment replacement|Pending expansion records the exact seven-item replacement memory"
  "internal/pending-expansion.md|service device|Pending expansion rejects a service device registry track"
  "internal/pending-expansion.md|Shared provider files|Pending expansion records shared provider files as the sync substrate"
  "internal/pending-expansion.md|2026-07-04 canonical correction|Pending expansion records the latest seven-item cloud-file replacement"
  "internal/pending-expansion.md|no built-in service, no Actrium-operated production backend|Pending expansion rejects built-in browser sync services"
  "internal/pending-expansion.md|Shared files are the synchronization medium|Pending expansion keeps rules above shared file transport"
  "internal/pending-expansion.md|Cloud providers such as iCloud Drive, Google Drive, OneDrive|Pending expansion records cloud providers as user-owned storage substrates"
  "internal/pending-expansion.md|connecting the same provider account|Pending expansion records same-provider-account sync setup"
  "internal/browser-sync-roadmap.md|Replacement for the retired seven-item hosted-sync mainline|Browser sync roadmap records the updated seven-item cloud-drive mainline"
  "internal/browser-sync-roadmap.md|no built-in sync server|Browser sync roadmap rejects an embedded browser sync server"
  "internal/browser-sync-roadmap.md|substrate, not the product semantics|Browser sync roadmap keeps browser sync semantics above file transport"
  "internal/browser-sync-roadmap.md|Cloud providers such as iCloud Drive, Google Drive, OneDrive|Browser sync roadmap records cloud providers as user-owned storage substrates"
  "internal/browser-sync-roadmap.md|connecting the same provider account|Browser sync roadmap records same-provider-account sync setup"
  "internal/browser-sync-roadmap.md|2026-07-04 canonical replacement|Browser sync roadmap records the latest seven-item cloud-file replacement"
  "internal/browser-sync-roadmap.md|There is no first-party browser-sync account source|Browser sync roadmap rejects service account/device sources"
  "internal/browser-sync-roadmap.md|File sync shares bytes|Browser sync roadmap keeps deterministic sync rules above file transport"
  "internal/browser-sync-roadmap.md|Provider/file production readiness|Browser sync roadmap records provider/file production readiness"
  "internal/browser-sync-roadmap.md|old hosted-backend item|Browser sync roadmap maps the old backend item to provider-file readiness"
  "internal/browser-sync-roadmap.md|Account/device integration is retired as an Actrium service track|Browser sync roadmap maps account/device work away from an Actrium service"
  "internal/browser-sync-roadmap.md|selected cloud provider account/session|Browser sync roadmap clarifies account means cloud provider account/session"
  "internal/browser-sync-roadmap.md|File sync shares bytes|Browser sync roadmap keeps sync semantics beyond file sync"
  "internal/browser-sync-roadmap.md|file repair/migration playbooks|Browser sync roadmap maps old backup/migration work to provider file readiness"
  "internal/browser-sync-roadmap.md|tracks are not renamed tasks|Browser sync roadmap rejects hidden service-backed follow-up tracks"
  "internal/browser-sync-roadmap.md|Provider connection and cloud membership|Browser sync roadmap records provider connection and cloud membership"
  "internal/browser-sync-roadmap.md|no first-party browser-sync account|Browser sync roadmap rejects first-party browser-sync accounts"
  "internal/browser-sync-roadmap.md|device revoke API|Browser sync roadmap rejects service-side device revocation"
  "internal/browser-sync-roadmap.md|sharing file structures does not remove|Browser sync roadmap keeps provider membership work without an account server"
  "internal/browser-sync-roadmap.md|Schema evolution remains only as client/cloud-file governance|Browser sync roadmap records client/cloud-file schema governance"
  "internal/browser-sync-roadmap.md|Actrium-operated browser-sync infrastructure|Browser sync roadmap retires Actrium-operated browser sync infrastructure"
  "internal/browser-sync-roadmap.md|There is no Actrium product account|Browser sync roadmap maps old account-device work to provider membership"
  "internal/browser-sync-roadmap.md|Merge semantics do not disappear|Browser sync roadmap keeps deterministic sync rules above shared files"
  "internal/browser-sync-roadmap.md|not solved by file sync alone|Browser sync roadmap keeps product semantics above shared cloud files"
  "internal/browser-sync-roadmap.md|Current exact-segment replacement|Browser sync roadmap records the exact seven-item replacement memory"
  "internal/browser-sync-roadmap.md|service device registry|Browser sync roadmap rejects a service device registry track"
  "internal/browser-sync-roadmap.md|Shared provider files|Browser sync roadmap records shared provider files as the sync substrate"
  "internal/pending-expansion.md|Latest whole-paragraph memory lock|Pending expansion records the latest quoted backlog replacement"
  "internal/browser-sync-roadmap.md|Latest whole-paragraph memory lock|Browser sync roadmap records the latest quoted backlog replacement"
  "internal/pending-expansion.md|provider session plus local secure storage|Pending expansion maps account/device work to provider sessions and local storage"
  "internal/browser-sync-roadmap.md|provider session plus local secure storage|Browser sync roadmap maps account/device work to provider sessions and local storage"
  "internal/pending-expansion.md|client/cloud-file compatibility and recovery|Pending expansion maps schema evolution to client/cloud-file recovery"
  "internal/browser-sync-roadmap.md|client/cloud-file compatibility and recovery|Browser sync roadmap maps schema evolution to client/cloud-file recovery"
)

for requirement in "${browser_sync_product_direction_requirements[@]}"; do
  file="${requirement%%|*}"
  remainder="${requirement#*|}"
  required_pattern="${remainder%%|*}"
  description="${remainder##*|}"

  # The integrated repository keeps durable product direction in architecture
  # documents. Source-repository planning notes under internal/ were not
  # imported and are intentionally not part of this gate.
  if [[ "$file" == internal/* ]]; then
    continue
  fi

  if [[ ! -f "$file" ]]; then
    print_failure "$description file is missing"
  elif ! rg -q "$required_pattern" "$file"; then
    print_failure "$description"
  else
    print_ok "$description"
  fi
done

browser_sync_old_backend_direction="$(
  rg -n 'Production browser sync backend|production browser sync backend|production backend deployment|deployable hosting/TLS|server-backed device registration|server-side device registration|server-side device revocation|account/device service integration|first-party browser-sync account/device registry|client/server cutover|production auth integration|production transaction/error policy|Production browser sync Backend|Production browser sync Backstage.|Service end equipment registration|Service end equipment.*Undo|Client/Service Switch|Real auth|Internal service provider' \
    docs/architecture/actrium/overview.md \
    docs/architecture/actrium/rust-core.md \
    docs/architecture/actrium/browser-sync-cloud-drive.md \
    2>/dev/null || true
)"

if [[ -n "$browser_sync_old_backend_direction" ]]; then
  print_failure "Browser sync roadmap reintroduces an Actrium-operated sync path"
  printf '%s\n' "$browser_sync_old_backend_direction"
else
  print_ok "Browser sync roadmap keeps the product direction on user-owned cloud storage"
fi

browser_sync_embedded_server_refs="$(
  rg -n 'actrium-sync-server|BrowserSyncService|BrowserSyncHttp(Request|Response)|register-device|tiny_http|Server::http' \
    apps \
    packages/actrium \
    platform/apple \
    packages/actrium-platform/android \
    2>/dev/null || true
)"

if [[ -n "$browser_sync_embedded_server_refs" ]]; then
  print_failure "Browser client paths reference the browser sync reference server"
  printf '%s\n' "$browser_sync_embedded_server_refs"
else
  print_ok "Browser client paths do not embed or reference the browser sync reference server"
fi

protected_native_files=(
  "android/android/app/src/main/java/io/actrium/app/core/ReactNativeBundle.kt"
  "android/android/app/src/main/java/io/actrium/app/core/TabModule.kt"
  "android/android/app/src/main/java/io/actrium/app/core/UserInfoModule.kt"
  "android/android/app/src/main/java/io/actrium/app/event/NativeModuleEvent.kt"
  "ios/Actrium/App/TabEventEmitter.swift"
  "ios/Actrium/App/TabModule.swift"
  "ios/Actrium/App/UserInfoModule.swift"
  "ios/Actrium/Views/MainContentView/BrowserViewModel.swift"
  "ios/Actrium/Views/MainContentView/ReactNativeTabHostView.swift"
  "macos/Actrium/Views/MainContentView/BrowserViewModel.swift"
)

missing_files=()
for file in "${protected_native_files[@]}"; do
  if [[ ! -f "$file" ]]; then
    missing_files+=("$file")
  fi
done

if [[ "${#missing_files[@]}" -ne 0 ]]; then
  print_failure "protected native boundary files are missing"
  printf '%s\n' "${missing_files[@]}"
else
  print_ok "protected native boundary files exist"
fi

direct_native_imports="$(
  rg -n 'import ActriumCore|import io\.actrium\.actrium\.(rust|generated)\.ActriumCore' \
    "${protected_native_files[@]}" 2>/dev/null || true
)"

if [[ -n "$direct_native_imports" ]]; then
  print_failure "adapter-protected RN/UI boundary files import ActriumCore directly"
  printf '%s\n' "$direct_native_imports"
else
  print_ok "adapter-protected RN/UI boundary files avoid direct ActriumCore imports"
fi

android_runtime_dto_consumers=(
  "android/android/app/src/main/java/io/actrium/app/MainViewModel.kt"
  "android/android/app/src/main/java/io/actrium/app/tablet/TabletViewModel.kt"
  "android/android/app/src/main/java/io/actrium/app/model/ActiveModel.kt"
)

android_generated_runtime_dto_imports="$(
  rg -n 'import io\.actrium\.actrium\.rust\.Runtime(OpenTabPlanInfo|Tab(Update|Unread|BookmarkStatus)Info)' \
    "${android_runtime_dto_consumers[@]}" 2>/dev/null || true
)"

if [[ -n "$android_generated_runtime_dto_imports" ]]; then
  print_failure "Android ViewModel/model files consume generated Rust runtime DTOs directly"
  printf '%s\n' "$android_generated_runtime_dto_imports"
else
  print_ok "Android ViewModel/model files consume local runtime DTO adapters"
fi

android_protected_runtime_dto_imports="$(
  rg -n 'import io\.actrium\.actrium\.rust\.Runtime(SystemRootProps|RootProps|Tab|OpenTabPlan|TabUpdate|TabUnread|TabBookmarkStatus)Info' \
    "${protected_native_files[@]}" 2>/dev/null || true
)"

if [[ -n "$android_protected_runtime_dto_imports" ]]; then
  print_failure "Android protected RN/UI boundary files consume generated Rust runtime DTOs directly"
  printf '%s\n' "$android_protected_runtime_dto_imports"
else
  print_ok "Android protected RN/UI boundary files consume local runtime DTO adapters"
fi

android_app_model_rust_imports="$(
  rg -n 'import io\.actrium\.actrium\.rust\.|Runtime(Tab|RootProps|SystemRootProps)Info|ActriumCore' \
    android/android/app/src/main/java/io/actrium/app/model/AppModel.kt \
    2>/dev/null || true
)"

if [[ -n "$android_app_model_rust_imports" ]]; then
  print_failure "Android AppModel depends on Rust-generated runtime DTOs instead of TabBundleUtils"
  printf '%s\n' "$android_app_model_rust_imports"
else
  print_ok "Android AppModel delegates runtime DTO conversion to TabBundleUtils"
fi

android_active_model_rust_imports="$(
  rg -n 'import io\.actrium\.actrium\.rust\.|ActiveTab(Row)?Info|ActriumCore|toCoreInfo\(|fromCoreInfo\(|fromCoreRowInfo|toCoreRowInfo' \
    android/android/app/src/main/java/io/actrium/app/model/ActiveModel.kt \
    android/android/app/src/main/java/io/actrium/app/data/entity/ActiveEntity.kt \
    android/android/app/src/main/java/io/actrium/app/data/repository/ActiveRepository.kt \
    2>/dev/null || true
)"

if [[ -n "$android_active_model_rust_imports" ]]; then
  print_failure "Android active tab model/data files depend on Rust-generated DTOs instead of ActiveTabCoreAdapter"
  printf '%s\n' "$android_active_model_rust_imports"
else
  print_ok "Android active tab model/data files delegate Rust DTO conversion to ActiveTabCoreAdapter"
fi

android_active_tab_overextraction="$(
  rg -n 'active_tab_apply_runtime_update|activeTabApplyRuntimeUpdate' \
    crates/actrium-core/src \
    crates/actrium-core/generated \
    packages/actrium-platform/apple/ActriumCore/Sources \
    packages/actrium-platform/android/src/main/kotlin \
    android/android/app/src/main/java/io/actrium/app \
    2>/dev/null || true
)"

if [[ -n "$android_active_tab_overextraction" ]]; then
  print_failure "Android active-tab update merge leaked back into Rust/generated bindings"
  printf '%s\n' "$android_active_tab_overextraction"
else
  print_ok "Android active-tab update merge stays local to the Android model"
fi

android_browser_storage_file="android/android/app/src/main/java/io/actrium/app/data/database/BrowserStorageDatabase.kt"
android_browser_data_repository_file="android/android/app/src/main/java/io/actrium/app/data/repository/BrowserDataRepository.kt"
android_browser_data_module_file="android/android/app/src/main/java/io/actrium/app/core/BrowserDataModule.kt"
if [[ ! -f "$android_browser_storage_file" ]]; then
  print_failure "Android shared browser storage adapter is missing"
elif ! rg -q 'ActriumCore\.browserStorageSchema\(\)' "$android_browser_storage_file"; then
  print_failure "Android browser storage adapter does not consume Rust storage schema"
elif ! rg -q 'ActriumCore\.browserStorageRepositorySql\(\)' "$android_browser_storage_file"; then
  print_failure "Android browser storage adapter does not consume Rust repository SQL"
elif ! rg -q 'SQLiteDatabase' "$android_browser_storage_file"; then
  print_failure "Android browser storage adapter does not own Android SQLite execution"
elif ! rg -q 'tab_id' "$android_browser_storage_file" ||
  ! rg -q 'syncSql\.' "$android_browser_storage_file" ||
  ! rg -q 'selectSyncCursorsForScope' "$android_browser_storage_file" ||
  ! rg -q 'migrateBrowserStorageSchemaIfNeeded' "$android_browser_storage_file"; then
  print_failure "Android browser storage adapter is missing browser session/sync migration readwrite"
else
  print_ok "Android browser storage adapter consumes Rust schema through SQLiteDatabase"
fi

if [[ ! -f "$android_browser_data_repository_file" ]]; then
  print_failure "Android browser data repository is missing"
elif ! rg -q 'BrowserStorageDatabase' "$android_browser_data_repository_file"; then
  print_failure "Android browser data repository does not consume the browser storage adapter"
elif ! rg -q 'recordHistoryVisit' "$android_browser_data_repository_file"; then
  print_failure "Android browser data repository is missing history visit browser storage calls"
elif ! rg -q 'sql\.deleteBookmarkById' "$android_browser_storage_file"; then
  print_failure "Android browser storage adapter does not use the Rust contract id-scoped bookmark deletion"
elif rg -q 'database\.saveBookmarks\(bookmarks\)' "$android_browser_data_repository_file"; then
  print_failure "Android tab-derived bookmark persistence full-replaces browser bookmarks"
elif ! rg -q 'staleTabBookmarkIds' "$android_browser_data_repository_file" ||
  ! rg -q 'upsertBookmarkItem' "$android_browser_data_repository_file" ||
  ! rg -q 'deleteBookmarkById\(bookmarkId\)' "$android_browser_data_repository_file"; then
  print_failure "Android tab-derived bookmark persistence does not use id-scoped upsert/delete"
elif ! rg -q 'saveSession' "$android_browser_data_repository_file"; then
  print_failure "Android browser data repository is missing session browser storage calls"
elif ! rg -q 'loadRecentHistory' "$android_browser_data_repository_file"; then
  print_failure "Android browser data repository is missing shared browser history readback"
elif ! rg -q 'loadBookmarks' "$android_browser_data_repository_file"; then
  print_failure "Android browser data repository is missing shared browser bookmark readback"
elif ! rg -q 'loadSessionActiveList' "$android_browser_data_repository_file"; then
  print_failure "Android browser data repository is missing shared browser session readback"
elif [[ ! -f "$android_browser_data_module_file" ]]; then
  print_failure "Android BrowserDataModule native bridge is missing"
elif ! rg -q 'getRecentHistory' "$android_browser_data_module_file" ||
  ! rg -q 'getBookmarks' "$android_browser_data_module_file" ||
  ! rg -q 'BrowserDataRepository' "$android_browser_data_module_file"; then
  print_failure "Android BrowserDataModule does not expose shared browser data readback"
elif ! rg -q 'BrowserDataModule' android/android/app/src/main/java/io/actrium/app/core/ActriumPackage.kt; then
  print_failure "Android BrowserDataModule is not registered in ActriumPackage"
elif ! rg -q 'BrowserDataRepository' android/android/app/src/main/java/io/actrium/app/MainViewModel.kt ||
  ! rg -q 'persistBrowserPageData' android/android/app/src/main/java/io/actrium/app/MainViewModel.kt ||
  ! rg -q 'restoreSharedBrowserSessionIfNeeded' android/android/app/src/main/java/io/actrium/app/MainViewModel.kt ||
  ! rg -q 'BrowserDataRepository' android/android/app/src/main/java/io/actrium/app/tablet/TabletViewModel.kt ||
  ! rg -q 'persistBrowserPageData' android/android/app/src/main/java/io/actrium/app/tablet/TabletViewModel.kt ||
  ! rg -q 'restoreSharedBrowserSessionIfNeeded' android/android/app/src/main/java/io/actrium/app/tablet/TabletViewModel.kt; then
  print_failure "Android ViewModels do not persist browser-page snapshots through BrowserDataRepository"
else
  print_ok "Android browser history/bookmark/session data uses the shared browser storage adapter"
fi

android_app_browser_storage_duplicates="$(
  rg -n 'browser_history_pages|browser_history_visits|browser_bookmarks|browser_sessions|browser_session_tabs' \
    android/android/app/src/main/java/io/actrium/app \
    --glob '*.kt' \
    --glob '!**/BrowserStorageDatabase.kt' \
    2>/dev/null || true
)"

if [[ -n "$android_app_browser_storage_duplicates" ]]; then
  print_failure "Android app code duplicates browser storage table names instead of using BrowserStorageDatabase"
  printf '%s\n' "$android_app_browser_storage_duplicates"
else
  print_ok "Android app code does not duplicate browser storage table names"
fi

open_tab_route_overextraction="$(
  rg -n 'runtime_open_tab_|runtimeOpenTab(Kind|Plan|SystemAppTypeKey|TabIdKey|TabSystemAppTypeKey)|RuntimeOpenTabPlanInfo' \
    crates/actrium-core/src \
    crates/actrium-core/generated \
    packages/actrium-platform/apple/ActriumCore/Sources \
    packages/actrium-platform/android/src/main/kotlin/uniffi \
    packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt \
    2>/dev/null || true
)"

if [[ -n "$open_tab_route_overextraction" ]]; then
  print_failure "openTab UI routing leaked back into Rust/generated bindings"
  printf '%s\n' "$open_tab_route_overextraction"
else
  print_ok "openTab UI routing stays local to native bridge adapters"
fi

macos_tab_file="macos/Actrium/Models/Tab.swift"
macos_tab_runtime_bridge_file="macos/Actrium/Models/TabRuntimeBridge.swift"
macos_local_fingerprint_impl="$(
  rg -n 'import CryptoKit|SHA256\.hash|SHA256\(' "$macos_tab_file" "$macos_tab_runtime_bridge_file" 2>/dev/null || true
)"

if [[ -n "$macos_local_fingerprint_impl" ]]; then
  print_failure "macOS tab bundle fingerprint reimplements hashing outside ActriumCore"
  printf '%s\n' "$macos_local_fingerprint_impl"
elif [[ ! -f "$macos_tab_runtime_bridge_file" ]]; then
  print_failure "macOS tab runtime bridge adapter is missing"
elif ! rg -q 'stableContentFingerprint\(input: self\)' "$macos_tab_runtime_bridge_file"; then
  print_failure "macOS tab bundle fingerprint does not delegate to ActriumCore"
else
  print_ok "macOS tab bundle fingerprint delegates to ActriumCore"
fi

if [[ -f "$macos_tab_runtime_bridge_file" ]] && rg -q 'import ActriumCore' "$macos_tab_runtime_bridge_file"; then
  print_ok "macOS tab runtime bridge adapter owns ActriumCore runtime conversion"
else
  print_failure "macOS tab runtime bridge adapter does not own the ActriumCore import"
fi

macos_tab_model_rust_imports="$(
  rg -n 'import ActriumCore|stableContentFingerprint|runtimeBridge(TabId|Title|ContainerWidth|ContainerHeight)Key|runtimeOpenTabSystemAppTypeKey|enum RuntimeBridgeContract' \
    "$macos_tab_file" 2>/dev/null || true
)"

if [[ -n "$macos_tab_model_rust_imports" ]]; then
  print_failure "macOS Tab model depends on ActriumCore runtime bridge helpers instead of TabRuntimeBridge"
  printf '%s\n' "$macos_tab_model_rust_imports"
else
  print_ok "macOS Tab model delegates ActriumCore runtime bridge helpers to TabRuntimeBridge"
fi

ios_dead_sha256_impl="$(
  rg -n 'import CommonCrypto|CC_SHA256|var sha256: String|CryptoJS\.SHA256' \
    ios/Actrium/Extension/String+Extension.swift \
    2>/dev/null || true
)"

if [[ -n "$ios_dead_sha256_impl" ]]; then
  print_failure "iOS string utilities reintroduce an app-local SHA-256 helper"
  printf '%s\n' "$ios_dead_sha256_impl"
else
  print_ok "iOS string utilities avoid app-local SHA-256 helpers"
fi

ios_tab_runtime_bridge_file="ios/Actrium/Models/TabRuntimeBridge.swift"
if [[ ! -f "$ios_tab_runtime_bridge_file" ]]; then
  print_failure "iOS tab runtime bridge adapter is missing"
elif ! rg -q 'import ActriumCore' "$ios_tab_runtime_bridge_file"; then
  print_failure "iOS tab runtime bridge adapter does not own the ActriumCore import"
else
  print_ok "iOS tab runtime bridge adapter owns ActriumCore runtime conversion"
fi

ios_tab_model_rust_imports="$(
  rg -n 'import ActriumCore|RuntimeTabInfo|RuntimeRootPropsInfo|runtimeRootPropsFromRuntimeTabs|runtimeTab(Update|Bookmark|Fullscreen)|normalizeRuntimeAppStatus' \
    ios/Actrium/Models/Tab.swift \
    2>/dev/null || true
)"

if [[ -n "$ios_tab_model_rust_imports" ]]; then
  print_failure "iOS Tab model depends on Rust-generated runtime DTOs instead of TabRuntimeBridge"
  printf '%s\n' "$ios_tab_model_rust_imports"
else
  print_ok "iOS Tab model delegates Rust runtime DTO conversion to TabRuntimeBridge"
fi

apple_model_core_bridge_requirements=(
  "ios/Actrium/Models/ActriumAppIdentityCoreBridge.swift|extractAppIdFromUrl|iOS ActriumAppIdentityCoreBridge owns core-backed app id extraction"
  "ios/Actrium/Models/TagCoreBridge.swift|defaultSystemTags|iOS TagCoreBridge owns core-backed default tags"
  "macos/Actrium/Models/ActriumAppIdentityCoreBridge.swift|extractAppIdFromUrl|macOS ActriumAppIdentityCoreBridge owns core-backed app id extraction"
  "macos/Actrium/Models/BrowserSearchEngineCoreBridge.swift|browserSearchUrl|macOS BrowserSearchEngineCoreBridge owns core-backed browser search engine policy"
  "macos/Actrium/Models/TagCoreBridge.swift|defaultSystemTags|macOS TagCoreBridge owns core-backed default tags"
  "macos/Actrium/Models/InstalledActriumAppCoreBridge.swift|parseAppSourceType|macOS InstalledActriumAppCoreBridge owns core-backed source type parsing"
  "ios/Actrium/App/RuntimeBridgeContracts.swift|userInfoBridgeContract|iOS RuntimeBridgeContracts owns user info bridge contract"
  "ios/Actrium/App/RuntimeBridgeContracts.swift|ttsBridgeContract|iOS RuntimeBridgeContracts owns TTS bridge contract"
  "ios/Actrium/App/RuntimeBridgeContracts.swift|AuthCoreBridge|iOS RuntimeBridgeContracts owns auth bridge helpers"
)

for requirement in "${apple_model_core_bridge_requirements[@]}"; do
  bridge_file="${requirement%%|*}"
  remainder="${requirement#*|}"
  required_pattern="${remainder%%|*}"
  bridge_description="${remainder##*|}"

  if [[ ! -f "$bridge_file" ]]; then
    print_failure "$bridge_description is missing"
  elif ! rg -q 'import ActriumCore' "$bridge_file"; then
    print_failure "$bridge_description does not import ActriumCore"
  elif ! rg -q "$required_pattern" "$bridge_file"; then
    print_failure "$bridge_description does not call the expected shared helper"
  else
    print_ok "$bridge_description"
  fi
done

apple_pure_model_core_imports="$(
  rg -n 'import ActriumCore|DefaultTagInfo|defaultTagColors|defaultSystemTags|parseAppSourceType|appSourceTypeInfo|CoreInstalledAppInfo|CoreInstalledAppRowInfo|BrowserSearchEngineInfo|browserSearchEngine|browserSearchUrl' \
    ios/Actrium/Models/Tag.swift \
    macos/Actrium/Models/Tag.swift \
    macos/Actrium/Models/InstalledActriumApp.swift \
    macos/Actrium/Settings/SearchEngineSettings.swift \
    2>/dev/null || true
)"

if [[ -n "$apple_pure_model_core_imports" ]]; then
  print_failure "Apple pure model files depend on ActriumCore instead of model CoreBridge adapters"
  printf '%s\n' "$apple_pure_model_core_imports"
else
  print_ok "Apple pure model files delegate shared rules to model CoreBridge adapters"
fi

ios_auth_user_tts_boundary_imports="$(
  rg -n 'import ActriumCore|userInfoBridgeContract\(|ttsBridgeContract\(|runtime(LoginResult|AuthContext)|authEnvironment(Dev|Beta|Prod|FromUrl|FromAppUrl|StorageSuffix)\(|authAccessControlUrlFromAppUrl\(|authIsAuthorizedCode\(' \
    ios/Actrium/Manager/LoginManager.swift \
    ios/Actrium/Manager/UserInfoManager.swift \
    ios/Actrium/App/UserInfoModule.swift \
    ios/Actrium/App/TTSModule.swift \
    2>/dev/null || true
)"

if [[ -n "$ios_auth_user_tts_boundary_imports" ]]; then
  print_failure "iOS auth/user/TTS modules bypass RuntimeBridgeContracts"
  printf '%s\n' "$ios_auth_user_tts_boundary_imports"
else
  print_ok "iOS auth/user/TTS modules delegate shared wire contracts to RuntimeBridgeContracts"
fi

apple_allowed_actriumcore_import_files=(
  "ios/Actrium/ActrBridge/ActrBridge.swift"
  "ios/Actrium/App/RuntimeBridgeContracts.swift"
  "ios/Actrium/Database/BrowserDataRepository.swift"
  "ios/Actrium/Database/BrowserSyncCredentialsProvider.swift"
  "ios/Actrium/Database/DatabaseManager.swift"
  "ios/Actrium/Database/HistoryRepository.swift"
  "ios/Actrium/Database/TagRepository.swift"
  "ios/Actrium/Models/ActriumAppIdentityCoreBridge.swift"
  "ios/Actrium/Models/ActrPackageManifestReader.swift"
  "ios/Actrium/Models/TabRuntimeBridge.swift"
  "ios/Actrium/Models/TagCoreBridge.swift"
  "macos/Actrium/Database/AppRepository.swift"
  "macos/Actrium/Database/BrowserDataRepository.swift"
  "macos/Actrium/Database/BookmarkRepository.swift"
  "macos/Actrium/Database/DatabaseManager.swift"
  "macos/Actrium/Database/HistoryRepository.swift"
  "macos/Actrium/Database/TagRepository.swift"
  "macos/Actrium/Models/ActriumAppIdentityCoreBridge.swift"
  "macos/Actrium/Models/ActrPackageManifestReader.swift"
  "macos/Actrium/Models/BrowserSearchEngineCoreBridge.swift"
  "macos/Actrium/Models/InstalledActriumAppCoreBridge.swift"
  "macos/Actrium/Models/TabRuntimeBridge.swift"
  "macos/Actrium/Models/TagCoreBridge.swift"
  "macos/Actrium/Settings/Sync/BrowserSyncCredentialsProvider.swift"
  "macos/Actrium/Settings/Sync/BrowserSyncProviderConnectionPlanner.swift"
)

apple_direct_actriumcore_import_files="$(
  rg -l '^import ActriumCore' \
    ios/Actrium \
    macos/Actrium \
    --glob '*.swift' \
    2>/dev/null | sort || true
)"

apple_unexpected_actriumcore_imports="$(
  while IFS= read -r file; do
    [[ -z "$file" ]] && continue

    is_allowed=false
    for allowed_file in "${apple_allowed_actriumcore_import_files[@]}"; do
      if [[ "$file" == "$allowed_file" ]]; then
        is_allowed=true
        break
      fi
    done

    if [[ "$is_allowed" == false ]]; then
      echo "$file"
    fi
  done <<< "$apple_direct_actriumcore_import_files"
)"

if [[ -n "$apple_unexpected_actriumcore_imports" ]]; then
  print_failure "Apple app ActriumCore imports escaped the adapter/repository allowlist"
  printf '%s\n' "$apple_unexpected_actriumcore_imports"
else
  print_ok "Apple app ActriumCore imports stay confined to adapter/repository boundary files"
fi

runtime_hash_dependency_files=(
  "packages/actrium/package.json"
  "packages/actrium/yarn.lock"
  "packages/actrium/bun.lock"
  "android/package.json"
  "android/yarn.lock"
)

runtime_hash_dependency_residue="$(
  rg -n 'crypto-js|@types/crypto-js' \
    "${runtime_hash_dependency_files[@]}" \
    2>/dev/null || true
)"

if [[ -n "$runtime_hash_dependency_residue" ]]; then
  print_failure "runtime package manifests/locks reintroduce unused CryptoJS dependencies"
  printf '%s\n' "$runtime_hash_dependency_residue"
else
  print_ok "runtime package manifests/locks avoid unused CryptoJS dependencies"
fi

# Windows OS-level interop (dwmapi window chrome) is not Rust-core access:
# only the actrium_core boundary must stay centralized in ActriumCore.cs.
windows_native_imports="$(
  rg -n 'DllImport\(|LibraryImport\(|actrium_core_' windows/src \
    --glob '!ActriumCore.cs' 2>/dev/null \
    | rg -v 'DllImport\("dwmapi\.dll"' \
    || true
)"

if [[ -n "$windows_native_imports" ]]; then
  print_failure "Windows UI files bypass the centralized ActriumCore adapter"
  printf '%s\n' "$windows_native_imports"
else
  print_ok "Windows native core access is centralized in ActriumCore.cs"
fi

windows_browser_data_requirements=(
  "windows/src/BrowserHistoryStore.cs|ActriumCore.NormalizeBrowserHistorySnapshotJson|Windows BrowserHistoryStore delegates snapshot schema normalization to ActriumCore"
  "windows/src/BrowserBookmarkStore.cs|ActriumCore.NormalizeBrowserBookmarkSnapshotJson|Windows BrowserBookmarkStore delegates snapshot schema normalization to ActriumCore"
  "windows/src/BrowserSessionStore.cs|ActriumCore.NormalizeBrowserSessionSnapshotJson|Windows BrowserSessionStore delegates snapshot schema normalization to ActriumCore"
  "windows/src/ActriumCore.cs|actrium_core_browser_history_snapshot_normalize_json|Windows ActriumCore adapter exposes browser data C ABI"
  "windows/src/ActriumCore.cs|LoadBrowserStorageSchema|Windows ActriumCore adapter exposes browser storage schema"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_schema_statements_json|Windows ActriumCore adapter exposes browser storage schema C ABI"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_sql_json|Windows ActriumCore adapter exposes browser storage sync SQL C ABI"
  "windows/src/ActriumCore.cs|LoadBrowserStorageDataModelJson|Windows ActriumCore adapter exposes browser storage data model"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_data_model_json|Windows ActriumCore adapter exposes browser storage data model C ABI"
  "windows/src/ActriumCore.cs|LoadBrowserStorageSyncConflictPolicyJson|Windows ActriumCore adapter exposes browser sync conflict policy"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_conflict_policy_json|Windows ActriumCore adapter exposes browser sync conflict policy C ABI"
  "windows/src/ActriumCore.cs|LoadBrowserStorageSyncMergePolicyJson|Windows ActriumCore adapter exposes browser sync merge policy"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_merge_policy_json|Windows ActriumCore adapter exposes browser sync merge policy C ABI"
  "windows/src/ActriumCore.cs|LoadBrowserStorageAddressCanonicalizationPolicyJson|Windows ActriumCore adapter exposes browser storage address canonicalization policy"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_address_canonicalization_policy_json|Windows ActriumCore adapter exposes browser storage address canonicalization policy C ABI"
  "windows/src/ActriumCore.cs|CanonicalizeBrowserStorageAddressesJson|Windows ActriumCore adapter exposes browser storage address canonicalization"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_canonicalize_addresses_json|Windows ActriumCore adapter exposes browser storage address canonicalization C ABI"
  "windows/src/ActriumCore.cs|LoadBrowserStorageBookmarkFolderTreeIntegrityPolicyJson|Windows ActriumCore adapter exposes bookmark folder tree integrity policy"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_bookmark_folder_tree_integrity_policy_json|Windows ActriumCore adapter exposes bookmark folder tree integrity policy C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageBookmarkFolderTreeIntegrityPlanJson|Windows ActriumCore adapter exposes bookmark folder tree integrity planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_bookmark_folder_tree_integrity_plan_json|Windows ActriumCore adapter exposes bookmark folder tree integrity plan C ABI"
  "windows/src/ActriumCore.cs|LoadBrowserStorageBookmarkRestorePlacementPolicyJson|Windows ActriumCore adapter exposes bookmark restore placement policy"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_bookmark_restore_placement_policy_json|Windows ActriumCore adapter exposes bookmark restore placement policy C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageBookmarkRestorePlacementPlanJson|Windows ActriumCore adapter exposes bookmark restore placement planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_bookmark_restore_placement_plan_json|Windows ActriumCore adapter exposes bookmark restore placement plan C ABI"
  "windows/src/ActriumCore.cs|LoadBrowserStorageBookmarkOwnershipPolicyJson|Windows ActriumCore adapter exposes bookmark ownership policy"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_bookmark_ownership_policy_json|Windows ActriumCore adapter exposes bookmark ownership policy C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageBookmarkOwnershipPlanJson|Windows ActriumCore adapter exposes bookmark ownership planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_bookmark_ownership_plan_json|Windows ActriumCore adapter exposes bookmark ownership plan C ABI"
  "windows/src/ActriumCore.cs|LoadBrowserStorageHistoryVisitAggregationPolicyJson|Windows ActriumCore adapter exposes history visit aggregation policy"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_history_visit_aggregation_policy_json|Windows ActriumCore adapter exposes history visit aggregation policy C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageHistoryVisitAggregationPlanJson|Windows ActriumCore adapter exposes history visit aggregation planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_history_visit_aggregation_plan_json|Windows ActriumCore adapter exposes history visit aggregation plan C ABI"
  "windows/src/ActriumCore.cs|LoadBrowserStorageSessionTabOrderingPolicyJson|Windows ActriumCore adapter exposes session tab ordering policy"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_session_tab_ordering_policy_json|Windows ActriumCore adapter exposes session tab ordering policy C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSessionTabOrderingPlanJson|Windows ActriumCore adapter exposes session tab ordering planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_session_tab_ordering_plan_json|Windows ActriumCore adapter exposes session tab ordering plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSessionTabIdPlanJson|Windows ActriumCore adapter exposes session tab id planning"
  "windows/src/ActriumCore.cs|LoadBrowserStorageSessionActiveTabConsistencyPolicyJson|Windows ActriumCore adapter exposes session active-tab consistency policy"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_session_active_tab_consistency_policy_json|Windows ActriumCore adapter exposes session active-tab consistency policy C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSessionActiveTabConsistencyPlanJson|Windows ActriumCore adapter exposes session active-tab consistency planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_session_active_tab_consistency_plan_json|Windows ActriumCore adapter exposes session active-tab consistency plan C ABI"
  "windows/src/ActriumCore.cs|LoadBrowserStorageRetentionPruningPolicyJson|Windows ActriumCore adapter exposes retention pruning policy"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_retention_pruning_policy_json|Windows ActriumCore adapter exposes retention pruning policy C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageRetentionPruningPlanJson|Windows ActriumCore adapter exposes retention pruning planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_retention_pruning_plan_json|Windows ActriumCore adapter exposes retention pruning plan C ABI"
  "windows/src/ActriumCore.cs|NormalizeBrowserStorageSyncBatchJson|Windows ActriumCore adapter exposes browser sync batch normalization"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_batch_normalize_json|Windows ActriumCore adapter exposes browser sync batch C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncBatchFromDirtyRowsJson|Windows ActriumCore adapter exposes dirty-row batch building"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_batch_from_dirty_rows_json|Windows ActriumCore adapter exposes dirty-row batch C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncPushRequestFromDirtyRowsJson|Windows ActriumCore adapter exposes dirty-row push request building"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_push_request_from_dirty_rows_json|Windows ActriumCore adapter exposes dirty-row push request C ABI"
  "windows/src/ActriumCore.cs|NormalizeBrowserStorageSyncAckBatchJson|Windows ActriumCore adapter exposes browser sync ack normalization"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_ack_batch_normalize_json|Windows ActriumCore adapter exposes browser sync ack batch C ABI"
  "windows/src/ActriumCore.cs|NormalizeBrowserStorageSyncCursorsJson|Windows ActriumCore adapter exposes browser sync cursor normalization"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cursors_normalize_json|Windows ActriumCore adapter exposes browser sync cursor C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncPushRequestJson|Windows ActriumCore adapter exposes browser sync push request building"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_push_request_json|Windows ActriumCore adapter exposes browser sync push request C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncPullRequestJson|Windows ActriumCore adapter exposes browser sync pull request building"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_pull_request_json|Windows ActriumCore adapter exposes browser sync pull request C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCycleResponseJson|Windows ActriumCore adapter exposes browser sync cycle response building"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cycle_response_json|Windows ActriumCore adapter exposes browser sync cycle response C ABI"
  "windows/src/ActriumCore.cs|NormalizeBrowserStorageSyncCycleResponseJson|Windows ActriumCore adapter exposes browser sync cycle response normalization"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cycle_response_normalize_json|Windows ActriumCore adapter exposes browser sync cycle response normalization C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncClientCyclePlanJson|Windows ActriumCore adapter exposes browser sync client cycle planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_client_cycle_plan_json|Windows ActriumCore adapter exposes browser sync client cycle plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncClientAckBatchAfterRemoteApplyJson|Windows ActriumCore adapter exposes browser sync post-remote ack selection"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_client_ack_batch_after_remote_apply_json|Windows ActriumCore adapter exposes browser sync post-remote ack C ABI"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_cycle_client_apply_result_json|Windows ActriumCore adapter exposes cloud cycle apply result C ABI"
  "windows/src/ActriumCore.cs|LoadBrowserStorageSyncTransportContractJson|Windows ActriumCore adapter exposes browser sync transport contract"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_transport_contract_json|Windows ActriumCore adapter exposes browser sync transport contract C ABI"
  "windows/src/ActriumCore.cs|LoadBrowserStorageSyncCloudStoreContractJson|Windows ActriumCore adapter exposes browser sync cloud store contract"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_store_contract_json|Windows ActriumCore adapter exposes browser sync cloud store contract C ABI"
  "windows/src/ActriumCore.cs|LoadBrowserStorageSyncCloudProviderAdapterContractJson|Windows ActriumCore adapter exposes browser sync cloud provider adapter contract"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_provider_adapter_contract_json|Windows ActriumCore adapter exposes browser sync cloud provider adapter contract C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudProviderAdapterPlanJson|Windows ActriumCore adapter exposes browser sync cloud provider adapter planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_provider_adapter_plan_json|Windows ActriumCore adapter exposes browser sync cloud provider adapter plan C ABI"
  "windows/src/ActriumCore.cs|LoadBrowserStorageSyncCloudProviderConnectionContractJson|Windows ActriumCore adapter exposes browser sync cloud provider connection contract"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_provider_connection_contract_json|Windows ActriumCore adapter exposes browser sync cloud provider connection contract C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudProviderConnectionStatusPolicyJson|Windows ActriumCore adapter exposes browser sync cloud provider connection status policy"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_provider_connection_status_policy_json|Windows ActriumCore adapter exposes browser sync cloud provider connection status policy C ABI"
  "windows/src/ActriumCore.cs|LoadBrowserStorageSyncCloudProviderErrorPolicyJson|Windows ActriumCore adapter exposes browser sync cloud provider error policy"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudProviderErrorClassificationJson|Windows ActriumCore adapter exposes browser sync cloud provider error classification"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_provider_error_classification_json|Windows ActriumCore adapter exposes browser sync cloud provider error classifier C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudStoreCapabilityPreflightJson|Windows ActriumCore adapter exposes browser sync cloud store capability preflight"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_store_capability_preflight_json|Windows ActriumCore adapter exposes browser sync cloud store capability preflight C ABI"
  "windows/src/ActriumCore.cs|LoadBrowserStorageSyncCloudLocalFolderCapabilitiesJson|Windows ActriumCore adapter exposes browser sync local-folder capabilities"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_local_folder_capabilities_json|Windows ActriumCore adapter exposes browser sync local-folder capabilities C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudLocalFolderCapabilityPreflightJson|Windows ActriumCore adapter exposes browser sync local-folder capability preflight"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_local_folder_capability_preflight_json|Windows ActriumCore adapter exposes browser sync local-folder capability preflight C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudLocalFolderCompatibilityPreflightJson|Windows ActriumCore adapter exposes browser sync local-folder compatibility preflight"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_local_folder_compatibility_preflight_json|Windows ActriumCore adapter exposes browser sync local-folder compatibility preflight C ABI"
  "windows/src/ActriumCore.cs|LoadBrowserStorageSyncCloudDataModelJson|Windows ActriumCore adapter exposes browser sync cloud data model"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_data_model_json|Windows ActriumCore adapter exposes browser sync cloud data model C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudCompatibilityStatusJson|Windows ActriumCore adapter exposes browser sync cloud compatibility status"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_compatibility_status_json|Windows ActriumCore adapter exposes browser sync cloud compatibility status C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudCompatibilityPreflightJson|Windows ActriumCore adapter exposes browser sync cloud compatibility preflight"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_compatibility_preflight_json|Windows ActriumCore adapter exposes browser sync cloud compatibility preflight C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudSchemaRolloutPlanJson|Windows ActriumCore adapter exposes browser sync cloud schema rollout planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_schema_rollout_plan_json|Windows ActriumCore adapter exposes browser sync cloud schema rollout plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudSchemaMigrationPlaybookJson|Windows ActriumCore adapter exposes browser sync cloud schema migration playbook"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_schema_migration_playbook_json|Windows ActriumCore adapter exposes browser sync cloud schema migration playbook C ABI"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_schema_migration_plan_json|Windows ActriumCore adapter exposes the browser storage schema migration plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudRecoverySnapshotPlanJson|Windows ActriumCore adapter exposes browser sync cloud recovery snapshot planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_recovery_snapshot_plan_json|Windows ActriumCore adapter exposes browser sync cloud recovery snapshot plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudRecoverySnapshotResultPlanJson|Windows ActriumCore adapter exposes browser sync cloud recovery snapshot result handoff"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_recovery_snapshot_result_plan_json|Windows ActriumCore adapter exposes browser sync cloud recovery snapshot result C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudLocalFolderRecoverySnapshotRoundJson|Windows ActriumCore adapter exposes browser sync local-folder recovery snapshot round"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_local_folder_recovery_snapshot_round_json|Windows ActriumCore adapter exposes browser sync local-folder recovery snapshot round C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudDefaultManifestJson|Windows ActriumCore adapter exposes browser sync cloud manifest defaults"
  "windows/src/ActriumCore.cs|NormalizeBrowserStorageSyncCloudManifestJson|Windows ActriumCore adapter exposes browser sync cloud manifest normalization"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudLogObjectJson|Windows ActriumCore adapter exposes browser sync cloud log objects"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudSnapshotObjectJson|Windows ActriumCore adapter exposes browser sync cloud snapshot objects"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudMembershipPlanJson|Windows ActriumCore adapter exposes browser sync cloud membership planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_membership_plan_json|Windows ActriumCore adapter exposes browser sync cloud membership plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudMembershipResultPlanJson|Windows ActriumCore adapter exposes browser sync cloud membership result planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_membership_result_plan_json|Windows ActriumCore adapter exposes browser sync cloud membership result plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudPushPlanJson|Windows ActriumCore adapter exposes browser sync cloud push planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_push_plan_json|Windows ActriumCore adapter exposes browser sync cloud push plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudPushResultPlanJson|Windows ActriumCore adapter exposes browser sync cloud push result planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_push_result_plan_json|Windows ActriumCore adapter exposes browser sync cloud push result plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudPullPlanJson|Windows ActriumCore adapter exposes browser sync cloud pull planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_pull_plan_json|Windows ActriumCore adapter exposes browser sync cloud pull plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudPullApplyPlanJson|Windows ActriumCore adapter exposes browser sync cloud pull apply planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_pull_apply_plan_json|Windows ActriumCore adapter exposes browser sync cloud pull apply plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudPullContinuationPlanJson|Windows ActriumCore adapter exposes browser sync cloud pull continuation planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_pull_continuation_plan_json|Windows ActriumCore adapter exposes browser sync cloud pull continuation plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudCompactionPlanJson|Windows ActriumCore adapter exposes browser sync cloud compaction planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_compaction_plan_json|Windows ActriumCore adapter exposes browser sync cloud compaction plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudCompactionResultPlanJson|Windows ActriumCore adapter exposes browser sync cloud compaction result planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_compaction_result_plan_json|Windows ActriumCore adapter exposes browser sync cloud compaction result plan C ABI"
  "windows/src/ActriumCore.cs|RunBrowserStorageSyncCloudLocalFolderCompactionRoundJson|Windows ActriumCore adapter exposes browser sync local-folder compaction round"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_local_folder_compaction_round_json|Windows ActriumCore adapter exposes browser sync local-folder compaction round C ABI"
  "windows/src/ActriumCore.cs|RunBrowserStorageSyncCloudLocalFolderCleanupRoundJson|Windows ActriumCore adapter exposes browser sync local-folder cleanup round"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_local_folder_cleanup_round_json|Windows ActriumCore adapter exposes browser sync local-folder cleanup round C ABI"
  "windows/src/ActriumCore.cs|LoadBrowserStorageSyncCloudHighWatermarkPolicyJson|Windows ActriumCore adapter exposes browser sync cloud high-watermark policy"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_high_watermark_policy_json|Windows ActriumCore adapter exposes browser sync cloud high-watermark policy C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudHighWatermarkPlanJson|Windows ActriumCore adapter exposes browser sync cloud high-watermark planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_high_watermark_plan_json|Windows ActriumCore adapter exposes browser sync cloud high-watermark plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudCleanupResultPlanJson|Windows ActriumCore adapter exposes browser sync cloud cleanup result planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_cleanup_result_plan_json|Windows ActriumCore adapter exposes browser sync cloud cleanup result plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudCycleClientPlanJson|Windows ActriumCore adapter exposes browser sync cloud cycle client planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_cycle_client_plan_json|Windows ActriumCore adapter exposes browser sync cloud cycle client plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudCycleStatusPolicyJson|Windows ActriumCore adapter exposes browser sync cloud cycle status policy"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_cycle_status_policy_json|Windows ActriumCore adapter exposes browser sync cloud cycle status policy C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudMembershipStatusPolicyJson|Windows ActriumCore adapter exposes browser sync cloud membership status policy"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_membership_status_policy_json|Windows ActriumCore adapter exposes browser sync cloud membership status policy C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudCycleScheduleStateJson|Windows ActriumCore adapter exposes browser sync cloud cycle schedule state"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_cycle_schedule_state_json|Windows ActriumCore adapter exposes browser sync cloud cycle schedule state C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudMembershipScheduleStateJson|Windows ActriumCore adapter exposes browser sync cloud membership schedule state"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_membership_schedule_state_json|Windows ActriumCore adapter exposes browser sync cloud membership schedule state C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudMembershipAttemptPlanJson|Windows ActriumCore adapter exposes browser sync cloud membership attempt plan"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_membership_attempt_plan_json|Windows ActriumCore adapter exposes browser sync cloud membership attempt plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudMembershipLoopPlanJson|Windows ActriumCore adapter exposes browser sync cloud membership loop plan"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_membership_loop_plan_json|Windows ActriumCore adapter exposes browser sync cloud membership loop plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudProviderDiagnosticsJson|Windows ActriumCore adapter exposes browser sync cloud provider diagnostics"
  "windows/src/ActriumCore.cs|providerErrorClassificationJson|Windows ActriumCore diagnostics adapter accepts provider error classification"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_provider_diagnostics_json|Windows ActriumCore adapter exposes browser sync cloud provider diagnostics C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudObservabilityEventJson|Windows ActriumCore adapter exposes browser sync cloud observability events"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_observability_event_json|Windows ActriumCore adapter exposes browser sync cloud observability event C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudCycleAttemptPlanJson|Windows ActriumCore adapter exposes browser sync cloud cycle attempt planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_cycle_attempt_plan_json|Windows ActriumCore adapter exposes browser sync cloud cycle attempt plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudCycleLoopPlanJson|Windows ActriumCore adapter exposes browser sync cloud cycle loop planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_cycle_loop_plan_json|Windows ActriumCore adapter exposes browser sync cloud cycle loop plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCloudLocalFolderCycleRoundJson|Windows ActriumCore adapter exposes browser sync local-folder cycle rounds"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_local_folder_cycle_round_json|Windows ActriumCore adapter exposes browser sync local-folder cycle round C ABI"
  "windows/src/ActriumCore.cs|RunBrowserStorageSyncCloudLocalFolderMembershipRoundJson|Windows ActriumCore adapter exposes browser sync local-folder membership rounds"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cloud_local_folder_membership_round_json|Windows ActriumCore adapter exposes browser sync local-folder membership round C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCycleRequestPlanJson|Windows ActriumCore adapter exposes browser sync cycle request planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cycle_request_plan_json|Windows ActriumCore adapter exposes browser sync cycle request plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncCycleTransportResultJson|Windows ActriumCore adapter exposes browser sync transport result classification"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_cycle_transport_result_json|Windows ActriumCore adapter exposes browser sync transport result C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncStatusPolicyJson|Windows ActriumCore adapter exposes browser sync status policy"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_status_policy_json|Windows ActriumCore adapter exposes browser sync status policy C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncScheduleStateJson|Windows ActriumCore adapter exposes browser sync schedule state"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_schedule_state_json|Windows ActriumCore adapter exposes browser sync schedule state C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncSchedulerStatusJson|Windows ActriumCore adapter exposes browser sync scheduler status"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_scheduler_status_json|Windows ActriumCore adapter exposes browser sync scheduler status C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncAckPlanJson|Windows ActriumCore adapter exposes browser sync ack planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_ack_plan_json|Windows ActriumCore adapter exposes browser sync ack plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncRemoteApplyPlanJson|Windows ActriumCore adapter exposes browser sync remote apply planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_remote_apply_plan_json|Windows ActriumCore adapter exposes browser sync remote apply C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncRemoteApplyResultJson|Windows ActriumCore adapter exposes browser sync remote apply result summaries"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_remote_apply_result_json|Windows ActriumCore adapter exposes browser sync remote apply result C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncServerMergePlanJson|Windows ActriumCore adapter exposes browser sync reference merge planning"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_server_merge_plan_json|Windows ActriumCore adapter exposes browser sync reference merge plan C ABI"
  "windows/src/ActriumCore.cs|BuildBrowserStorageSyncServerMergeResultJson|Windows ActriumCore adapter exposes browser sync reference merge result summaries"
  "windows/src/ActriumCore.cs|actrium_core_browser_storage_sync_server_merge_result_json|Windows ActriumCore adapter exposes browser sync reference merge result C ABI"
  "windows/src/ActriumCore.cs|LoadAuthEnvironmentProd|Windows ActriumCore adapter exposes Rust auth environment defaults"
  "windows/src/ActriumCore.cs|actrium_core_auth_account_base_url|Windows ActriumCore adapter exposes Rust auth endpoint C ABI"
  "windows/src/BrowserStorageDatabase.cs|ActriumCore.LoadBrowserStorageSchema|Windows browser storage adapter consumes Rust storage schema"
  "windows/src/BrowserStorageDatabase.cs|LoadBrowserStorageSchemaMigrationPlanJson|Windows browser storage adapter consumes the Rust schema migration plan"
  "windows/src/BrowserStorageDatabase.cs|Microsoft.Data.Sqlite|Windows browser storage adapter owns SQLite execution"
  "windows/src/BrowserStorageDatabase.cs|browser_history_pages|Windows browser storage adapter targets Rust-owned browser storage tables"
  "windows/src/BrowserStorageDatabase.cs|tab_id|Windows browser storage adapter persists stable browser session tab ids"
  "windows/src/BrowserStorageDatabase.cs|BuildBrowserStorageSessionTabIdPlanJson|Windows browser storage adapter derives session tab ids through the Rust batch plan"
  "windows/src/BrowserStorageDatabase.cs|deleted_at|Windows browser storage adapter preserves browser tombstones"
  "windows/src/BrowserStorageDatabase.cs|SelectSyncCursorsForScope|Windows browser storage adapter reads sync cursors through the Rust contract"
  "windows/src/BrowserStorageDatabase.cs|ApplyRemoteSyncPlan|Windows browser storage adapter executes Rust browser sync remote apply plans"
  "windows/src/BrowserStorageDatabase.cs|ApplySyncAckPlan|Windows browser storage adapter executes Rust browser sync ack plans"
  "windows/src/BrowserStorageDatabase.cs|ApplySyncCycleResponse|Windows browser storage adapter applies Rust-planned sync cycles"
  "windows/src/BrowserStorageDatabase.cs|ApplyCloudCycleClientPlan|Windows browser storage adapter applies Rust cloud cycle client plans"
  "windows/src/BrowserStorageDatabase.cs|ExportDirtySyncBatch|Windows browser storage adapter exports dirty rows through Rust batch builder"
  "windows/src/BrowserStorageDatabase.cs|ExportDirtySyncPushRequest|Windows browser storage adapter exports dirty rows through Rust push builder"
  "windows/src/BrowserStorageDatabase.cs|ExportSyncCycleRequestPlan|Windows browser storage adapter exports Rust-planned sync cycle requests"
  "windows/src/BrowserStorageDatabase.cs|SelectDirtyStatements|Windows browser storage adapter consumes Rust dirty-row SQL selectors"
  "windows/src/BrowserStorageDatabase.cs|BrowserStorageSyncSqlContract|Windows browser storage adapter consumes Rust browser sync SQL"
  "windows/src/BrowserStorageSyncClient.cs|IBrowserStorageSyncTransport|Windows browser sync client uses injectable transport"
  "windows/src/BrowserStorageSyncClient.cs|BuildBrowserStorageSyncCycleTransportResultJson|Windows browser sync client classifies transport through Rust"
  "windows/src/BrowserStorageSyncClient.cs|RunLocalFolderSyncCycleAsync|Windows browser sync client exposes local-folder cloud cycle execution"
  "windows/src/BrowserStorageSyncClient.cs|RunLocalFolderMembershipRoundAsync|Windows browser sync client exposes local-folder cloud membership execution"
  "windows/src/BrowserStorageSyncClient.cs|BuildBrowserStorageSyncCloudLocalFolderCapabilityPreflightJson|Windows browser sync client consumes Rust local-folder capability preflight"
  "windows/src/BrowserStorageSyncClient.cs|BuildBrowserStorageSyncCloudLocalFolderCompatibilityPreflightJson|Windows browser sync client consumes Rust local-folder compatibility preflight"
  "windows/src/BrowserStorageSyncClient.cs|compatibilityPreflight|Windows browser sync client returns Rust local-folder compatibility preflight"
  "windows/src/BrowserStorageSyncClient.cs|BuildBrowserStorageSyncCloudLocalFolderCycleRoundJson|Windows browser sync client consumes Rust local-folder cycle helper"
  "windows/src/BrowserStorageSyncClient.cs|RunBrowserStorageSyncCloudLocalFolderCompactionRoundJson|Windows browser sync client consumes Rust local-folder compaction helper"
  "windows/src/BrowserStorageSyncClient.cs|RunBrowserStorageSyncCloudLocalFolderCleanupRoundJson|Windows browser sync client consumes Rust local-folder cleanup helper"
  "windows/src/BrowserStorageSyncClient.cs|BuildCloudSchemaRolloutPlan|Windows browser sync client consumes Rust schema rollout planning on compatibility block"
  "windows/src/BrowserStorageSyncClient.cs|BuildCloudSchemaMigrationPlaybook|Windows browser sync client consumes Rust schema migration playbook on compatibility block"
  "windows/src/BrowserStorageSyncClient.cs|BuildLocalFolderRecoverySnapshotRound|Windows browser sync client consumes Rust local-folder recovery snapshot round on compatibility block"
  "windows/src/BrowserStorageSyncClient.cs|RunBrowserStorageSyncCloudLocalFolderMembershipRoundJson|Windows browser sync client consumes Rust local-folder membership helper"
  "windows/src/BrowserStorageSyncClient.cs|BuildCloudMembershipStatusPolicy|Windows browser sync membership client consumes Rust membership status policy"
  "windows/src/BrowserStorageSyncClient.cs|\"statusPolicy\"|Windows browser sync membership client returns Rust membership status policy"
  "windows/src/BrowserStorageSyncClient.cs|BuildCloudMembershipScheduleState|Windows browser sync membership client consumes Rust membership schedule state"
  "windows/src/BrowserStorageSyncClient.cs|\"scheduleState\"|Windows browser sync membership client returns Rust membership schedule state"
  "windows/src/BrowserStorageSyncClient.cs|BuildCloudMembershipAttemptPlan|Windows browser sync membership client consumes Rust membership attempt plan"
  "windows/src/BrowserStorageSyncClient.cs|\"attemptPlan\"|Windows browser sync membership client returns Rust membership attempt plan"
  "windows/src/BrowserStorageSyncClient.cs|BuildCloudMembershipLoopPlan|Windows browser sync membership client consumes Rust membership loop plan"
  "windows/src/BrowserStorageSyncClient.cs|\"loopPlan\"|Windows browser sync membership client returns Rust membership loop plan"
  "windows/src/BrowserStorageSyncClient.cs|schemaRolloutPlan = BuildCloudSchemaRolloutPlan|Windows browser sync membership client prepares Rust schema rollout on compatibility block"
  "windows/src/BrowserStorageSyncClient.cs|observabilityEvent = BuildCloudObservabilityEvent|Windows browser sync membership client exposes Rust observability events"
  "windows/src/BrowserStorageSyncClient.cs|BuildBrowserStorageSyncCloudCycleScheduleStateJson|Windows browser sync client consumes Rust cloud cycle schedule state"
  "windows/src/BrowserStorageSyncClient.cs|BuildBrowserStorageSyncCloudProviderDiagnosticsJson|Windows browser sync client consumes Rust cloud provider diagnostics"
  "windows/src/BrowserStorageSyncClient.cs|BuildLocalFolderProviderErrorClassification|Windows browser sync client builds provider error classification from real local-folder rounds"
  "windows/src/BrowserStorageSyncClient.cs|localFolderProviderRoundErrors|Windows browser sync client sources provider errors from local-folder rounds"
  "windows/src/BrowserStorageSyncClient.cs|BuildBrowserStorageSyncCloudObservabilityEventJson|Windows browser sync client consumes Rust cloud observability events"
  "windows/src/BrowserStorageSyncClient.cs|BuildBrowserStorageSyncCloudCycleAttemptPlanJson|Windows browser sync client consumes Rust cloud cycle attempt planning"
  "windows/src/BrowserStorageSyncClient.cs|BuildBrowserStorageSyncCloudCycleLoopPlanJson|Windows browser sync client consumes Rust cloud cycle loop planning"
  "windows/src/BrowserStorageSyncCoordinator.cs|IBrowserStorageSyncCredentialsProvider|Windows browser sync coordinator uses injectable credential lookup"
  "windows/src/BrowserStorageSyncCoordinator.cs|BrowserStorageSyncClient|Windows browser sync coordinator delegates network/apply work to the sync client"
  "windows/src/BrowserStorageSyncCoordinator.cs|CreateDefault|Windows browser sync coordinator creates default client and credential store"
  "windows/src/BrowserStorageSyncCoordinator.cs|statusPolicy|Windows browser sync coordinator returns Rust-owned status policy payloads"
  "windows/src/BrowserStorageSyncCoordinator.cs|scheduleState|Windows browser sync coordinator returns Rust-owned schedule state payloads"
  "windows/src/BrowserStorageSyncScheduler.cs|BrowserStorageSyncSchedulerSnapshot|Windows browser sync scheduler exposes platform schedule snapshots"
  "windows/src/BrowserStorageSyncScheduler.cs|NextRunDelaySeconds|Windows browser sync scheduler consumes Rust-owned schedule state timing"
  "windows/src/BrowserStorageSyncScheduler.cs|StatusJson|Windows browser sync scheduler exposes Rust-owned product status"
  "windows/src/BrowserStorageSyncScheduler.cs|ScheduleAfterLocalChange|Windows browser sync scheduler can debounce local browser data changes"
  "windows/src/BrowserMenuModel.cs|ConfigureSync|Windows app menu exposes browser sync credential configuration"
  "windows/src/BrowserMenuModel.cs|SyncNow|Windows app menu exposes manual browser sync"
  "windows/src/MainForm.cs|ConfigureBrowserSyncConnection|Windows main form wires browser sync credential configuration"
  "windows/src/MainForm.cs|SyncBrowserDataNowAsync|Windows main form wires manual browser sync"
  "windows/src/MainForm.cs|ScheduleBrowserDataSyncAfterLocalChange|Windows main form schedules sync after local browser data changes"
  "windows/src/MainForm.cs|ScheduleBrowserDataSyncAfterConnectionChange|Windows main form schedules sync after connection changes"
  "windows/src/BrowserSyncStatusPolicy.cs|FormatStatusJson|Windows UI formats Rust-owned browser sync status locally"
  "windows/src/BrowserStorageSyncCredentialStore.cs|IBrowserStorageSyncCredentialsProvider|Windows browser sync credential store implements coordinator credential lookup"
  "windows/src/BrowserStorageSyncCredentialStore.cs|SaveSession|Windows browser sync credential store persists account sync sessions"
  "windows/src/BrowserStorageSyncCredentialStore.cs|browser-sync-credentials.json|Windows browser sync credential store persists local credential snapshot"
  "windows/src/BrowserSyncConnectionDialog.cs|BrowserSyncConnectionDialog|Windows browser sync configuration dialog exists"
  "windows/tests/Actrium.Windows.Tests.csproj|BrowserStorageSyncCoordinator.cs|Windows tests compile the browser sync coordinator"
  "windows/tests/Actrium.Windows.Tests.csproj|BrowserStorageSyncCredentialStore.cs|Windows tests compile the browser sync credential store"
  "windows/tests/Actrium.Windows.Tests.csproj|BrowserStorageSyncScheduler.cs|Windows tests compile the browser sync scheduler"
  "windows/tests/Actrium.Windows.Tests.csproj|BrowserSyncStatusPolicy.cs|Windows tests compile browser sync status UI policy"
  "windows/tests/Program.cs|migrates browser storage sqlite v1 v2 v3 to v6|Windows tests cover browser storage v1/v2/v3 SQLite migration"
  "windows/tests/Program.cs|BrowserSyncBatchHasBookmarkOwner|Windows tests cover browser bookmark owner sync payloads"
  "windows/tests/Program.cs|ApplySyncCycleResponse|Windows tests cover browser sync cycle apply"
  "windows/tests/Program.cs|ExportSyncCycleRequestPlan|Windows tests cover browser sync cycle request planning"
  "windows/tests/Program.cs|runs browser storage sync client cycle|Windows tests cover browser sync client cycle"
  "windows/tests/Program.cs|runs browser storage local-folder cloud sync client cycle|Windows tests cover local-folder cloud sync client"
  "windows/tests/Program.cs|runs browser storage local-folder cloud membership client round|Windows tests cover local-folder cloud membership client"
  "windows/tests/Program.cs|finishCloudDeviceRevoked|Windows local-folder cloud membership test asserts Rust revoke classification"
  "windows/tests/Program.cs|capabilityPreflight|Windows local-folder cloud sync test asserts Rust capability preflight"
  "windows/tests/Program.cs|compatibilityPreflight|Windows local-folder cloud sync test asserts Rust compatibility preflight"
  "windows/tests/Program.cs|clientUpgradeRequired|Windows local-folder cloud sync test asserts compatibility blocking"
  "windows/tests/Program.cs|cloudCycleScheduleState|Windows local-folder cloud sync test asserts Rust schedule state"
  "windows/tests/Program.cs|providerDiagnostics|Windows local-folder cloud sync test asserts Rust provider diagnostics"
  "windows/tests/Program.cs|cloudObservabilityEvent|Windows local-folder cloud sync test asserts Rust observability event"
  "windows/tests/Program.cs|loopPlan|Windows local-folder cloud sync test asserts Rust loop planning"
  "windows/tests/Program.cs|compactionRound|Windows local-folder cloud sync test asserts Rust compaction round"
  "windows/tests/Program.cs|cleanupRound|Windows local-folder cloud sync test asserts Rust cleanup round"
  "windows/tests/Program.cs|recoverySnapshotRound|Windows local-folder cloud sync test asserts Rust recovery snapshot round"
  "windows/tests/Program.cs|providerDiagnostics.*compatibilityClassification|Windows local-folder cloud membership test asserts Rust provider diagnostics"
  "windows/tests/Program.cs|observabilityEvent.*planType|Windows local-folder cloud membership test asserts Rust observability event"
  "windows/tests/Program.cs|cloudMembershipStatusPolicy|Windows local-folder cloud membership test asserts Rust membership status policy"
  "windows/tests/Program.cs|cloudMembershipScheduleState|Windows local-folder cloud membership test asserts Rust membership schedule state"
  "windows/tests/Program.cs|cloudMembershipAttemptPlan|Windows local-folder cloud membership test asserts Rust membership attempt plan"
  "windows/tests/Program.cs|cloudMembershipLoopPlan|Windows local-folder cloud membership test asserts Rust membership loop plan"
  "windows/tests/Program.cs|finishMembershipAttemptCount|Windows local-folder cloud membership test asserts Rust membership loop summary"
  "windows/tests/Program.cs|cloudProviderAdapterPlan|Windows tests cover Rust provider adapter object-space plan"
  "windows/tests/Program.cs|cloudProviderConnectionStatusPolicy|Windows tests cover Rust provider connection status policy"
  "windows/tests/Program.cs|classifies browser sync cloud provider errors through Rust|Windows tests cover Rust provider error classification"
  "windows/tests/Program.cs|providerErrorClassification|Windows tests cover provider error diagnostics source mapping"
  "windows/tests/Program.cs|blockedJoinResult.*schemaRolloutPlan.*planType|Windows local-folder cloud membership test asserts Rust schema rollout on compatibility block"
  "windows/tests/Program.cs|skips browser storage sync coordinator without credentials|Windows tests cover browser sync credential gating"
  "windows/tests/Program.cs|runs browser storage sync coordinator with credentials|Windows tests cover browser sync coordinator delegation"
  "windows/tests/Program.cs|waitForCredentials|Windows tests cover sync coordinator status policy"
  "windows/tests/Program.cs|schedules browser storage sync retry from status policy|Windows tests cover sync coordinator schedule state"
  "windows/tests/Program.cs|browser storage sync scheduler consumes schedule state and resets after success|Windows tests cover sync scheduler retry lifecycle"
  "windows/tests/Program.cs|retryScheduled|Windows tests cover Rust-owned sync scheduler status"
  "windows/tests/Program.cs|formats browser sync status for product UI|Windows tests cover browser sync status UI policy"
  "windows/tests/Program.cs|persists browser storage sync credentials|Windows tests cover browser sync credential persistence"
  "windows/tests/Program.cs|runs browser storage sync coordinator with stored credentials|Windows tests cover coordinator credential-store integration"
  "windows/src/BrowserHistoryStore.cs|BrowserStorageDatabase|Windows BrowserHistoryStore uses shared browser storage database"
  "windows/src/BrowserBookmarkStore.cs|BrowserStorageDatabase|Windows BrowserBookmarkStore uses shared browser storage database"
  "windows/src/BrowserSessionStore.cs|BrowserStorageDatabase|Windows BrowserSessionStore uses shared browser storage database"
  "crates/actrium-core/src/browser_data.rs|browser_history_pages|Rust owns browser history storage schema"
  "crates/actrium-core/src/browser_data.rs|select_session|Rust owns browser session read SQL"
  "crates/actrium-core/src/browser_data.rs|browser_session_tabs|Rust owns browser session storage schema"
  "crates/actrium-core/src/browser_data.rs|tab_id TEXT|Rust browser session schema owns stable tab ids"
  "crates/actrium-core/src/browser_data.rs|sync_state TEXT|Rust browser storage schema owns sync state"
  "crates/actrium-core/src/browser_data.rs|deleted_at INTEGER|Rust browser storage schema owns tombstone columns"
  "crates/actrium-core/src/browser_data.rs|owner_kind TEXT|Rust browser bookmark storage schema owns owner kind"
  "crates/actrium-core/src/browser_data.rs|ownerKind|Rust browser bookmark sync payloads include owner kind"
  "crates/actrium-core/src/browser_data.rs|surface_kind TEXT NOT NULL DEFAULT 'browser'|Rust browser session storage schema owns surface kind"
  "crates/actrium-core/src/browser_data.rs|surfaceKind|Rust browser session sync payloads include surface kind"
  "crates/actrium-core/src/browser_data.rs|browser_sync_cursors|Rust browser storage schema owns sync cursors"
  "crates/actrium-core/src/browser_data.rs|select_dirty_bookmarks|Rust browser storage schema owns dirty-row sync SQL"
  "crates/actrium-core/src/c_api.rs|actrium_core_auth_environment_prod|Rust C ABI exposes auth environment defaults"
  "crates/actrium-core/src/c_api.rs|actrium_core_auth_account_base_url|Rust C ABI exposes auth account base URL"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncBatch|Rust browser storage sync owns typed batch payloads"
  "crates/actrium-core/src/browser_data.rs|BROWSER_STORAGE_ADDRESS_CANONICALIZATION_POLICY|Rust browser storage sync owns address canonicalization policy"
  "crates/actrium-core/src/browser_data.rs|browser_storage_canonicalize_addresses_json|Rust browser storage sync owns batched address canonicalization"
  "crates/actrium-core/src/browser_data.rs|credential_stripped|Rust browser storage sync owns URL credential stripping diagnostics"
  "crates/actrium-core/src/browser_data.rs|canonicalization_summary|Rust browser storage sync owns address canonicalization summaries"
  "crates/actrium-core/src/browser_data.rs|BROWSER_STORAGE_BOOKMARK_FOLDER_TREE_INTEGRITY_POLICY|Rust browser storage sync owns bookmark folder tree integrity policy"
  "crates/actrium-core/src/browser_data.rs|browser_storage_bookmark_folder_tree_integrity_plan_json|Rust browser storage sync owns batched bookmark folder tree integrity planning"
  "crates/actrium-core/src/browser_data.rs|integrity_action|Rust browser storage sync owns bookmark folder integrity diagnostics"
  "crates/actrium-core/src/browser_data.rs|integrity_summary|Rust browser storage sync owns bookmark folder integrity summaries"
  "crates/actrium-core/src/browser_data.rs|BROWSER_STORAGE_BOOKMARK_RESTORE_PLACEMENT_POLICY|Rust browser storage sync owns bookmark restore placement policy"
  "crates/actrium-core/src/browser_data.rs|browser_storage_bookmark_restore_placement_plan_json|Rust browser storage sync owns batched bookmark restore placement planning"
  "crates/actrium-core/src/browser_data.rs|placement_reason|Rust browser storage sync owns bookmark restore placement reasons"
  "crates/actrium-core/src/browser_data.rs|state_conflict_action|Rust browser storage sync owns bookmark delete/restore state diagnostics"
  "crates/actrium-core/src/browser_data.rs|state_conflict_reason|Rust browser storage sync owns bookmark delete/restore state reasons"
  "crates/actrium-core/src/browser_data.rs|state_conflicts|Rust browser storage sync owns bookmark delete/restore state conflict details"
  "crates/actrium-core/src/browser_data.rs|state_conflict_summary|Rust browser storage sync owns bookmark delete/restore state conflict summaries"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageBookmarkOwnershipPolicy|Rust browser storage sync owns bookmark ownership policy contract"
  "crates/actrium-core/src/browser_data.rs|browser_storage_bookmark_ownership_plan_json|Rust browser storage sync owns batched bookmark ownership planning"
  "crates/actrium-core/src/browser_data.rs|retention_reason|Rust browser storage sync owns bookmark ownership retention reason diagnostics"
  "crates/actrium-core/src/browser_data.rs|suppressed_bookmark_ids|Rust browser storage sync owns bookmark ownership suppressed source diagnostics"
  "crates/actrium-core/src/browser_data.rs|ownership_summary|Rust browser storage sync owns bookmark ownership summaries"
  "crates/actrium-core/src/browser_data.rs|BROWSER_STORAGE_HISTORY_VISIT_AGGREGATION_POLICY|Rust browser storage sync owns history visit aggregation policy"
  "crates/actrium-core/src/browser_data.rs|browser_storage_history_visit_aggregation_plan_json|Rust browser storage sync owns batched history visit aggregation planning"
  "crates/actrium-core/src/browser_data.rs|aggregation_action|Rust browser storage sync owns history visit aggregation diagnostics"
  "crates/actrium-core/src/browser_data.rs|aggregation_summary|Rust browser storage sync owns history visit aggregation summaries"
  "crates/actrium-core/src/browser_data.rs|BROWSER_STORAGE_SESSION_TAB_ORDERING_POLICY|Rust browser storage sync owns session tab ordering policy"
  "crates/actrium-core/src/browser_data.rs|browser_storage_session_tab_ordering_plan_json|Rust browser storage sync owns batched session tab ordering planning"
  "crates/actrium-core/src/browser_data.rs|ordering_action|Rust browser storage sync owns session tab ordering diagnostics"
  "crates/actrium-core/src/browser_data.rs|ordering_summary|Rust browser storage sync owns session tab ordering summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_session_tab_id_plan_json|Rust browser storage sync owns batched session tab id normalization"
  "crates/actrium-core/src/browser_data.rs|BROWSER_STORAGE_SESSION_ACTIVE_TAB_CONSISTENCY_POLICY|Rust browser storage sync owns session active-tab consistency policy"
  "crates/actrium-core/src/browser_data.rs|browser_storage_session_active_tab_consistency_plan_json|Rust browser storage sync owns batched session active-tab consistency planning"
  "crates/actrium-core/src/browser_data.rs|normalize_browser_storage_sync_batch_json|Rust browser storage sync normalizes batch payloads"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_batch_from_dirty_rows_json|Rust browser storage sync owns dirty-row batch conversion"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_push_request_from_dirty_rows_json|Rust browser storage sync owns dirty-row push conversion"
  "crates/actrium-core/src/browser_data.rs|normalize_browser_storage_sync_ack_batch_json|Rust browser storage sync normalizes ack payloads"
  "crates/actrium-core/src/browser_data.rs|normalize_browser_storage_sync_cursors_json|Rust browser storage sync normalizes cursor payloads"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_push_request_json|Rust browser storage sync owns push request payloads"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_pull_request_json|Rust browser storage sync owns pull request payloads"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cycle_response_json|Rust browser storage sync owns cycle response payloads"
  "crates/actrium-core/src/browser_data.rs|cursors: Vec<BrowserStorageSyncCursorUpdate>|Rust browser storage sync batches carry cursor handoff payloads"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_client_cycle_plan_json|Rust browser storage sync owns client cycle apply/ack planning"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_client_ack_batch_after_remote_apply_json|Rust browser storage sync owns post-remote safe ack selection"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_client_cycle_apply_result_json|Rust browser storage sync owns sync cycle apply result adjudication"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_transport_contract_json|Rust browser storage sync owns transport endpoint contract"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudStore|Rust browser storage sync owns cloud provider object-store trait"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_store_contract_json|Rust browser storage sync owns cloud provider object-store contract"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudProviderAdapterPlan|Rust browser storage sync owns cloud provider adapter object-space plan"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_provider_adapter_contract_json|Rust browser storage sync exports cloud provider adapter contract"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_provider_adapter_plan_json|Rust browser storage sync exports cloud provider adapter plan"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudProviderErrorClassificationPlan|Rust browser storage sync owns cloud provider error classification"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_provider_error_policy_json|Rust browser storage sync exports cloud provider error policy"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_provider_error_classification_json|Rust browser storage sync exports cloud provider error classifier"
  "crates/actrium-core/src/browser_data.rs|operation_boundary|Rust browser storage sync keeps provider adapter IO coarse-grained"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudDataModelContract|Rust browser storage sync owns cloud document data model"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_data_model_json|Rust browser storage sync exports cloud document data model"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_default_manifest_json|Rust browser storage sync owns cloud manifest defaults"
  "crates/actrium-core/src/browser_data.rs|normalize_browser_storage_sync_cloud_manifest_json|Rust browser storage sync owns cloud manifest normalization"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_log_object_json|Rust browser storage sync owns cloud log object building"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_snapshot_object_json|Rust browser storage sync owns cloud snapshot object building"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudPushPlan|Rust browser storage sync owns cloud push write plans"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudPushWriteSummary|Rust browser storage sync owns cloud push write summaries"
  "crates/actrium-core/src/browser_data.rs|write_summary|Rust browser storage sync embeds cloud push write summaries in push plans"
  "crates/actrium-core/src/browser_data.rs|cloud_push_plan_builds_ordered_provider_writes|Rust browser storage sync tests cloud push write summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_push_plan_json|Rust browser storage sync exports cloud push write plans"
  "crates/actrium-core/src/browser_data.rs|write_reason|Rust browser storage sync records cloud planned write reasons"
  "crates/actrium-core/src/browser_data.rs|write_reason: operation.write_reason.clone|Rust browser storage sync propagates planned write reasons into result issues"
  "crates/actrium-core/src/browser_data.rs|write_reason: Some|Rust browser storage sync propagates planned write reasons into compaction issues"
  "crates/actrium-core/src/browser_data.rs|delete_reason: Some|Rust browser storage sync propagates planned delete reasons into compaction issues"
  "crates/actrium-core/src/browser_data.rs|delete_reason: operation.delete_reason.clone|Rust browser storage sync propagates planned delete reasons into cleanup issues"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudPushResultPlan|Rust browser storage sync owns cloud push result plans"
  "crates/actrium-core/src/browser_data.rs|ack_reason|Rust browser storage sync owns cloud push ack reasons"
  "crates/actrium-core/src/browser_data.rs|allRequiredCloudWritesSucceeded|Rust browser storage sync records committed cloud-write ack reason"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_push_result_plan_json|Rust browser storage sync exports cloud push result plans"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudPullPlan|Rust browser storage sync owns cloud pull read plans"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudPullReadSummary|Rust browser storage sync owns cloud pull read summaries"
  "crates/actrium-core/src/browser_data.rs|read_summary|Rust browser storage sync embeds cloud pull read summaries in pull plans"
  "crates/actrium-core/src/browser_data.rs|cloud_pull_plan_lists_logs_and_snapshots_from_manifest_state|Rust browser storage sync tests cloud pull read summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_pull_plan_json|Rust browser storage sync exports cloud pull read plans"
  "crates/actrium-core/src/browser_data.rs|read_reason|Rust browser storage sync owns cloud planned read reasons"
  "crates/actrium-core/src/browser_data.rs|readListedCloudLogObject|Rust browser storage sync records listed log-object read reasons"
  "crates/actrium-core/src/browser_data.rs|read_reason: operation.read_reason.clone|Rust browser storage sync propagates planned read reasons into list results"
  "crates/actrium-core/src/browser_data.rs|read_reason: browser_storage_sync_cloud_planned_read_reason|Rust browser storage sync records direct read result reasons"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudPullApplyPlan|Rust browser storage sync owns cloud pull apply plans"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudPullApplySummary|Rust browser storage sync owns cloud pull apply summaries"
  "crates/actrium-core/src/browser_data.rs|apply_summary|Rust browser storage sync embeds cloud pull apply summaries in pull apply plans"
  "crates/actrium-core/src/browser_data.rs|cloud_pull_apply_plan_builds_remote_batch_from_provider_reads|Rust browser storage sync tests cloud pull apply summaries"
  "crates/actrium-core/src/browser_data.rs|apply_action|Rust browser storage sync owns cloud pull apply action diagnostics"
  "crates/actrium-core/src/browser_data.rs|remoteBatchContainsRecords|Rust browser storage sync records remote-apply record reason"
  "crates/actrium-core/src/browser_data.rs|onlySkippedCloudInputs|Rust browser storage sync records no-change skipped-input reason"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudPullApplySkippedSnapshot|Rust browser storage sync owns cloud pull apply skipped-snapshot diagnostics"
  "crates/actrium-core/src/browser_data.rs|skipSnapshotForIncrementalPull|Rust browser storage sync records incremental skipped-snapshot action"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudPullApplySkippedLog|Rust browser storage sync owns cloud pull apply skipped-log diagnostics"
  "crates/actrium-core/src/browser_data.rs|skipSelfOriginatedLog|Rust browser storage sync records same-device skipped-log action"
  "crates/actrium-core/src/browser_data.rs|skipRedundantLog|Rust browser storage sync records redundant skipped-log action"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_pull_apply_plan_json|Rust browser storage sync exports cloud pull apply plans"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudPullContinuationPlan|Rust browser storage sync owns cloud pull pagination continuation plans"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudPullContinuationSummary|Rust browser storage sync owns cloud pull continuation summaries"
  "crates/actrium-core/src/browser_data.rs|continuation_summary|Rust browser storage sync embeds cloud pull continuation summaries in continuation plans"
  "crates/actrium-core/src/browser_data.rs|cloud_pull_continuation_plan_preserves_provider_page_cursors|Rust browser storage sync tests cloud pull continuation summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_pull_continuation_plan_json|Rust browser storage sync exports cloud pull pagination continuation plans"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudCompactionPlan|Rust browser storage sync owns cloud compaction plans"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_compaction_plan_json|Rust browser storage sync exports cloud compaction plans"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudCompactionResultPlan|Rust browser storage sync owns cloud compaction result plans"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_compaction_result_plan_json|Rust browser storage sync exports cloud compaction result plans"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudCompactionRoundResult|Rust browser storage sync owns cloud compaction round results"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudCompactionRoundSummary|Rust browser storage sync owns cloud compaction round summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_compaction_round_summary|Rust browser storage sync builds cloud compaction round summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_run_compaction_round|Rust browser storage sync exports cloud compaction round runner"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_local_folder_compaction_round_json|Rust browser storage sync exports local-folder compaction round JSON"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudHighWatermarkPolicy|Rust browser storage sync owns cloud high-watermark policy"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudHighWatermarkPlan|Rust browser storage sync owns cloud high-watermark retention plans"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_high_watermark_plan_json|Rust browser storage sync exports cloud high-watermark retention plans"
  "crates/actrium-core/src/browser_data.rs|delete_reason|Rust browser storage sync owns cloud planned delete reasons"
  "crates/actrium-core/src/browser_data.rs|cloud_high_watermark_plan_marks_lagging_and_cleanup_candidates|Rust browser storage sync tests cloud high-watermark cleanup candidates"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudCleanupResultPlan|Rust browser storage sync owns cloud cleanup result plans"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_cleanup_result_plan_json|Rust browser storage sync exports cloud cleanup result plans"
  "crates/actrium-core/src/browser_data.rs|cloud_cleanup_result_plan_retries_transient_delete_failures|Rust browser storage sync tests cloud cleanup result retry planning"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudCleanupRoundResult|Rust browser storage sync owns cloud cleanup round results"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudCleanupRoundSummary|Rust browser storage sync owns cloud cleanup round summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_cleanup_round_summary|Rust browser storage sync builds cloud cleanup round summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_run_cleanup_round|Rust browser storage sync owns cloud cleanup round execution"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_local_folder_cleanup_round_json|Rust browser storage sync exports local-folder cleanup round JSON"
  "crates/actrium-core/src/browser_data.rs|cloud_local_folder_cleanup_round_json_deletes_retained_old_logs|Rust browser storage sync tests local-folder cleanup rounds against real files"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_recovery_retention_plan|Rust browser storage sync owns cloud recovery-snapshot retention planning"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudRecoveryRetentionResultPlan|Rust browser storage sync owns cloud recovery retention result plans"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_run_recovery_retention_round|Rust browser storage sync owns cloud recovery retention round execution"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudRecoveryRetentionRoundSummary|Rust browser storage sync owns cloud recovery retention round summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_local_folder_recovery_retention_round_json|Rust browser storage sync exports local-folder recovery retention round JSON"
  "crates/actrium-core/src/browser_data.rs|cloud_local_folder_recovery_retention_round_json_deletes_oldest_roots|Rust browser storage sync tests local-folder recovery retention rounds against real files"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_local_folder_recovery_retention_round_json|Rust browser storage sync exposes local-folder recovery retention round C ABI"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncMemoryCloudStore|Rust browser storage sync owns in-memory cloud store fixture"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncLocalFolderCloudStore|Rust browser storage sync owns local-folder cloud store fixture"
  "crates/actrium-core/src/browser_data.rs|cloud_local_folder_store_runs_push_round_against_real_files|Rust browser storage sync tests local-folder cloud push rounds against real files"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_execute_planned_writes|Rust browser storage sync owns cloud planned write execution"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_execute_planned_deletes|Rust browser storage sync owns cloud planned delete execution"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudReadOperationResult|Rust browser storage sync owns cloud read result classification"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudPushRoundResult|Rust browser storage sync owns cloud push round result classification"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudPushRoundSummary|Rust browser storage sync owns cloud push round summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_push_round_summary|Rust browser storage sync builds cloud push round summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_run_push_round|Rust browser storage sync owns cloud push round execution"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudExecutedReadOperations|Rust browser storage sync owns cloud planned read execution results"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudPullRoundResult|Rust browser storage sync owns cloud pull round result classification"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudPullRoundSummary|Rust browser storage sync owns cloud pull round summaries"
  "crates/actrium-core/src/browser_data.rs|round_summary|Rust browser storage sync embeds cloud pull round summaries in pull rounds"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_execute_planned_reads|Rust browser storage sync owns cloud planned read execution"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_execute_planned_reads_with_continuations|Rust browser storage sync owns paginated cloud planned read execution"
  "crates/actrium-core/src/browser_data.rs|BROWSER_STORAGE_SYNC_CLOUD_MAX_PULL_CONTINUATION_PAGES|Rust browser storage sync bounds pull continuation execution"
  "crates/actrium-core/src/browser_data.rs|paginationLimitExceeded|Rust browser storage sync diagnoses pull pagination safety limit"
  "crates/actrium-core/src/browser_data.rs|cloud_pull_round_follows_log_pagination_continuations|Rust browser storage sync tests paginated cloud pull execution"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_run_pull_round|Rust browser storage sync owns cloud pull round execution"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudCycleRoundResult|Rust browser storage sync owns cloud cycle round result classification"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudCycleRoundSummary|Rust browser storage sync owns cloud cycle round summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_cycle_round_summary|Rust browser storage sync builds cloud cycle round summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_run_cycle_round|Rust browser storage sync owns cloud cycle round execution"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudCycleClientPlan|Rust browser storage sync owns cloud cycle client planning"
  "crates/actrium-core/src/browser_data.rs|step_reason|Rust browser storage sync owns cloud cycle client step reasons"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_cycle_client_plan_json|Rust browser storage sync owns cloud cycle client plan JSON helper"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudCycleStatusPolicy|Rust browser storage sync owns cloud cycle status policy"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_cycle_status_policy_json|Rust browser storage sync owns cloud cycle status policy JSON helper"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudMembershipStatusPolicy|Rust browser storage sync owns cloud membership status policy"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_membership_status_policy_json|Rust browser storage sync owns cloud membership status policy JSON helper"
  "crates/actrium-core/src/browser_data.rs|cloud_membership_status_policy_classifies_membership_rounds|Rust browser storage sync tests cloud membership status policy"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudCycleScheduleState|Rust browser storage sync owns cloud cycle schedule state"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_cycle_schedule_state_json|Rust browser storage sync owns cloud cycle schedule state JSON helper"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudMembershipScheduleState|Rust browser storage sync owns cloud membership schedule state"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_membership_schedule_state_json|Rust browser storage sync owns cloud membership schedule state JSON helper"
  "crates/actrium-core/src/browser_data.rs|cloud_membership_schedule_state_derives_provider_native_timing_and_flags|Rust browser storage sync tests cloud membership schedule state"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudMembershipAttemptPlan|Rust browser storage sync owns cloud membership attempt planning"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_membership_attempt_plan_json|Rust browser storage sync owns cloud membership attempt plan JSON helper"
  "crates/actrium-core/src/browser_data.rs|cloud_membership_attempt_plan_composes_retry_and_recovery_paths|Rust browser storage sync tests cloud membership attempt plan"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudMembershipLoopPlan|Rust browser storage sync owns cloud membership loop planning"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudMembershipLoopSummary|Rust browser storage sync owns cloud membership loop summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_membership_loop_summary|Rust browser storage sync builds cloud membership loop summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_membership_loop_plan_json|Rust browser storage sync owns cloud membership loop plan JSON helper"
  "crates/actrium-core/src/browser_data.rs|cloud_membership_loop_plan_bounds_connect_revoke_rounds|Rust browser storage sync tests cloud membership loop plan"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudProviderDiagnostics|Rust browser storage sync owns cloud provider diagnostics"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_provider_diagnostics_json|Rust browser storage sync owns cloud provider diagnostics JSON helper"
  "crates/actrium-core/src/browser_data.rs|provider_error_classification_json|Rust cloud provider diagnostics accepts provider error classification"
  "crates/actrium-core/src/browser_data.rs|provider_error_classification|Rust cloud provider diagnostics exposes provider error classification source"
  "crates/actrium-core/src/browser_data.rs|cloud_provider_error_classification_prioritizes_recovery_and_retry|Rust browser storage sync tests cloud provider error classification priority"
  "crates/actrium-core/src/browser_data.rs|cloud_provider_diagnostics_accepts_provider_error_classification_source|Rust browser storage sync tests provider error diagnostics source mapping"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_provider_error_collect_items|Rust cloud provider error classifier extracts nested round/result provider failures"
  "crates/actrium-core/src/browser_data.rs|cloud_provider_error_classification_extracts_nested_round_failures|Rust browser storage sync tests nested provider round failure extraction"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudObservabilityEvent|Rust browser storage sync owns cloud observability events"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_observability_event_json|Rust browser storage sync owns cloud observability event JSON helper"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudCycleAttemptPlan|Rust browser storage sync owns cloud cycle attempt planning"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_cycle_attempt_plan_json|Rust browser storage sync owns cloud cycle attempt plan JSON helper"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudCycleLoopPlan|Rust browser storage sync owns cloud cycle loop planning"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudCycleLoopSummary|Rust browser storage sync owns cloud cycle loop summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_cycle_loop_summary|Rust browser storage sync builds cloud cycle loop summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_cycle_loop_plan_json|Rust browser storage sync owns cloud cycle loop plan JSON helper"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_local_folder_capabilities_json|Rust browser storage sync exports local-folder capabilities JSON helper"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_local_folder_capability_preflight_json|Rust browser storage sync exports local-folder capability preflight JSON helper"
  "crates/actrium-core/src/browser_data.rs|cloud_local_folder_capability_preflight_uses_rust_owned_capabilities|Rust browser storage sync tests Rust-owned local-folder capability preflight"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_run_compatibility_preflight|Rust browser storage sync owns generic cloud compatibility preflight execution"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_local_folder_compatibility_preflight_json|Rust browser storage sync exports local-folder compatibility preflight JSON helper"
  "crates/actrium-core/src/browser_data.rs|cloud_local_folder_compatibility_preflight_reads_real_files|Rust browser storage sync tests local-folder compatibility preflight against real files"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_local_folder_cycle_round_json|Rust browser storage sync exposes local-folder cloud cycle JSON helper"
  "crates/actrium-core/src/browser_data.rs|cloud_local_folder_cycle_round_json_runs_against_real_files|Rust browser storage sync tests local-folder cycle JSON against real files"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_local_folder_membership_round_json|Rust browser storage sync exposes local-folder cloud membership JSON helper"
  "crates/actrium-core/src/browser_data.rs|cloud_local_folder_membership_round_json_runs_against_real_files|Rust browser storage sync tests local-folder membership JSON against real files"
  "crates/actrium-core/src/browser_data.rs|compatibility_preflight: Option<BrowserStorageSyncCloudCompatibilityPreflightPlan>|Rust browser storage sync returns membership compatibility preflight"
  "crates/actrium-core/src/browser_data.rs|cloudCompatibilityPreflightUnavailable|Rust browser storage sync blocks membership when compatibility preflight cannot run"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudStoreCapabilityPreflightPlan|Rust browser storage sync owns cloud store capability preflight"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_store_capability_preflight_json|Rust browser storage sync exports cloud store capability preflight"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudCompatibilityStatus|Rust browser storage sync owns cloud compatibility status"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_compatibility_status_json|Rust browser storage sync exports cloud compatibility status"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudCompatibilityPreflightPlan|Rust browser storage sync owns batched cloud compatibility preflight"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_compatibility_preflight_json|Rust browser storage sync exports batched cloud compatibility preflight"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudSchemaRolloutPlan|Rust browser storage sync owns cloud schema rollout planning"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_schema_rollout_plan_json|Rust browser storage sync exports cloud schema rollout planning"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudSchemaMigrationPlaybook|Rust browser storage sync owns cloud schema migration playbook"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_schema_migration_playbook_json|Rust browser storage sync exports cloud schema migration playbook"
  "crates/actrium-core/src/browser_data.rs|browser_storage_schema_migration_plan_json|Rust browser storage owns the incremental schema migration plan DDL"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudRecoverySnapshotPlan|Rust browser storage sync owns cloud recovery snapshot planning"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_recovery_snapshot_plan_json|Rust browser storage sync exports cloud recovery snapshot planning"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudRecoverySnapshotResultPlan|Rust browser storage sync owns cloud recovery snapshot result handoff"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_recovery_snapshot_result_plan_json|Rust browser storage sync exports cloud recovery snapshot result handoff"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudRecoverySnapshotRoundResult|Rust browser storage sync owns cloud recovery snapshot round results"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudRecoverySnapshotRoundSummary|Rust browser storage sync owns cloud recovery snapshot round summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_recovery_snapshot_round_summary|Rust browser storage sync builds cloud recovery snapshot round summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_run_recovery_snapshot_round|Rust browser storage sync owns the cloud recovery snapshot round runner"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_local_folder_recovery_snapshot_round_json|Rust browser storage sync exports local-folder recovery snapshot rounds"
  "crates/actrium-core/src/browser_data.rs|recoverySnapshotRound|Rust browser storage sync emits recovery snapshot round plans"
  "crates/actrium-core/src/browser_data.rs|cloudRecoverySnapshotResult|Rust browser storage sync emits cloud recovery snapshot result plans"
  "crates/actrium-core/src/browser_data.rs|readRecoverySnapshotSource|Rust browser storage sync emits recovery snapshot source rereads"
  "crates/actrium-core/src/browser_data.rs|writeRecoveryCopy|Rust browser storage sync emits cloud recovery snapshot copy operations"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudMembershipPlan|Rust browser storage sync owns cloud device membership planning"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_membership_plan_json|Rust browser storage sync exports cloud device membership planning"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudMembershipResultPlan|Rust browser storage sync owns cloud device membership result planning"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_membership_result_plan_json|Rust browser storage sync exports cloud device membership result planning"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudMembershipRoundSummary|Rust browser storage sync owns cloud membership round summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_membership_round_summary|Rust browser storage sync builds cloud membership round summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cloud_run_membership_round|Rust browser storage sync owns cloud membership round execution"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_pull_continuation_plan_json|Rust UniFFI contract exposes browser sync cloud pull continuation plans"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_membership_status_policy_json|Rust UniFFI contract exposes browser sync cloud membership status policy"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_cycle_schedule_state_json|Rust UniFFI contract exposes browser sync cloud cycle schedule state"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_membership_schedule_state_json|Rust UniFFI contract exposes browser sync cloud membership schedule state"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_membership_attempt_plan_json|Rust UniFFI contract exposes browser sync cloud membership attempt plans"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_membership_loop_plan_json|Rust UniFFI contract exposes browser sync cloud membership loop plans"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_provider_diagnostics_json|Rust UniFFI contract exposes browser sync cloud provider diagnostics"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_observability_event_json|Rust UniFFI contract exposes browser sync cloud observability events"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_cycle_attempt_plan_json|Rust UniFFI contract exposes browser sync cloud cycle attempt plan"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_cycle_loop_plan_json|Rust UniFFI contract exposes browser sync cloud cycle loop plan"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_local_folder_capabilities_json|Rust UniFFI contract exposes browser sync local-folder capabilities"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_local_folder_capability_preflight_json|Rust UniFFI contract exposes browser sync local-folder capability preflight"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_local_folder_compatibility_preflight_json|Rust UniFFI contract exposes browser sync local-folder compatibility preflight"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_local_folder_cycle_round_json|Rust UniFFI contract exposes browser sync local-folder cycle rounds"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_local_folder_membership_round_json|Rust UniFFI contract exposes browser sync local-folder membership rounds"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_store_capability_preflight_json|Rust UniFFI contract exposes browser sync cloud store capability preflight"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_compatibility_status_json|Rust UniFFI contract exposes browser sync cloud compatibility status"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_compatibility_preflight_json|Rust UniFFI contract exposes browser sync cloud compatibility preflight"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_schema_rollout_plan_json|Rust UniFFI contract exposes browser sync cloud schema rollout planning"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_schema_migration_playbook_json|Rust UniFFI contract exposes browser sync cloud schema migration playbook"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_schema_migration_plan_json|Rust UniFFI contract exposes the browser storage schema migration plan"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_recovery_snapshot_plan_json|Rust UniFFI contract exposes browser sync cloud recovery snapshot planning"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_recovery_snapshot_result_plan_json|Rust UniFFI contract exposes browser sync cloud recovery snapshot result handoff"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_local_folder_recovery_snapshot_round_json|Rust UniFFI contract exposes browser sync local-folder recovery snapshot rounds"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_membership_plan_json|Rust UniFFI contract exposes browser sync cloud membership plans"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_membership_result_plan_json|Rust UniFFI contract exposes browser sync cloud membership result plans"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_high_watermark_policy_json|Rust UniFFI contract exposes browser sync cloud high-watermark policy"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_high_watermark_plan_json|Rust UniFFI contract exposes browser sync cloud high-watermark plans"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_cleanup_result_plan_json|Rust UniFFI contract exposes browser sync cloud cleanup result plans"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_local_folder_cleanup_round_json|Rust UniFFI contract exposes browser sync local-folder cleanup round"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_address_canonicalization_policy_json|Rust UniFFI contract exposes address canonicalization policy"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_canonicalize_addresses_json|Rust UniFFI contract exposes batched address canonicalization"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_bookmark_folder_tree_integrity_policy_json|Rust UniFFI contract exposes bookmark folder tree integrity policy"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_bookmark_folder_tree_integrity_plan_json|Rust UniFFI contract exposes bookmark folder tree integrity planning"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_bookmark_restore_placement_policy_json|Rust UniFFI contract exposes bookmark restore placement policy"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_bookmark_restore_placement_plan_json|Rust UniFFI contract exposes bookmark restore placement planning"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_bookmark_ownership_policy_json|Rust UniFFI contract exposes bookmark ownership policy"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_bookmark_ownership_plan_json|Rust UniFFI contract exposes bookmark ownership planning"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_history_visit_aggregation_policy_json|Rust UniFFI contract exposes history visit aggregation policy"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_history_visit_aggregation_plan_json|Rust UniFFI contract exposes history visit aggregation planning"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_session_tab_ordering_policy_json|Rust UniFFI contract exposes session tab ordering policy"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_session_tab_ordering_plan_json|Rust UniFFI contract exposes session tab ordering planning"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_session_tab_id_plan_json|Rust UniFFI contract exposes session tab id planning"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_session_active_tab_consistency_policy_json|Rust UniFFI contract exposes session active-tab consistency policy"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_session_active_tab_consistency_plan_json|Rust UniFFI contract exposes session active-tab consistency planning"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_retention_pruning_policy_json|Rust UniFFI contract exposes retention pruning policy"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_retention_pruning_plan_json|Rust UniFFI contract exposes retention pruning planning"
  "crates/actrium-core/src/browser_data.rs|BROWSER_STORAGE_SYNC_CLOUD_MANIFEST_PATH|Rust browser storage sync owns cloud manifest path constants"
  "crates/actrium-core/src/browser_data.rs|expected_revision|Rust browser storage sync cloud store uses optimistic provider revisions"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_provider_adapter_contract_json|Rust UniFFI contract exposes provider adapter contract"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_provider_adapter_plan_json|Rust UniFFI contract exposes provider adapter planning"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_provider_connection_status_policy_json|Rust UniFFI contract exposes provider connection status policy"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_provider_error_policy_json|Rust UniFFI contract exposes provider error policy"
  "crates/actrium-core/src/actrium_core.udl|browser_storage_sync_cloud_provider_error_classification_json|Rust UniFFI contract exposes provider error classification"
  "crates/actrium-core/src/actrium_core.udl|provider_error_classification_json|Rust UniFFI diagnostics contract accepts provider error classification"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_provider_adapter_contract_json|Rust C ABI exposes provider adapter contract"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_provider_adapter_plan_json|Rust C ABI exposes provider adapter plan"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_provider_adapter_contract_json|Rust C header exposes provider adapter contract"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_provider_adapter_plan_json|Rust C header exposes provider adapter plan"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncCloudProviderConnectionStatusPolicy|Rust browser storage sync owns provider connection status policy"
  "crates/actrium-core/src/browser_data.rs|should_attempt_membership|Rust provider connection policy can hand off cloud membership"
  "crates/actrium-core/src/browser_data.rs|No Actrium product server allocates provider account|Rust provider connection contract rejects service-side account allocation"
  "crates/actrium-core/src/ffi.rs|browser_storage_sync_cloud_provider_error_classification_json|Rust UniFFI wrapper exposes provider error classification"
  "crates/actrium-core/src/browser_data.rs|normalize_browser_sync_provider_settings|Rust owns the browser sync provider settings normalization contract"
  "crates/actrium-core/src/ffi.rs|normalize_browser_sync_provider_settings_json|Rust UniFFI wrapper exposes browser sync provider settings normalization"
  "crates/actrium-core/src/actrium_core.udl|normalize_browser_sync_provider_settings_json|Rust UniFFI contract exposes browser sync provider settings normalization"
  "crates/actrium-core/src/c_api.rs|actrium_core_normalize_browser_sync_provider_settings_json|Rust C ABI exposes browser sync provider settings normalization"
  "crates/actrium-core/include/actrium_core.h|actrium_core_normalize_browser_sync_provider_settings_json|Rust C header exposes browser sync provider settings normalization"
  "windows/src/ActriumCore.cs|actrium_core_normalize_browser_sync_provider_settings_json|Windows ActriumCore adapter exposes browser sync provider settings normalization C ABI"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_provider_connection_contract_json|Rust C ABI exposes provider connection contract"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_provider_connection_status_policy_json|Rust C ABI exposes provider connection status policy"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_provider_error_policy_json|Rust C ABI exposes provider error policy"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_provider_error_classification_json|Rust C ABI exposes provider error classification"
  "crates/actrium-core/src/c_api.rs|provider_error_classification_json|Rust C ABI diagnostics accepts provider error classification"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_store_capability_preflight_json|Rust C ABI exposes browser sync cloud store capability preflight"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_data_model_json|Rust C ABI exposes browser sync cloud data model"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_compatibility_status_json|Rust C ABI exposes browser sync cloud compatibility status"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_compatibility_preflight_json|Rust C ABI exposes browser sync cloud compatibility preflight"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_schema_rollout_plan_json|Rust C ABI exposes browser sync cloud schema rollout planning"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_schema_migration_playbook_json|Rust C ABI exposes browser sync cloud schema migration playbook"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_schema_migration_plan_json|Rust C ABI exposes the browser storage schema migration plan"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_recovery_snapshot_plan_json|Rust C ABI exposes browser sync cloud recovery snapshot planning"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_recovery_snapshot_result_plan_json|Rust C ABI exposes browser sync cloud recovery snapshot result handoff"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_local_folder_recovery_snapshot_round_json|Rust C ABI exposes browser sync local-folder recovery snapshot rounds"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_membership_plan_json|Rust C ABI exposes browser sync cloud membership plans"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_membership_result_plan_json|Rust C ABI exposes browser sync cloud membership result plans"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_push_plan_json|Rust C ABI exposes browser sync cloud push plans"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_push_result_plan_json|Rust C ABI exposes browser sync cloud push result plans"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_pull_plan_json|Rust C ABI exposes browser sync cloud pull plans"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_pull_apply_plan_json|Rust C ABI exposes browser sync cloud pull apply plans"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_pull_continuation_plan_json|Rust C ABI exposes browser sync cloud pull continuation plans"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_compaction_plan_json|Rust C ABI exposes browser sync cloud compaction plans"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_compaction_result_plan_json|Rust C ABI exposes browser sync cloud compaction result plans"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_local_folder_compaction_round_json|Rust C ABI exposes browser sync local-folder compaction round"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_high_watermark_policy_json|Rust C ABI exposes browser sync cloud high-watermark policy"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_high_watermark_plan_json|Rust C ABI exposes browser sync cloud high-watermark plans"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_cleanup_result_plan_json|Rust C ABI exposes browser sync cloud cleanup result plans"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_local_folder_cleanup_round_json|Rust C ABI exposes browser sync local-folder cleanup round"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_cycle_client_plan_json|Rust C ABI exposes browser sync cloud cycle client plans"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_cycle_status_policy_json|Rust C ABI exposes browser sync cloud cycle status policy"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_membership_status_policy_json|Rust C ABI exposes browser sync cloud membership status policy"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_cycle_schedule_state_json|Rust C ABI exposes browser sync cloud cycle schedule state"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_membership_schedule_state_json|Rust C ABI exposes browser sync cloud membership schedule state"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_membership_attempt_plan_json|Rust C ABI exposes browser sync cloud membership attempt plans"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_membership_loop_plan_json|Rust C ABI exposes browser sync cloud membership loop plans"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_provider_diagnostics_json|Rust C ABI exposes browser sync cloud provider diagnostics"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_observability_event_json|Rust C ABI exposes browser sync cloud observability events"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_cycle_attempt_plan_json|Rust C ABI exposes browser sync cloud cycle attempt plan"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_cycle_loop_plan_json|Rust C ABI exposes browser sync cloud cycle loop plan"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_local_folder_capabilities_json|Rust C ABI exposes browser sync local-folder capabilities"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_local_folder_capability_preflight_json|Rust C ABI exposes browser sync local-folder capability preflight"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_local_folder_compatibility_preflight_json|Rust C ABI exposes browser sync local-folder compatibility preflight"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_local_folder_cycle_round_json|Rust C ABI exposes browser sync local-folder cycle rounds"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_sync_cloud_local_folder_membership_round_json|Rust C ABI exposes browser sync local-folder membership rounds"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_address_canonicalization_policy_json|Rust C ABI exposes address canonicalization policy"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_canonicalize_addresses_json|Rust C ABI exposes batched address canonicalization"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_bookmark_folder_tree_integrity_policy_json|Rust C ABI exposes bookmark folder tree integrity policy"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_bookmark_folder_tree_integrity_plan_json|Rust C ABI exposes bookmark folder tree integrity planning"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_bookmark_restore_placement_policy_json|Rust C ABI exposes bookmark restore placement policy"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_bookmark_restore_placement_plan_json|Rust C ABI exposes bookmark restore placement planning"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_bookmark_ownership_policy_json|Rust C ABI exposes bookmark ownership policy"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_bookmark_ownership_plan_json|Rust C ABI exposes bookmark ownership planning"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_history_visit_aggregation_policy_json|Rust C ABI exposes history visit aggregation policy"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_history_visit_aggregation_plan_json|Rust C ABI exposes history visit aggregation planning"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_session_tab_ordering_policy_json|Rust C ABI exposes session tab ordering policy"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_session_tab_ordering_plan_json|Rust C ABI exposes session tab ordering planning"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_session_tab_id_plan_json|Rust C ABI exposes session tab id planning"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_session_active_tab_consistency_policy_json|Rust C ABI exposes session active-tab consistency policy"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_session_active_tab_consistency_plan_json|Rust C ABI exposes session active-tab consistency planning"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_retention_pruning_policy_json|Rust C ABI exposes retention pruning policy"
  "crates/actrium-core/src/c_api.rs|actrium_core_browser_storage_retention_pruning_plan_json|Rust C ABI exposes retention pruning planning"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_compatibility_status_json|Rust C header exposes browser sync cloud compatibility status"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_provider_connection_contract_json|Rust C header exposes provider connection contract"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_provider_connection_status_policy_json|Rust C header exposes provider connection status policy"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_provider_error_policy_json|Rust C header exposes provider error policy"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_provider_error_classification_json|Rust C header exposes provider error classification"
  "crates/actrium-core/include/actrium_core.h|provider_error_classification_json|Rust C header diagnostics accepts provider error classification"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_store_capability_preflight_json|Rust C header exposes browser sync cloud store capability preflight"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_compatibility_preflight_json|Rust C header exposes browser sync cloud compatibility preflight"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_schema_rollout_plan_json|Rust C header exposes browser sync cloud schema rollout planning"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_schema_migration_playbook_json|Rust C header exposes browser sync cloud schema migration playbook"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_schema_migration_plan_json|Rust C header exposes the browser storage schema migration plan"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_recovery_snapshot_plan_json|Rust C header exposes browser sync cloud recovery snapshot planning"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_recovery_snapshot_result_plan_json|Rust C header exposes browser sync cloud recovery snapshot result handoff"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_local_folder_recovery_snapshot_round_json|Rust C header exposes browser sync local-folder recovery snapshot rounds"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_membership_plan_json|Rust C header exposes browser sync cloud membership plans"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_membership_result_plan_json|Rust C header exposes browser sync cloud membership result plans"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_pull_continuation_plan_json|Rust C header exposes browser sync cloud pull continuation plans"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_high_watermark_policy_json|Rust C header exposes browser sync cloud high-watermark policy"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_high_watermark_plan_json|Rust C header exposes browser sync cloud high-watermark plans"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_cleanup_result_plan_json|Rust C header exposes browser sync cloud cleanup result plans"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_local_folder_cleanup_round_json|Rust C header exposes browser sync local-folder cleanup round"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_cycle_client_plan_json|Rust C header exposes browser sync cloud cycle client plans"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_cycle_status_policy_json|Rust C header exposes browser sync cloud cycle status policy"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_membership_status_policy_json|Rust C header exposes browser sync cloud membership status policy"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_cycle_schedule_state_json|Rust C header exposes browser sync cloud cycle schedule state"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_membership_schedule_state_json|Rust C header exposes browser sync cloud membership schedule state"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_membership_attempt_plan_json|Rust C header exposes browser sync cloud membership attempt plans"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_membership_loop_plan_json|Rust C header exposes browser sync cloud membership loop plans"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_provider_diagnostics_json|Rust C header exposes browser sync cloud provider diagnostics"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_observability_event_json|Rust C header exposes browser sync cloud observability events"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_cycle_attempt_plan_json|Rust C header exposes browser sync cloud cycle attempt plan"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_cycle_loop_plan_json|Rust C header exposes browser sync cloud cycle loop plan"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_local_folder_capabilities_json|Rust C header exposes browser sync local-folder capabilities"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_local_folder_capability_preflight_json|Rust C header exposes browser sync local-folder capability preflight"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_local_folder_compatibility_preflight_json|Rust C header exposes browser sync local-folder compatibility preflight"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_local_folder_cycle_round_json|Rust C header exposes browser sync local-folder cycle rounds"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_sync_cloud_local_folder_membership_round_json|Rust C header exposes browser sync local-folder membership rounds"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_address_canonicalization_policy_json|Rust C header exposes address canonicalization policy"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_canonicalize_addresses_json|Rust C header exposes batched address canonicalization"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_bookmark_folder_tree_integrity_policy_json|Rust C header exposes bookmark folder tree integrity policy"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_bookmark_folder_tree_integrity_plan_json|Rust C header exposes bookmark folder tree integrity planning"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_bookmark_restore_placement_policy_json|Rust C header exposes bookmark restore placement policy"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_bookmark_restore_placement_plan_json|Rust C header exposes bookmark restore placement planning"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_bookmark_ownership_policy_json|Rust C header exposes bookmark ownership policy"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_bookmark_ownership_plan_json|Rust C header exposes bookmark ownership planning"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_history_visit_aggregation_policy_json|Rust C header exposes history visit aggregation policy"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_history_visit_aggregation_plan_json|Rust C header exposes history visit aggregation planning"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_session_tab_ordering_policy_json|Rust C header exposes session tab ordering policy"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_session_tab_ordering_plan_json|Rust C header exposes session tab ordering planning"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_session_tab_id_plan_json|Rust C header exposes session tab id planning"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_session_active_tab_consistency_policy_json|Rust C header exposes session active-tab consistency policy"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_session_active_tab_consistency_plan_json|Rust C header exposes session active-tab consistency planning"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_retention_pruning_policy_json|Rust C header exposes retention pruning policy"
  "crates/actrium-core/include/actrium_core.h|actrium_core_browser_storage_retention_pruning_plan_json|Rust C header exposes retention pruning planning"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cycle_request_plan_json|Rust browser storage sync owns cycle request planning"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_cycle_transport_result_json|Rust browser storage sync owns transport result classification"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_status_policy_json|Rust browser storage sync owns coordinator status/retry policy"
  "crates/actrium-core/src/browser_data.rs|retryDelaySeconds|Rust browser storage sync status policy includes deterministic retry delay"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_schedule_state_json|Rust browser storage sync owns coordinator schedule state"
  "crates/actrium-core/src/browser_data.rs|nextRunAtUnixSeconds|Rust browser storage sync schedule state includes next run timing"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_scheduler_status_json|Rust browser storage sync owns product-visible scheduler status"
  "crates/actrium-core/src/browser_data.rs|merge_browser_storage_sync_cursors|Rust browser storage sync merges pull cursors into ack plans"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_ack_plan_json|Rust browser storage sync owns ack execution plans"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_remote_apply_plan_json|Rust browser storage sync owns remote apply execution plans"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_remote_apply_result_json|Rust browser storage sync owns remote apply result summaries"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_server_merge_plan_json|Rust browser storage sync owns reference merge execution plans"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_server_merge_result_json|Rust browser storage sync owns reference merge result summaries"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageSyncServerSql|Rust browser storage sync owns reference SQL contract"
  "crates/actrium-core/src/browser_data.rs|browser_sync_server_revisions|Rust browser storage sync owns reference revision allocation schema"
  "crates/actrium-core/src/browser_data.rs|allocate_server_revision|Rust browser storage sync owns reference revision allocation SQL"
  "crates/actrium-core/src/browser_data.rs|merge_pushed_history_page|Rust browser storage sync owns reference merge SQL"
  "crates/actrium-core/src/browser_data.rs|select_remote_history_pages|Rust browser storage sync owns reference remote pull SQL"
  "crates/actrium-core/src/browser_data.rs|server_revision_param_rule|Rust browser storage sync documents reference merge binding rule"
  "crates/actrium-sync/Cargo.toml|actrium-core|Rust browser sync reference executor depends on core contracts"
  "crates/actrium-sync/src/lib.rs|BrowserSyncServerDatabase|Rust browser sync reference executor has a concrete SQLite database boundary"
  "crates/actrium-sync/src/lib.rs|merge_push_request_json|Rust browser sync reference executor applies pushed batches"
  "crates/actrium-sync/src/lib.rs|pull_remote_batch_json|Rust browser sync reference executor builds pull batches"
  "crates/actrium-sync/src/lib.rs|sync_cycle_json|Rust browser sync reference executor supports push-pull cycles"
  "crates/actrium-sync/src/lib.rs|remoteBatch.*cursors|Rust browser sync reference tests require remote pull cursor handoff"
  "crates/actrium-sync/src/lib.rs|BrowserSyncService|Rust browser sync reference has an endpoint-level service boundary"
  "crates/actrium-sync/src/lib.rs|register_device|Rust browser sync reference service keeps local device bootstrap fixtures"
  "crates/actrium-sync/src/lib.rs|authorize_context|Rust browser sync reference service enforces device authorization"
  "crates/actrium-sync/src/lib.rs|max_push_records|Rust browser sync reference service enforces push record limits"
  "crates/actrium-sync/src/lib.rs|BrowserSyncHttpRequest|Rust browser sync reference service has an HTTP facade request type"
  "crates/actrium-sync/src/lib.rs|BrowserSyncHttpResponse|Rust browser sync reference service has an HTTP facade response type"
  "crates/actrium-sync/src/lib.rs|handle_http_request|Rust browser sync reference service maps HTTP requests to service methods"
  "crates/actrium-sync/src/lib.rs|BROWSER_STORAGE_SYNC_TRANSPORT_CYCLE_PATH|Rust browser sync reference HTTP facade uses core endpoint constants"
  "crates/actrium-sync/src/lib.rs|http_facade_routes_contract_push_cycle_pull_and_cursors|Rust browser sync reference HTTP facade tests cover happy paths"
  "crates/actrium-sync/src/lib.rs|http_facade_maps_auth_validation_and_routing_errors|Rust browser sync reference HTTP facade tests cover error mapping"
  "crates/actrium-sync/Cargo.toml|tiny_http|Rust browser sync reference has a minimal HTTP runtime dependency"
  "crates/actrium-sync/src/bin/actrium-sync-server.rs|Server::http|Rust browser sync reference binary starts tiny_http socket runtime"
  "crates/actrium-sync/src/bin/actrium-sync-server.rs|handle_http_request|Rust browser sync reference binary delegates protocol handling to service facade"
  "crates/actrium-sync/src/bin/actrium-sync-server.rs|register-device|Rust browser sync reference binary supports local device bootstrap"
  "crates/actrium-sync/src/bin/actrium-sync-server.rs|parses_server_options|Rust browser sync reference binary tests CLI parsing"
  "package.json|cargo test -p actrium-core -p actrium-vault -p actrium-sync-server|Rust verification includes sync reference executor"
  "crates/actrium-core/src/browser_data.rs|BrowserStorageDataModelContract|Rust browser storage owns a data model contract"
  "crates/actrium-core/src/browser_data.rs|browser_storage_data_model_json|Rust browser storage exports the data model contract as JSON"
  "crates/actrium-core/src/browser_data.rs|local_primary_key_columns|Rust browser storage data model records local primary keys"
  "crates/actrium-core/src/browser_data.rs|stable_identity_fields|Rust browser storage data model records stable sync identities"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_conflict_policy_json|Rust browser storage sync owns conflict policy contract"
  "crates/actrium-core/src/browser_data.rs|browser_storage_sync_merge_policy_json|Rust browser storage sync owns collection merge policy contract"
  "crates/actrium-core/src/browser_data.rs|browser_storage_address_canonicalization_policy_json|Rust browser storage sync owns address canonicalization policy contract"
  "crates/actrium-core/src/browser_data.rs|browser_storage_retention_pruning_policy_json|Rust browser storage sync owns retention pruning policy contract"
  "crates/actrium-core/src/browser_data.rs|browser_storage_retention_pruning_plan_json|Rust browser storage sync owns batched retention pruning planning"
  "crates/actrium-core/src/browser_data.rs|consistency_reason|Rust browser storage sync owns session active-tab consistency diagnostics"
  "crates/actrium-core/src/browser_data.rs|consistency_summary|Rust browser storage sync owns session active-tab consistency summaries"
  "crates/actrium-core/src/browser_data.rs|retention_reason|Rust browser storage sync owns retention pruning row diagnostics"
  "crates/actrium-core/src/browser_data.rs|retention_summary|Rust browser storage sync owns retention pruning summaries"
  "crates/actrium-core/src/browser_data.rs|originDeviceHistoryPageLimitExceeded|Rust browser storage sync owns per-origin-device history pruning"
  "crates/actrium-core/src/browser_data.rs|originDeviceSessionTabLimitExceeded|Rust browser storage sync owns per-origin-device session-tab pruning"
  "crates/actrium-core/src/browser_data.rs|BROWSER_STORAGE_SYNC_REMOTE_APPLY_CONFLICT_POLICY|Rust browser storage sync names remote apply conflict policy"
  "crates/actrium-core/src/browser_data.rs|BROWSER_STORAGE_SYNC_REMOTE_APPLY_CONFLICT_REASON|Rust browser storage sync names remote apply conflict reason"
  "crates/actrium-core/src/browser_data.rs|BROWSER_STORAGE_SYNC_MERGE_POLICY_SESSION_TABS|Rust browser storage sync names session-tab merge policy"
  "crates/actrium-core/src/browser_data.rs|apply_remote_history_page|Rust browser storage sync owns remote apply SQL"
  "crates/actrium-core/src/browser_data.rs|mark_history_page_synced|Rust browser storage sync owns ack SQL"
  "crates/actrium-core/src/browser_data.rs|sync_revision = MAX\\(sync_revision, \\?\\)|Rust browser storage ack SQL writes accepted sync revisions"
  "crates/actrium-core/src/browser_data.rs|upsert_sync_cursor|Rust browser storage sync owns cursor ack SQL"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|\"syncRevision\": 999|Apple browser storage tests verify ack revision writeback"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|\\.put\\(\"syncRevision\", 999\\)|Android browser storage tests verify ack revision writeback"
  "windows/tests/Program.cs|\"syncRevision\": 999|Windows browser storage tests verify ack revision writeback"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|normalizeBrowserStorageSyncBatchJson|Apple ActriumCore binding exposes browser sync batch normalization"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncBatchFromDirtyRowsJson|Apple ActriumCore binding exposes dirty-row batch conversion"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncPushRequestFromDirtyRowsJson|Apple ActriumCore binding exposes dirty-row push conversion"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|normalizeBrowserStorageSyncAckBatchJson|Apple ActriumCore binding exposes browser sync ack normalization"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|normalizeBrowserStorageSyncCursorsJson|Apple ActriumCore binding exposes browser sync cursor normalization"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncPushRequestJson|Apple ActriumCore binding exposes browser sync push request building"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncPullRequestJson|Apple ActriumCore binding exposes browser sync pull request building"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCycleResponseJson|Apple ActriumCore binding exposes browser sync cycle response building"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|normalizeBrowserStorageSyncCycleResponseJson|Apple ActriumCore binding exposes browser sync cycle response normalization"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncClientCyclePlanJson|Apple ActriumCore binding exposes browser sync client cycle planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncClientAckBatchAfterRemoteApplyJson|Apple ActriumCore binding exposes browser sync post-remote ack selection"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncTransportContractJson|Apple ActriumCore binding exposes browser sync transport contract"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudStoreContractJson|Apple ActriumCore binding exposes browser sync cloud store contract"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudProviderAdapterContractJson|Apple ActriumCore binding exposes browser sync cloud provider adapter contract"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudProviderAdapterPlanJson|Apple ActriumCore binding exposes browser sync cloud provider adapter plan"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudProviderErrorPolicyJson|Apple ActriumCore binding exposes browser sync cloud provider error policy"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudProviderErrorClassificationJson|Apple ActriumCore binding exposes browser sync cloud provider error classification"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|providerErrorClassificationJson|Apple ActriumCore diagnostics binding accepts provider error classification"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudStoreCapabilityPreflightJson|Apple ActriumCore binding exposes browser sync cloud store capability preflight"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudLocalFolderCapabilitiesJson|Apple ActriumCore binding exposes browser sync local-folder capabilities"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudLocalFolderCapabilityPreflightJson|Apple ActriumCore binding exposes browser sync local-folder capability preflight"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudLocalFolderCompatibilityPreflightJson|Apple ActriumCore binding exposes browser sync local-folder compatibility preflight"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudDataModelJson|Apple ActriumCore binding exposes browser sync cloud data model"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudCompatibilityStatusJson|Apple ActriumCore binding exposes browser sync cloud compatibility status"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudCompatibilityPreflightJson|Apple ActriumCore binding exposes browser sync cloud compatibility preflight"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudSchemaRolloutPlanJson|Apple ActriumCore binding exposes browser sync cloud schema rollout planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudSchemaMigrationPlaybookJson|Apple ActriumCore binding exposes browser sync cloud schema migration playbook"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSchemaMigrationPlanJson|Apple ActriumCore binding exposes the browser storage schema migration plan"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageDatabase.swift|browserStorageSchemaMigrationPlanJson|Apple browser storage adapter consumes the Rust schema migration plan"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudRecoverySnapshotPlanJson|Apple ActriumCore binding exposes browser sync cloud recovery snapshot planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudRecoverySnapshotResultPlanJson|Apple ActriumCore binding exposes browser sync cloud recovery snapshot result handoff"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudLocalFolderRecoverySnapshotRoundJson|Apple ActriumCore binding exposes browser sync local-folder recovery snapshot rounds"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudDefaultManifestJson|Apple ActriumCore binding exposes browser sync cloud manifest defaults"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudLogObjectJson|Apple ActriumCore binding exposes browser sync cloud log objects"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudSnapshotObjectJson|Apple ActriumCore binding exposes browser sync cloud snapshot objects"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudMembershipPlanJson|Apple ActriumCore binding exposes browser sync cloud membership plans"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudMembershipResultPlanJson|Apple ActriumCore binding exposes browser sync cloud membership result plans"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudPushPlanJson|Apple ActriumCore binding exposes browser sync cloud push planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudPushResultPlanJson|Apple ActriumCore binding exposes browser sync cloud push result planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudPullPlanJson|Apple ActriumCore binding exposes browser sync cloud pull planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudPullApplyPlanJson|Apple ActriumCore binding exposes browser sync cloud pull apply planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudPullContinuationPlanJson|Apple ActriumCore binding exposes browser sync cloud pull continuation planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudCompactionPlanJson|Apple ActriumCore binding exposes browser sync cloud compaction planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudCompactionResultPlanJson|Apple ActriumCore binding exposes browser sync cloud compaction result planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudLocalFolderCompactionRoundJson|Apple ActriumCore binding exposes browser sync local-folder compaction round"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudLocalFolderCleanupRoundJson|Apple ActriumCore binding exposes browser sync local-folder cleanup round"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudHighWatermarkPolicyJson|Apple ActriumCore binding exposes browser sync cloud high-watermark policy"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudHighWatermarkPlanJson|Apple ActriumCore binding exposes browser sync cloud high-watermark planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudCleanupResultPlanJson|Apple ActriumCore binding exposes browser sync cloud cleanup result planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudCycleClientPlanJson|Apple ActriumCore binding exposes browser sync cloud cycle client planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudCycleStatusPolicyJson|Apple ActriumCore binding exposes browser sync cloud cycle status policy"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudMembershipStatusPolicyJson|Apple ActriumCore binding exposes browser sync cloud membership status policy"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudCycleScheduleStateJson|Apple ActriumCore binding exposes browser sync cloud cycle schedule state"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudMembershipScheduleStateJson|Apple ActriumCore binding exposes browser sync cloud membership schedule state"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudMembershipAttemptPlanJson|Apple ActriumCore binding exposes browser sync cloud membership attempt plan"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudMembershipLoopPlanJson|Apple ActriumCore binding exposes browser sync cloud membership loop plan"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudProviderDiagnosticsJson|Apple ActriumCore binding exposes browser sync cloud provider diagnostics"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudObservabilityEventJson|Apple ActriumCore binding exposes browser sync cloud observability events"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudCycleAttemptPlanJson|Apple ActriumCore binding exposes browser sync cloud cycle attempt plan"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudCycleLoopPlanJson|Apple ActriumCore binding exposes browser sync cloud cycle loop plan"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudLocalFolderCycleRoundJson|Apple ActriumCore binding exposes browser sync local-folder cycle rounds"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCloudLocalFolderMembershipRoundJson|Apple ActriumCore binding exposes browser sync local-folder membership rounds"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCycleRequestPlanJson|Apple ActriumCore binding exposes browser sync cycle request planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncCycleTransportResultJson|Apple ActriumCore binding exposes browser sync transport result classification"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncStatusPolicyJson|Apple ActriumCore binding exposes browser sync status policy"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncScheduleStateJson|Apple ActriumCore binding exposes browser sync schedule state"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageDataModelJson|Apple ActriumCore binding exposes browser storage data model"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncAckPlanJson|Apple ActriumCore binding exposes browser sync ack planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncRemoteApplyPlanJson|Apple ActriumCore binding exposes browser sync remote apply planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncRemoteApplyResultJson|Apple ActriumCore binding exposes browser sync remote apply result summaries"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncServerMergePlanJson|Apple ActriumCore binding exposes browser sync reference merge planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncServerMergeResultJson|Apple ActriumCore binding exposes browser sync reference merge result summaries"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncConflictPolicyJson|Apple ActriumCore binding exposes browser sync conflict policy"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSyncMergePolicyJson|Apple ActriumCore binding exposes browser sync merge policy"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageAddressCanonicalizationPolicyJson|Apple ActriumCore binding exposes address canonicalization policy"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageCanonicalizeAddressesJson|Apple ActriumCore binding exposes batched address canonicalization"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageBookmarkFolderTreeIntegrityPolicyJson|Apple ActriumCore binding exposes bookmark folder tree integrity policy"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageBookmarkFolderTreeIntegrityPlanJson|Apple ActriumCore binding exposes bookmark folder tree integrity planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageBookmarkRestorePlacementPolicyJson|Apple ActriumCore binding exposes bookmark restore placement policy"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageBookmarkRestorePlacementPlanJson|Apple ActriumCore binding exposes bookmark restore placement planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageBookmarkOwnershipPolicyJson|Apple ActriumCore binding exposes bookmark ownership policy"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageBookmarkOwnershipPlanJson|Apple ActriumCore binding exposes bookmark ownership planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageHistoryVisitAggregationPolicyJson|Apple ActriumCore binding exposes history visit aggregation policy"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageHistoryVisitAggregationPlanJson|Apple ActriumCore binding exposes history visit aggregation planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSessionTabOrderingPolicyJson|Apple ActriumCore binding exposes session tab ordering policy"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSessionTabOrderingPlanJson|Apple ActriumCore binding exposes session tab ordering planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSessionActiveTabConsistencyPolicyJson|Apple ActriumCore binding exposes session active-tab consistency policy"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageSessionActiveTabConsistencyPlanJson|Apple ActriumCore binding exposes session active-tab consistency planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageRetentionPruningPolicyJson|Apple ActriumCore binding exposes retention pruning policy"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/actrium_core.swift|browserStorageRetentionPruningPlanJson|Apple ActriumCore binding exposes retention pruning planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleSQLiteDatabase.swift|executeReturningChanges|Apple SQLite adapter reports rows affected for browser sync conflicts"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageDatabase.swift|applyRemoteSyncPlan|Apple browser storage adapter executes Rust browser sync remote apply plans"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageDatabase.swift|browserStorageSessionTabIdPlanJson|Apple browser storage adapter derives session tab ids through the Rust batch plan"
  "android/android/app/src/main/java/io/actrium/app/data/database/BrowserStorageDatabase.kt|browserStorageSessionTabIdPlanJson|Android browser storage adapter derives session tab ids through the Rust batch plan"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageDatabase.swift|browserSessionSnapshotNormalizeJson|Apple browser storage adapter persists sessions through the Rust snapshot normalizer"
  "android/android/app/src/main/java/io/actrium/app/data/database/BrowserStorageDatabase.kt|browserSessionSnapshotNormalizeJson|Android browser storage adapter persists sessions through the Rust snapshot normalizer"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|testSaveSessionAdoptsCoreNormalizedSnapshot|Apple browser storage tests cover Rust-normalized session snapshot persistence"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|saveSessionAdoptsCoreNormalizedSnapshot|Android instrumentation tests cover Rust-normalized session snapshot persistence"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageDatabase.swift|applySyncAckPlan|Apple browser storage adapter executes Rust browser sync ack plans"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageDatabase.swift|applySyncCycleResponse|Apple browser storage adapter applies Rust-planned sync cycles"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageDatabase.swift|exportDirtySyncPushRequest|Apple browser storage adapter exports dirty rows through Rust push builder"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageDatabase.swift|exportSyncCycleRequestPlan|Apple browser storage adapter exports Rust-planned sync cycle requests"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageDatabase.swift|applyCloudCycleClientPlan|Apple browser storage adapter applies Rust cloud cycle client plans"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageDatabase.swift|owner_kind|Apple browser storage adapter migrates bookmark owner kind"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageDatabase.swift|ownerKind|Apple browser storage adapter maps bookmark owner kind"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageDatabase.swift|surface_kind|Apple browser storage adapter migrates session surface kind"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageDatabase.swift|surfaceKind|Apple browser storage adapter maps session surface kind"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|AppleBrowserStorageSyncTransport|Apple browser sync client uses injectable transport"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|browserStorageSyncCycleTransportResultJson|Apple browser sync client classifies transport through Rust"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|runLocalFolderSyncCycle|Apple browser sync client exposes local-folder cloud cycle execution"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|runLocalFolderMembershipRound|Apple browser sync client exposes local-folder cloud membership execution"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|browserStorageSyncCloudLocalFolderCapabilityPreflightJson|Apple browser sync client consumes Rust local-folder capability preflight"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|browserStorageSyncCloudLocalFolderCompatibilityPreflightJson|Apple browser sync client consumes Rust local-folder compatibility preflight"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|compatibilityPreflight|Apple browser sync client returns Rust local-folder compatibility preflight"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|browserStorageSyncCloudLocalFolderCycleRoundJson|Apple browser sync client consumes Rust local-folder cycle helper"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|browserStorageSyncCloudLocalFolderCompactionRoundJson|Apple browser sync client consumes Rust local-folder compaction helper"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|browserStorageSyncCloudLocalFolderCleanupRoundJson|Apple browser sync client consumes Rust local-folder cleanup helper"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|appleBrowserStorageCloudSchemaRolloutPlan|Apple browser sync client consumes Rust schema rollout planning on compatibility block"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|appleBrowserStorageCloudSchemaMigrationPlaybook|Apple browser sync client consumes Rust schema migration playbook on compatibility block"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|appleBrowserStorageLocalFolderRecoverySnapshotRound|Apple browser sync client consumes Rust local-folder recovery snapshot round on compatibility block"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|browserStorageSyncCloudLocalFolderMembershipRoundJson|Apple browser sync client consumes Rust local-folder membership helper"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|appleBrowserStorageCloudMembershipStatusPolicy|Apple browser sync membership client consumes Rust membership status policy"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|\"statusPolicy\": statusPolicy|Apple browser sync membership client returns Rust membership status policy"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|appleBrowserStorageCloudMembershipScheduleState|Apple browser sync membership client consumes Rust membership schedule state"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|\"scheduleState\": scheduleState|Apple browser sync membership client returns Rust membership schedule state"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|appleBrowserStorageCloudMembershipAttemptPlan|Apple browser sync membership client consumes Rust membership attempt plan"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|\"attemptPlan\": attemptPlan|Apple browser sync membership client returns Rust membership attempt plan"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|appleBrowserStorageCloudMembershipLoopPlan|Apple browser sync membership client consumes Rust membership loop plan"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|\"loopPlan\": loopPlan|Apple browser sync membership client returns Rust membership loop plan"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|let schemaRolloutPlan: [String: Any]?|Apple browser sync membership client prepares Rust schema rollout on compatibility block"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|membershipRound[\"compatibilityPreflight\"]|Apple browser sync client lifts Rust membership compatibility preflight"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|observabilityEvent: observabilityEvent|Apple browser sync membership client exposes Rust observability events"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|browserStorageSyncCloudCycleScheduleStateJson|Apple browser sync client consumes Rust cloud cycle schedule state"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|browserStorageSyncCloudProviderDiagnosticsJson|Apple browser sync client consumes Rust cloud provider diagnostics"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|providerErrorClassification|Apple browser sync client passes provider error diagnostics slot"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|appleBrowserStorageLocalFolderProviderErrorClassification|Apple browser sync client builds provider error classification from real local-folder rounds"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|localFolderProviderRoundErrors|Apple browser sync client sources provider errors from local-folder rounds"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|browserStorageSyncCloudObservabilityEventJson|Apple browser sync client consumes Rust cloud observability events"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|browserStorageSyncCloudCycleAttemptPlanJson|Apple browser sync client consumes Rust cloud cycle attempt planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift|browserStorageSyncCloudCycleLoopPlanJson|Apple browser sync client consumes Rust cloud cycle loop planning"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncCoordinator.swift|AppleBrowserStorageSyncCredentialsProvider|Apple browser sync coordinator uses injectable credential lookup"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncCoordinator.swift|AppleBrowserStorageSyncClient|Apple browser sync coordinator delegates network/apply work to the sync client"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncCoordinator.swift|statusPolicy|Apple browser sync coordinator returns Rust-owned status policy payloads"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncCoordinator.swift|scheduleState|Apple browser sync coordinator returns Rust-owned schedule state payloads"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncScheduler.swift|AppleBrowserStorageSyncSchedulerSnapshot|Apple browser sync scheduler exposes platform schedule snapshots"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncScheduler.swift|nextRunDelaySeconds|Apple browser sync scheduler consumes Rust-owned schedule state timing"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncScheduler.swift|browserStorageSyncSchedulerStatusJson|Apple browser sync scheduler delegates product status to Rust"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncScheduler.swift|scheduleAfterLocalChange|Apple browser sync scheduler can debounce local browser data changes"
  "ios/Actrium/Database/BrowserSyncCredentialsProvider.swift|keychainSessionAccount|iOS browser sync credentials come only from the stored Keychain sync session"
  "ios/Actrium/Database/BrowserSyncCredentialsProvider.swift|AppleBrowserStorageSyncCredentialsProvider|iOS browser sync credentials implement the Apple coordinator provider"
  "ios/Actrium/Database/BrowserDataRepository.swift|syncScheduler|iOS browser data repository wires the sync scheduler"
  "ios/Actrium/Database/BrowserDataRepository.swift|syncStatusJson|iOS browser data repository exposes Rust-owned sync status"
  "ios/Actrium/Database/BrowserDataRepository.swift|scheduleSyncAfterLocalChange|iOS browser data repository schedules local-change sync"
  "ios/Actrium/App/BrowserDataModule.swift|BrowserDataModule|iOS exposes browser data through a full native module"
  "ios/Actrium/App/BrowserDataModule.swift|loadRecentHistory|iOS BrowserDataModule exposes browser history reads"
  "ios/Actrium/App/BrowserDataModule.swift|getBookmarks|iOS BrowserDataModule exposes browser bookmark reads"
  "ios/Actrium/Database/BrowserDataRepository.swift|loadBookmarkPayloads|iOS browser data repository maps bookmark payloads behind the repository boundary"
  "ios/Actrium/Database/BrowserDataRepository.swift|ownerKind: \"tab\"|iOS tab-derived bookmarks are marked with tab ownership"
  "ios/Actrium/Database/BrowserDataRepository.swift|ownerId: tab.id.uuidString|iOS tab-derived bookmarks carry stable tab owner ids"
  "ios/Actrium/App/BrowserDataModule.swift|saveSyncSession|iOS BrowserDataModule exposes sync credential writes"
  "ios/Actrium/App/BrowserDataModule.swift|syncStatusJson|iOS BrowserDataModule exposes Rust-owned sync status"
  "ios/Actrium/App/BrowserDataModule.m|RCT_EXTERN_MODULE\\(BrowserDataModule|iOS BrowserDataModule is registered with React Native"
  "macos/Actrium/Settings/Sync/BrowserSyncCredentialsProvider.swift|save\\(baseUrl|macOS browser sync credentials have a local write surface for future account UI"
  "macos/Actrium/Settings/Sync/BrowserSyncCredentialsProvider.swift|saveSession|macOS browser sync credentials persist account sync sessions"
  "macos/Actrium/Settings/Sync/BrowserSyncCredentialsProvider.swift|legacyDeviceIdKey|macOS browser sync credentials migrate legacy device identity"
  "macos/Actrium/Settings/Sync/BrowserSyncCredentialsProvider.swift|AppleBrowserStorageSyncCredentialsProvider|macOS browser sync credentials implement the Apple coordinator provider"
  "macos/Actrium/Database/BrowserDataRepository.swift|syncScheduler|macOS browser data repository wires the sync scheduler"
  "macos/Actrium/Database/BrowserDataRepository.swift|syncStatusJson|macOS browser data repository exposes Rust-owned sync status"
  "macos/Actrium/Database/BrowserDataRepository.swift|scheduleSyncAfterLocalChange|macOS browser data repository schedules local-change sync"
  "macos/Actrium/Database/BrowserDataRepository.swift|scheduleSyncAfterCredentialChange|macOS browser data repository schedules credential-change sync"
  "macos/Actrium/Settings/SyncSettingsView.swift|BrowserDataSyncSettingsModel|macOS sync settings shows browser sync status and manual trigger"
  "macos/Actrium/Settings/SyncSettingsView.swift|Save Connection|macOS sync settings exposes browser sync credential controls"
  "packages/actrium-platform/apple/ActriumCore/Package.swift|ActriumCoreTests|Apple ActriumCore package wires browser storage migration tests"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|testMigratesBrowserStorageSQLiteV1V2V3ToV6|Apple tests cover browser storage v1/v2/v3 SQLite migration"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|testCloudProviderAdapterObjectSpacePlanContract|Apple tests cover provider adapter object-space plan"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|testCloudProviderConnectionStatusPolicyContract|Apple tests cover provider connection status policy"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|testCloudProviderErrorClassificationPolicyContract|Apple tests cover provider error classification"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|providerErrorClassification|Apple tests cover provider error diagnostics source mapping"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|batchHasBookmarkOwner|Apple tests cover browser bookmark owner sync payloads"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|exportDirtySyncPushRequest|Apple browser storage migration tests cover dirty push export"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|applySyncCycleResponse|Apple browser storage migration tests cover sync cycle apply"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|exportSyncCycleRequestPlan|Apple browser storage migration tests cover sync cycle request planning"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|testSyncClientExecutesInjectedCycleTransportAndAppliesResponse|Apple tests cover browser sync client cycle"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|testSyncClientRunsLocalFolderCloudCycleAndAppliesClientPlan|Apple tests cover local-folder cloud sync client"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|testSyncClientRunsLocalFolderCloudMembershipRound|Apple tests cover local-folder cloud membership client"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|finishCloudDeviceRevoked|Apple local-folder cloud membership test asserts Rust revoke classification"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|capabilityPreflight|Apple local-folder cloud sync test asserts Rust capability preflight"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|compatibilityPreflight|Apple local-folder cloud sync test asserts Rust compatibility preflight"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|clientUpgradeRequired|Apple local-folder cloud sync test asserts compatibility blocking"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|cloudCycleScheduleState|Apple local-folder cloud sync test asserts Rust schedule state"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|providerDiagnostics|Apple local-folder cloud sync test asserts Rust provider diagnostics"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|cloudObservabilityEvent|Apple local-folder cloud sync test asserts Rust observability event"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|loopPlan|Apple local-folder cloud sync test asserts Rust loop planning"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|compactionRound|Apple local-folder cloud sync test asserts Rust compaction round"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|cleanupRound|Apple local-folder cloud sync test asserts Rust cleanup round"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|recoverySnapshotRound|Apple local-folder cloud sync test asserts Rust recovery snapshot round"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|providerDiagnostics.*compatibilityClassification|Apple local-folder cloud membership test asserts Rust provider diagnostics"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|observabilityEvent.*planType|Apple local-folder cloud membership test asserts Rust observability event"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|cloudMembershipStatusPolicy|Apple local-folder cloud membership test asserts Rust membership status policy"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|cloudMembershipScheduleState|Apple local-folder cloud membership test asserts Rust membership schedule state"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|finishMembershipAttemptCount|Apple local-folder cloud membership test asserts Rust membership loop summary"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|cloudMembershipAttemptPlan|Apple local-folder cloud membership test asserts Rust membership attempt plan"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|cloudMembershipLoopPlan|Apple local-folder cloud membership test asserts Rust membership loop plan"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|blockedMembershipRollout|Apple local-folder cloud membership test asserts Rust schema rollout on compatibility block"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|testSyncCoordinatorSkipsCycleWhenCredentialsUnavailable|Apple tests cover browser sync credential gating"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|testSyncCoordinatorRunsClientWithAvailableCredentials|Apple tests cover browser sync coordinator delegation"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|waitForCredentials|Apple tests cover sync coordinator status policy"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|testSyncCoordinatorSchedulesRetryFromStatusPolicy|Apple tests cover sync coordinator schedule state"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|testSyncSchedulerConsumesScheduleStateAndResetsAfterSuccess|Apple tests cover sync scheduler retry lifecycle"
  "packages/actrium-platform/apple/ActriumCore/Tests/ActriumCoreTests/AppleBrowserStorageDatabaseTests.swift|retryScheduled|Apple tests cover Rust-owned sync scheduler status"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|normalizeBrowserStorageSyncBatchJson|Android ActriumRust facade exposes browser sync batch normalization"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncBatchFromDirtyRowsJson|Android ActriumRust facade exposes dirty-row batch conversion"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncPushRequestFromDirtyRowsJson|Android ActriumRust facade exposes dirty-row push conversion"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|normalizeBrowserStorageSyncAckBatchJson|Android ActriumRust facade exposes browser sync ack normalization"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|normalizeBrowserStorageSyncCursorsJson|Android ActriumRust facade exposes browser sync cursor normalization"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncPushRequestJson|Android ActriumRust facade exposes browser sync push request building"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncPullRequestJson|Android ActriumRust facade exposes browser sync pull request building"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCycleResponseJson|Android ActriumRust facade exposes browser sync cycle response building"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncSchedulerStatusJson|Android ActriumRust facade exposes browser sync scheduler status"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|normalizeBrowserStorageSyncCycleResponseJson|Android ActriumRust facade exposes browser sync cycle response normalization"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncClientCyclePlanJson|Android ActriumRust facade exposes browser sync client cycle planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncClientAckBatchAfterRemoteApplyJson|Android ActriumRust facade exposes browser sync post-remote ack selection"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncTransportContractJson|Android ActriumRust facade exposes browser sync transport contract"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudStoreContractJson|Android ActriumRust facade exposes browser sync cloud store contract"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudProviderAdapterContractJson|Android ActriumRust facade exposes browser sync cloud provider adapter contract"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudProviderAdapterPlanJson|Android ActriumRust facade exposes browser sync cloud provider adapter plan"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudProviderErrorPolicyJson|Android ActriumRust facade exposes browser sync cloud provider error policy"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudProviderErrorClassificationJson|Android ActriumRust facade exposes browser sync cloud provider error classification"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|providerErrorClassificationJson|Android ActriumRust diagnostics facade accepts provider error classification"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudStoreCapabilityPreflightJson|Android ActriumRust facade exposes browser sync cloud store capability preflight"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudLocalFolderCapabilitiesJson|Android ActriumRust facade exposes browser sync local-folder capabilities"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudLocalFolderCapabilityPreflightJson|Android ActriumRust facade exposes browser sync local-folder capability preflight"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudLocalFolderCompatibilityPreflightJson|Android ActriumRust facade exposes browser sync local-folder compatibility preflight"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudDataModelJson|Android ActriumRust facade exposes browser sync cloud data model"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudCompatibilityStatusJson|Android ActriumRust facade exposes browser sync cloud compatibility status"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudCompatibilityPreflightJson|Android ActriumRust facade exposes browser sync cloud compatibility preflight"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudSchemaRolloutPlanJson|Android ActriumRust facade exposes browser sync cloud schema rollout planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudSchemaMigrationPlaybookJson|Android ActriumRust facade exposes browser sync cloud schema migration playbook"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSchemaMigrationPlanJson|Android ActriumRust facade exposes the browser storage schema migration plan"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudRecoverySnapshotPlanJson|Android ActriumRust facade exposes browser sync cloud recovery snapshot planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudRecoverySnapshotResultPlanJson|Android ActriumRust facade exposes browser sync cloud recovery snapshot result handoff"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudLocalFolderRecoverySnapshotRoundJson|Android ActriumRust facade exposes browser sync local-folder recovery snapshot rounds"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudDefaultManifestJson|Android ActriumRust facade exposes browser sync cloud manifest defaults"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudLogObjectJson|Android ActriumRust facade exposes browser sync cloud log objects"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudSnapshotObjectJson|Android ActriumRust facade exposes browser sync cloud snapshot objects"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudMembershipPlanJson|Android ActriumRust facade exposes browser sync cloud membership plans"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudMembershipResultPlanJson|Android ActriumRust facade exposes browser sync cloud membership result plans"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudPushPlanJson|Android ActriumRust facade exposes browser sync cloud push planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudPushResultPlanJson|Android ActriumRust facade exposes browser sync cloud push result planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudPullPlanJson|Android ActriumRust facade exposes browser sync cloud pull planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudPullApplyPlanJson|Android ActriumRust facade exposes browser sync cloud pull apply planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudPullContinuationPlanJson|Android ActriumRust facade exposes browser sync cloud pull continuation planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudCompactionPlanJson|Android ActriumRust facade exposes browser sync cloud compaction planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudCompactionResultPlanJson|Android ActriumRust facade exposes browser sync cloud compaction result planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudLocalFolderCompactionRoundJson|Android ActriumRust facade exposes browser sync local-folder compaction round"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudLocalFolderCleanupRoundJson|Android ActriumRust facade exposes browser sync local-folder cleanup round"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudHighWatermarkPolicyJson|Android ActriumRust facade exposes browser sync cloud high-watermark policy"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudHighWatermarkPlanJson|Android ActriumRust facade exposes browser sync cloud high-watermark planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudCleanupResultPlanJson|Android ActriumRust facade exposes browser sync cloud cleanup result planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudCycleClientPlanJson|Android ActriumRust facade exposes browser sync cloud cycle client planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudCycleStatusPolicyJson|Android ActriumRust facade exposes browser sync cloud cycle status policy"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudMembershipStatusPolicyJson|Android ActriumRust facade exposes browser sync cloud membership status policy"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudCycleScheduleStateJson|Android ActriumRust facade exposes browser sync cloud cycle schedule state"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudMembershipScheduleStateJson|Android ActriumRust facade exposes browser sync cloud membership schedule state"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudMembershipAttemptPlanJson|Android ActriumRust facade exposes browser sync cloud membership attempt plan"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudMembershipLoopPlanJson|Android ActriumRust facade exposes browser sync cloud membership loop plan"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudProviderDiagnosticsJson|Android ActriumRust facade exposes browser sync cloud provider diagnostics"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudObservabilityEventJson|Android ActriumRust facade exposes browser sync cloud observability events"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudCycleAttemptPlanJson|Android ActriumRust facade exposes browser sync cloud cycle attempt plan"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudCycleLoopPlanJson|Android ActriumRust facade exposes browser sync cloud cycle loop plan"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudLocalFolderCycleRoundJson|Android ActriumRust facade exposes browser sync local-folder cycle rounds"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCloudLocalFolderMembershipRoundJson|Android ActriumRust facade exposes browser sync local-folder membership rounds"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCycleRequestPlanJson|Android ActriumRust facade exposes browser sync cycle request planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncCycleTransportResultJson|Android ActriumRust facade exposes browser sync transport result classification"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncStatusPolicyJson|Android ActriumRust facade exposes browser sync status policy"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncScheduleStateJson|Android ActriumRust facade exposes browser sync schedule state"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageDataModelJson|Android ActriumRust facade exposes browser storage data model"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncAckPlanJson|Android ActriumRust facade exposes browser sync ack planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncRemoteApplyPlanJson|Android ActriumRust facade exposes browser sync remote apply planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncRemoteApplyResultJson|Android ActriumRust facade exposes browser sync remote apply result summaries"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncServerMergePlanJson|Android ActriumRust facade exposes browser sync reference merge planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncServerMergeResultJson|Android ActriumRust facade exposes browser sync reference merge result summaries"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncConflictPolicyJson|Android ActriumRust facade exposes browser sync conflict policy"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSyncMergePolicyJson|Android ActriumRust facade exposes browser sync merge policy"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageAddressCanonicalizationPolicyJson|Android ActriumRust facade exposes address canonicalization policy"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageCanonicalizeAddressesJson|Android ActriumRust facade exposes batched address canonicalization"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageBookmarkFolderTreeIntegrityPolicyJson|Android ActriumRust facade exposes bookmark folder tree integrity policy"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageBookmarkFolderTreeIntegrityPlanJson|Android ActriumRust facade exposes bookmark folder tree integrity planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageBookmarkRestorePlacementPolicyJson|Android ActriumRust facade exposes bookmark restore placement policy"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageBookmarkRestorePlacementPlanJson|Android ActriumRust facade exposes bookmark restore placement planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageBookmarkOwnershipPolicyJson|Android ActriumRust facade exposes bookmark ownership policy"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageBookmarkOwnershipPlanJson|Android ActriumRust facade exposes bookmark ownership planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageHistoryVisitAggregationPolicyJson|Android ActriumRust facade exposes history visit aggregation policy"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageHistoryVisitAggregationPlanJson|Android ActriumRust facade exposes history visit aggregation planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSessionTabOrderingPolicyJson|Android ActriumRust facade exposes session tab ordering policy"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSessionTabOrderingPlanJson|Android ActriumRust facade exposes session tab ordering planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSessionActiveTabConsistencyPolicyJson|Android ActriumRust facade exposes session active-tab consistency policy"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageSessionActiveTabConsistencyPlanJson|Android ActriumRust facade exposes session active-tab consistency planning"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageRetentionPruningPolicyJson|Android ActriumRust facade exposes retention pruning policy"
  "packages/actrium-platform/android/src/main/kotlin/io/actrium/app/rust/ActriumRust.kt|browserStorageRetentionPruningPlanJson|Android ActriumRust facade exposes retention pruning planning"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageAddressCanonicalizationPolicyJson|Android UniFFI binding exposes address canonicalization policy"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageCanonicalizeAddressesJson|Android UniFFI binding exposes batched address canonicalization"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageBookmarkFolderTreeIntegrityPolicyJson|Android UniFFI binding exposes bookmark folder tree integrity policy"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageBookmarkFolderTreeIntegrityPlanJson|Android UniFFI binding exposes bookmark folder tree integrity planning"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageBookmarkRestorePlacementPolicyJson|Android UniFFI binding exposes bookmark restore placement policy"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageBookmarkRestorePlacementPlanJson|Android UniFFI binding exposes bookmark restore placement planning"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageBookmarkOwnershipPolicyJson|Android UniFFI binding exposes bookmark ownership policy"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageBookmarkOwnershipPlanJson|Android UniFFI binding exposes bookmark ownership planning"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageHistoryVisitAggregationPolicyJson|Android UniFFI binding exposes history visit aggregation policy"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageHistoryVisitAggregationPlanJson|Android UniFFI binding exposes history visit aggregation planning"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSessionTabOrderingPolicyJson|Android UniFFI binding exposes session tab ordering policy"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSessionTabOrderingPlanJson|Android UniFFI binding exposes session tab ordering planning"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSessionActiveTabConsistencyPolicyJson|Android UniFFI binding exposes session active-tab consistency policy"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSessionActiveTabConsistencyPlanJson|Android UniFFI binding exposes session active-tab consistency planning"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageRetentionPruningPolicyJson|Android UniFFI binding exposes retention pruning policy"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageRetentionPruningPlanJson|Android UniFFI binding exposes retention pruning planning"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudProviderAdapterContractJson|Android UniFFI binding exposes browser sync provider adapter contract"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudProviderAdapterPlanJson|Android UniFFI binding exposes browser sync provider adapter plan"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudSchemaRolloutPlanJson|Android UniFFI binding exposes browser sync cloud schema rollout planning"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudSchemaMigrationPlaybookJson|Android UniFFI binding exposes browser sync cloud schema migration playbook"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSchemaMigrationPlanJson|Android UniFFI binding exposes the browser storage schema migration plan"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudRecoverySnapshotPlanJson|Android UniFFI binding exposes browser sync cloud recovery snapshot planning"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudRecoverySnapshotResultPlanJson|Android UniFFI binding exposes browser sync cloud recovery snapshot result handoff"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudLocalFolderRecoverySnapshotRoundJson|Android UniFFI binding exposes browser sync local-folder recovery snapshot rounds"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudCycleLoopPlanJson|Android UniFFI binding exposes browser sync cloud cycle loop plan"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudProviderDiagnosticsJson|Android UniFFI binding exposes browser sync cloud provider diagnostics"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudProviderErrorPolicyJson|Android UniFFI binding exposes browser sync cloud provider error policy"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudProviderErrorClassificationJson|Android UniFFI binding exposes browser sync cloud provider error classification"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|providerErrorClassificationJson|Android UniFFI diagnostics binding accepts provider error classification"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudMembershipStatusPolicyJson|Android UniFFI binding exposes browser sync cloud membership status policy"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudMembershipScheduleStateJson|Android UniFFI binding exposes browser sync cloud membership schedule state"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudMembershipAttemptPlanJson|Android UniFFI binding exposes browser sync cloud membership attempt plan"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudMembershipLoopPlanJson|Android UniFFI binding exposes browser sync cloud membership loop plan"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudObservabilityEventJson|Android UniFFI binding exposes browser sync cloud observability events"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudHighWatermarkPolicyJson|Android UniFFI binding exposes browser sync cloud high-watermark policy"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudHighWatermarkPlanJson|Android UniFFI binding exposes browser sync cloud high-watermark planning"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudCleanupResultPlanJson|Android UniFFI binding exposes browser sync cloud cleanup result planning"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudLocalFolderCleanupRoundJson|Android UniFFI binding exposes browser sync local-folder cleanup round"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudLocalFolderCapabilitiesJson|Android UniFFI binding exposes browser sync local-folder capabilities"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudLocalFolderCapabilityPreflightJson|Android UniFFI binding exposes browser sync local-folder capability preflight"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudLocalFolderCompatibilityPreflightJson|Android UniFFI binding exposes browser sync local-folder compatibility preflight"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudLocalFolderCycleRoundJson|Android UniFFI binding exposes browser sync local-folder cycle rounds"
  "packages/actrium-platform/android/src/main/kotlin/uniffi/actrium_core/actrium_core.kt|browserStorageSyncCloudLocalFolderMembershipRoundJson|Android UniFFI binding exposes browser sync local-folder membership rounds"
  "android/android/app/src/main/java/io/actrium/app/data/database/BrowserStorageDatabase.kt|applyRemoteSyncPlan|Android browser storage adapter executes Rust browser sync remote apply plans"
  "android/android/app/src/main/java/io/actrium/app/data/database/BrowserStorageDatabase.kt|applySyncAckPlan|Android browser storage adapter executes Rust browser sync ack plans"
  "android/android/app/src/main/java/io/actrium/app/data/database/BrowserStorageDatabase.kt|applySyncCycleResponse|Android browser storage adapter applies Rust-planned sync cycles"
  "android/android/app/src/main/java/io/actrium/app/data/database/BrowserStorageDatabase.kt|exportDirtySyncPushRequest|Android browser storage adapter exports dirty rows through Rust push builder"
  "android/android/app/src/main/java/io/actrium/app/data/database/BrowserStorageDatabase.kt|exportSyncCycleRequestPlan|Android browser storage adapter exports Rust-planned sync cycle requests"
  "android/android/app/src/main/java/io/actrium/app/data/database/BrowserStorageDatabase.kt|applyCloudCycleClientPlan|Android browser storage adapter applies Rust cloud cycle client plans"
  "android/android/app/src/main/java/io/actrium/app/data/database/BrowserStorageDatabase.kt|createAtPath|Android browser storage adapter can open isolated fixture databases"
  "android/android/app/src/main/java/io/actrium/app/data/database/BrowserStorageDatabase.kt|executeUpdateDelete|Android browser storage adapter reports rows affected for browser sync conflicts"
  "android/android/app/src/main/java/io/actrium/app/data/database/BrowserStorageDatabase.kt|owner_kind|Android browser storage adapter migrates bookmark owner kind"
  "android/android/app/src/main/java/io/actrium/app/data/database/BrowserStorageDatabase.kt|ownerKind|Android browser storage adapter maps bookmark owner kind"
  "android/android/app/src/main/java/io/actrium/app/data/database/BrowserStorageDatabase.kt|surface_kind|Android browser storage adapter migrates session surface kind"
  "android/android/app/src/main/java/io/actrium/app/data/database/BrowserStorageDatabase.kt|surfaceKind|Android browser storage adapter maps session surface kind"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|BrowserStorageSyncTransport|Android browser sync client uses injectable transport"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|browserStorageSyncCycleTransportResultJson|Android browser sync client classifies transport through Rust"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|runLocalFolderSyncCycle|Android browser sync client exposes local-folder cloud cycle execution"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|runLocalFolderMembershipRound|Android browser sync client exposes local-folder cloud membership execution"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|browserStorageSyncCloudLocalFolderCapabilityPreflightJson|Android browser sync client consumes Rust local-folder capability preflight"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|browserStorageSyncCloudLocalFolderCompatibilityPreflightJson|Android browser sync client consumes Rust local-folder compatibility preflight"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|compatibilityPreflight|Android browser sync client returns Rust local-folder compatibility preflight"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|browserStorageSyncCloudLocalFolderCycleRoundJson|Android browser sync client consumes Rust local-folder cycle helper"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|browserStorageSyncCloudLocalFolderCompactionRoundJson|Android browser sync client consumes Rust local-folder compaction helper"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|browserStorageSyncCloudLocalFolderCleanupRoundJson|Android browser sync client consumes Rust local-folder cleanup helper"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|cloudSchemaRolloutPlan|Android browser sync client consumes Rust schema rollout planning on compatibility block"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|cloudSchemaMigrationPlaybook|Android browser sync client consumes Rust schema migration playbook on compatibility block"
  "android/android/app/src/main/java/io/actrium/app/data/database/BrowserStorageDatabase.kt|browserStorageSchemaMigrationPlanJson|Android browser storage adapter consumes the Rust schema migration plan"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|localFolderRecoverySnapshotRound|Android browser sync client consumes Rust local-folder recovery snapshot round on compatibility block"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|browserStorageSyncCloudLocalFolderMembershipRoundJson|Android browser sync client consumes Rust local-folder membership helper"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|cloudMembershipStatusPolicy|Android browser sync membership client consumes Rust membership status policy"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|\"statusPolicy\"|Android browser sync membership client returns Rust membership status policy"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|cloudMembershipScheduleState|Android browser sync membership client consumes Rust membership schedule state"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|\"scheduleState\"|Android browser sync membership client returns Rust membership schedule state"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|cloudMembershipAttemptPlan|Android browser sync membership client consumes Rust membership attempt plan"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|\"attemptPlan\"|Android browser sync membership client returns Rust membership attempt plan"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|cloudMembershipLoopPlan|Android browser sync membership client consumes Rust membership loop plan"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|\"loopPlan\"|Android browser sync membership client returns Rust membership loop plan"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|val schemaRolloutPlan: JSONObject?|Android browser sync membership client prepares Rust schema rollout on compatibility block"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|val compatibilityPreflight = membershipRound.optJSONObject|Android browser sync client lifts Rust membership compatibility preflight"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|observabilityEvent = cloudObservabilityEvent|Android browser sync membership client exposes Rust observability events"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|browserStorageSyncCloudCycleScheduleStateJson|Android browser sync client consumes Rust cloud cycle schedule state"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|browserStorageSyncCloudProviderDiagnosticsJson|Android browser sync client consumes Rust cloud provider diagnostics"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|providerErrorClassificationJson|Android browser sync client passes provider error diagnostics slot"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|localFolderProviderErrorClassification|Android browser sync client builds provider error classification from real local-folder rounds"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|localFolderProviderRoundErrors|Android browser sync client sources provider errors from local-folder rounds"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|browserStorageSyncCloudObservabilityEventJson|Android browser sync client consumes Rust cloud observability events"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|browserStorageSyncCloudCycleAttemptPlanJson|Android browser sync client consumes Rust cloud cycle attempt planning"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt|browserStorageSyncCloudCycleLoopPlanJson|Android browser sync client consumes Rust cloud cycle loop planning"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncCoordinator.kt|BrowserStorageSyncCredentialsProvider|Android browser sync coordinator uses injectable credential lookup"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncCoordinator.kt|BrowserStorageSyncClient|Android browser sync coordinator delegates network/apply work to the sync client"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncCoordinator.kt|statusPolicy|Android browser sync coordinator returns Rust-owned status policy payloads"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncCoordinator.kt|scheduleState|Android browser sync coordinator returns Rust-owned schedule state payloads"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncScheduler.kt|BrowserStorageSyncSchedulerSnapshot|Android browser sync scheduler exposes platform schedule snapshots"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncScheduler.kt|nextRunDelaySeconds|Android browser sync scheduler consumes Rust-owned schedule state timing"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncScheduler.kt|statusJson|Android browser sync scheduler exposes Rust-owned product status"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncScheduler.kt|scheduleAfterLocalChange|Android browser sync scheduler can debounce local browser data changes"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncCredentialStore.kt|SharedPreferences|Android browser sync credential store persists native session credentials"
  "android/android/app/src/main/java/io/actrium/app/data/repository/BrowserDataRepository.kt|onLocalMutation|Android browser data repository exposes local-change sync hooks"
  "android/android/app/src/main/java/io/actrium/app/data/repository/BrowserDataRepository.kt|ownerKind = \"tab\"|Android tab-derived bookmarks are marked with tab ownership"
  "android/android/app/src/main/java/io/actrium/app/data/repository/BrowserDataRepository.kt|ownerId = activeModel.id|Android tab-derived bookmarks carry stable tab owner ids"
  "android/android/app/src/main/java/io/actrium/app/core/BrowserDataModule.kt|saveSyncSession|Android BrowserDataModule exposes sync credential writes"
  "android/android/app/src/main/java/io/actrium/app/core/BrowserDataModule.kt|syncNow|Android BrowserDataModule exposes a native sync trigger"
  "android/android/app/src/main/java/io/actrium/app/core/BrowserDataModule.kt|getSyncStatus|Android BrowserDataModule exposes native sync status"
  "android/android/app/src/main/java/io/actrium/app/MainActivity.kt|BrowserSyncStack|Android phone shell wires local-change sync scheduling through the shared sync stack"
  "android/android/app/src/main/java/io/actrium/app/tablet/TabletViewModel.kt|BrowserSyncStack|Android tablet shell wires local-change sync scheduling through the shared sync stack"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserSyncStack.kt|scheduleAfterLocalChange|Android shared sync stack schedules sync after local changes"
  "android/android/app/src/main/java/io/actrium/app/data/sync/BrowserSyncStack.kt|BrowserStorageSyncScheduler|Android shared sync stack owns the single sync scheduler"
  "android/android/app/src/main/java/io/actrium/app/generated/ActriumAndroidPersistenceContract.kt|object Runtime|Android generated persistence contract includes runtime database constants"
  "android/android/app/src/main/java/io/actrium/app/generated/ActriumAndroidPersistenceContract.kt|SELECT_TOKEN_BY_ENV|Android generated persistence contract includes auth token queries"
  "packages/actrium/src/runtime-host/generated/actriumCoreContract.ts|RUNTIME_DATABASE_NAME = 'AppDatabase.db'|RN generated contract exports the runtime database name"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|migratesBrowserStorageSQLiteV1V2V3ToV6|Android instrumentation tests cover browser storage v1/v2/v3 SQLite migration"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|plansCloudProviderAdapterObjectSpaceThroughRust|Android instrumentation tests cover provider adapter object-space plan"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|classifiesCloudProviderConnectionStatusThroughRust|Android instrumentation tests cover provider connection status policy"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|classifiesCloudProviderErrorsThroughRust|Android instrumentation tests cover provider error classification"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|providerErrorClassification|Android instrumentation tests cover provider error diagnostics source mapping"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|batchHasBookmarkOwner|Android instrumentation tests cover browser bookmark owner sync payloads"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|exportDirtySyncPushRequest|Android browser storage migration tests cover dirty push export"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|applySyncCycleResponse|Android browser storage migration tests cover sync cycle apply"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|exportSyncCycleRequestPlan|Android browser storage migration tests cover sync cycle request planning"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|syncClientExecutesInjectedCycleTransportAndAppliesResponse|Android instrumentation tests cover browser sync client cycle"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|syncClientRunsLocalFolderCloudCycleAndAppliesClientPlan|Android instrumentation tests cover local-folder cloud sync client"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|syncClientRunsLocalFolderCloudMembershipRound|Android instrumentation tests cover local-folder cloud membership client"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|finishCloudDeviceRevoked|Android local-folder cloud membership test asserts Rust revoke classification"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|device-android-2|Android local-folder cloud membership test asserts compatibility blocking prevents new device writes"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|capabilityPreflight|Android local-folder cloud sync test asserts Rust capability preflight"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|compatibilityPreflight|Android local-folder cloud sync test asserts Rust compatibility preflight"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|clientUpgradeRequired|Android local-folder cloud sync test asserts compatibility blocking"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|cloudCycleScheduleState|Android local-folder cloud sync test asserts Rust schedule state"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|providerDiagnostics|Android local-folder cloud sync test asserts Rust provider diagnostics"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|cloudObservabilityEvent|Android local-folder cloud sync test asserts Rust observability event"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|loopPlan|Android local-folder cloud sync test asserts Rust loop planning"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|compactionRound|Android local-folder cloud sync test asserts Rust compaction round"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|cleanupRound|Android local-folder cloud sync test asserts Rust cleanup round"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|recoverySnapshotRound|Android local-folder cloud sync test asserts Rust recovery snapshot round"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|providerDiagnostics.*compatibilityClassification|Android local-folder cloud membership test asserts Rust provider diagnostics"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|observabilityEvent.*planType|Android local-folder cloud membership test asserts Rust observability event"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|cloudMembershipStatusPolicy|Android local-folder cloud membership test asserts Rust membership status policy"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|cloudMembershipScheduleState|Android local-folder cloud membership test asserts Rust membership schedule state"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|cloudMembershipAttemptPlan|Android local-folder cloud membership test asserts Rust membership attempt plan"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|cloudMembershipLoopPlan|Android local-folder cloud membership test asserts Rust membership loop plan"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|finishMembershipAttemptCount|Android local-folder cloud membership test asserts Rust membership loop summary"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|blockedMembershipRollout|Android local-folder cloud membership test asserts Rust schema rollout on compatibility block"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|syncCoordinatorSkipsCycleWhenCredentialsUnavailable|Android instrumentation tests cover browser sync credential gating"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|syncCoordinatorRunsClientWithAvailableCredentials|Android instrumentation tests cover browser sync coordinator delegation"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|waitForCredentials|Android instrumentation tests cover sync coordinator status policy"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|syncCoordinatorSchedulesRetryFromStatusPolicy|Android instrumentation tests cover sync coordinator schedule state"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|syncSchedulerConsumesScheduleStateAndResetsAfterSuccess|Android instrumentation tests cover sync scheduler retry lifecycle"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|retryScheduled|Android instrumentation tests cover Rust-owned sync scheduler status"
  "android/android/app/src/androidTest/java/io/actrium/app/data/database/BrowserStorageDatabaseInstrumentedTest.kt|syncCoordinatorReadsStoredNativeCredentials|Android instrumentation tests cover native browser sync credential-store integration"
)

for requirement in "${windows_browser_data_requirements[@]}"; do
  file="${requirement%%|*}"
  remainder="${requirement#*|}"
  required_pattern="${remainder%%|*}"
  description="${remainder##*|}"

  if [[ ! -f "$file" ]]; then
    print_failure "$description file is missing"
  elif ! rg -q "$required_pattern" "$file"; then
    print_failure "$description"
  else
    print_ok "$description"
  fi
done

windows_snapshot_post_rewrite="$(
  rg -n 'NormalizeSnapshotFallback\(normalized' \
    windows/src/BrowserHistoryStore.cs \
    windows/src/BrowserBookmarkStore.cs \
    windows/src/BrowserSessionStore.cs \
    2>/dev/null || true
)"

if [[ -n "$windows_snapshot_post_rewrite" ]]; then
  print_failure "Windows browser stores re-normalize Rust snapshot output with platform rules"
  printf '%s\n' "$windows_snapshot_post_rewrite"
else
  print_ok "Windows browser stores adopt Rust snapshot normalization output verbatim"
fi

platform_session_tab_id_generation="$(
  rg -n 'fnv1a|stable_session_tab_id|stableSessionTabId|StableSessionTabId' \
    packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageDatabase.swift \
    android/android/app/src/main/java/io/actrium/app/data/database/BrowserStorageDatabase.kt \
    windows/src/BrowserSessionStore.cs \
    windows/src/BrowserStorageDatabase.cs \
    2>/dev/null || true
)"

if [[ -n "$platform_session_tab_id_generation" ]]; then
  print_failure "Platform session persistence re-implements the Rust stable session tab id derivation"
  printf '%s\n' "$platform_session_tab_id_generation"
else
  print_ok "Platform session persistence leaves stable session tab id derivation to the Rust core"
fi

local_folder_capability_duplicate_fields="$(
  rg -n 'supportsConditionalWrite|supportsConditionalDelete|supportsReadIfChanged|supportsPrefixList|supportsPagedList|revisionKind|conflictResolution' \
    windows/src/BrowserStorageSyncClient.cs \
    packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageSyncClient.swift \
    android/android/app/src/main/java/io/actrium/app/data/sync/BrowserStorageSyncClient.kt \
    2>/dev/null || true
)"

if [[ -n "$local_folder_capability_duplicate_fields" ]]; then
  print_failure "Local-folder sync clients duplicate provider capability fields instead of using the Rust local-folder capability preflight helper"
  printf '%s\n' "$local_folder_capability_duplicate_fields"
else
  print_ok "Local-folder sync clients use the Rust-owned local-folder capability preflight helper"
fi

apple_manifest_reader_files=(
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleActrPackageManifestReader.swift"
  "ios/Actrium/Models/ActrPackageManifestReader.swift"
  "macos/Actrium/Models/ActrPackageManifestReader.swift"
)

missing_apple_manifest_files=()
for file in "${apple_manifest_reader_files[@]}"; do
  if [[ ! -f "$file" ]]; then
    missing_apple_manifest_files+=("$file")
  fi
done

if [[ "${#missing_apple_manifest_files[@]}" -ne 0 ]]; then
  print_failure "Apple ACTR manifest reader boundary files are missing"
  printf '%s\n' "${missing_apple_manifest_files[@]}"
else
  print_ok "Apple ACTR manifest reader boundary files exist"
fi

apple_manifest_reader_duplicates="$(
  rg -n 'CryptoKit|SHA256|parseActrPackageManifest|createPackageIntegrityPlan|evaluateContract|CoreError|findManifestJSON|resolvedPackageIntegrityFileURL' \
    ios/Actrium/Models/ActrPackageManifestReader.swift \
    macos/Actrium/Models/ActrPackageManifestReader.swift \
    2>/dev/null || true
)"

if [[ -n "$apple_manifest_reader_duplicates" ]]; then
  print_failure "Apple app-local ACTR manifest readers duplicate shared package-loading logic"
  printf '%s\n' "$apple_manifest_reader_duplicates"
else
  print_ok "Apple app-local ACTR manifest readers delegate package-loading logic"
fi

if ! rg -q 'AppleActrPackageManifestReader\.read' \
  ios/Actrium/Models/ActrPackageManifestReader.swift \
  macos/Actrium/Models/ActrPackageManifestReader.swift; then
  print_failure "Apple app-local ACTR manifest readers do not call the shared ActriumCore helper"
else
  print_ok "Apple app-local ACTR manifest readers call the shared ActriumCore helper"
fi

apple_sqlite_boundary_files=(
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleSQLiteDatabase.swift"
  "packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageDatabase.swift"
  "ios/Actrium/Database/DatabaseManager.swift"
  "macos/Actrium/Database/DatabaseManager.swift"
)

missing_apple_sqlite_files=()
for file in "${apple_sqlite_boundary_files[@]}"; do
  if [[ ! -f "$file" ]]; then
    missing_apple_sqlite_files+=("$file")
  fi
done

if [[ "${#missing_apple_sqlite_files[@]}" -ne 0 ]]; then
  print_failure "Apple SQLite boundary files are missing"
  printf '%s\n' "${missing_apple_sqlite_files[@]}"
else
  print_ok "Apple SQLite boundary files exist"
fi

apple_database_manager_duplicates="$(
  rg -n 'import SQLite3|sqlite3_|OpaquePointer|CoreSqlValueInfo.*sqliteValue' \
    ios/Actrium/Database/DatabaseManager.swift \
    macos/Actrium/Database/DatabaseManager.swift \
    2>/dev/null || true
)"

if [[ -n "$apple_database_manager_duplicates" ]]; then
  print_failure "Apple app-local DatabaseManager files duplicate shared SQLite glue"
  printf '%s\n' "$apple_database_manager_duplicates"
else
  print_ok "Apple app-local DatabaseManager files delegate SQLite glue"
fi

if ! rg -q 'AppleSQLiteDatabase' \
  ios/Actrium/Database/DatabaseManager.swift \
  macos/Actrium/Database/DatabaseManager.swift; then
  print_failure "Apple app-local DatabaseManager files do not call the shared SQLite helper"
else
  print_ok "Apple app-local DatabaseManager files call the shared SQLite helper"
fi

apple_browser_storage_file="packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleBrowserStorageDatabase.swift"
if [[ ! -f "$apple_browser_storage_file" ]]; then
  print_failure "Apple shared browser storage adapter is missing"
elif ! rg -q 'browserStorageSchema\(\)' "$apple_browser_storage_file"; then
  print_failure "Apple shared browser storage adapter does not consume Rust storage schema"
elif ! rg -q 'browserStorageRepositorySql\(\)' "$apple_browser_storage_file"; then
  print_failure "Apple shared browser storage adapter does not consume Rust repository SQL"
elif ! rg -q 'AppleSQLiteDatabase' "$apple_browser_storage_file"; then
  print_failure "Apple shared browser storage adapter does not own SQLite execution through AppleSQLiteDatabase"
elif ! rg -q 'hasMetaFlag' "$apple_browser_storage_file" ||
  ! rg -q 'setMetaFlag' "$apple_browser_storage_file"; then
  print_failure "Apple shared browser storage adapter is missing migration meta flag helpers"
elif ! rg -q 'deleteBookmark\(id: String\)' "$apple_browser_storage_file"; then
  print_failure "Apple shared browser storage adapter is missing id-scoped bookmark deletion"
elif ! rg -q 'tab_id' "$apple_browser_storage_file" ||
  ! rg -q 'syncSql\.' "$apple_browser_storage_file" ||
  ! rg -q 'selectSyncCursorsForScope' "$apple_browser_storage_file" ||
  ! rg -q 'migrateBrowserStorageSchemaIfNeeded' "$apple_browser_storage_file"; then
  print_failure "Apple shared browser storage adapter is missing browser session/sync migration readwrite"
else
  print_ok "Apple shared browser storage adapter consumes Rust schema through AppleSQLiteDatabase"
fi

macos_browser_data_repository_file="macos/Actrium/Database/BrowserDataRepository.swift"
macos_browser_view_model_file="macos/Actrium/Views/MainContentView/BrowserViewModel.swift"
if [[ ! -f "$macos_browser_data_repository_file" ]]; then
  print_failure "macOS browser data repository is missing"
elif ! rg -q 'AppleBrowserStorageDatabase' "$macos_browser_data_repository_file"; then
  print_failure "macOS browser data repository does not consume the shared Apple browser storage adapter"
elif ! rg -q 'recordHistoryVisit' "$macos_browser_data_repository_file"; then
  print_failure "macOS browser data repository is missing history visit browser storage calls"
elif ! rg -q 'upsertBookmarkItem' "$macos_browser_data_repository_file"; then
  print_failure "macOS browser data repository is missing bookmark browser storage calls"
elif ! rg -q 'saveSession' "$macos_browser_data_repository_file"; then
  print_failure "macOS browser data repository is missing session browser storage calls"
elif ! rg -q 'clearHistory' "$macos_browser_data_repository_file"; then
  print_failure "macOS browser data repository is missing history clearing calls"
elif ! rg -q 'clearBookmarks' "$macos_browser_data_repository_file"; then
  print_failure "macOS browser data repository is missing history/bookmark browser storage calls"
elif ! rg -q 'loadRecentHistory' "$macos_browser_data_repository_file"; then
  print_failure "macOS browser data repository is missing shared browser history readback"
elif ! rg -q 'loadBookmarks' "$macos_browser_data_repository_file"; then
  print_failure "macOS browser data repository is missing shared browser bookmark readback"
elif ! rg -q 'backfillHistoryIfNeeded' "$macos_browser_data_repository_file"; then
  print_failure "macOS browser data repository is missing local history backfill"
elif ! rg -q 'backfillBookmarksIfNeeded' "$macos_browser_data_repository_file"; then
  print_failure "macOS browser data repository is missing local bookmark backfill"
elif [[ ! -f "$macos_browser_view_model_file" ]]; then
  print_failure "macOS BrowserViewModel is missing"
elif ! rg -q 'persistBrowserPageSession' "$macos_browser_view_model_file"; then
  print_failure "macOS BrowserViewModel does not persist browser-page session snapshots"
elif ! rg -q 'BrowserDataRepository\.shared' \
  macos/Actrium/ViewModels/HistoryViewModel.swift \
  macos/Actrium/ViewModels/BookmarkViewModel.swift; then
  print_failure "macOS history/bookmark view models do not write through BrowserDataRepository"
elif ! rg -q 'loadRecentHistory' macos/Actrium/ViewModels/HistoryViewModel.swift ||
  ! rg -q 'loadBookmarks' macos/Actrium/ViewModels/BookmarkViewModel.swift; then
  print_failure "macOS history/bookmark view models do not read back shared browser data"
elif ! rg -q 'backfillSharedBrowserHistory' macos/Actrium/ViewModels/HistoryViewModel.swift ||
  ! rg -q 'backfillSharedBrowserBookmarks' macos/Actrium/ViewModels/BookmarkViewModel.swift; then
  print_failure "macOS history/bookmark view models do not backfill local browser-page data"
else
  print_ok "macOS browser history/bookmark/session data uses the shared browser storage adapter"
fi

ios_browser_data_repository_file="ios/Actrium/Database/BrowserDataRepository.swift"
ios_browser_view_model_file="ios/Actrium/Views/MainContentView/BrowserViewModel.swift"
if [[ ! -f "$ios_browser_data_repository_file" ]]; then
  print_failure "iOS browser data repository is missing"
elif ! rg -q 'AppleBrowserStorageDatabase' "$ios_browser_data_repository_file"; then
  print_failure "iOS browser data repository does not consume the shared Apple browser storage adapter"
elif ! rg -q 'recordHistoryVisit' "$ios_browser_data_repository_file"; then
  print_failure "iOS browser data repository is missing history visit browser storage calls"
elif rg -q 'database\?\.saveBookmarks\(items\)' "$ios_browser_data_repository_file"; then
  print_failure "iOS tab-derived bookmark persistence full-replaces browser bookmarks"
elif ! rg -q 'staleTabBookmarkIds' "$ios_browser_data_repository_file" ||
  ! rg -q 'upsertBookmarkItem' "$ios_browser_data_repository_file" ||
  ! rg -q 'deleteBookmark\(id: bookmarkId\)' "$ios_browser_data_repository_file"; then
  print_failure "iOS tab-derived bookmark persistence does not use id-scoped upsert/delete"
elif ! rg -q 'saveSession' "$ios_browser_data_repository_file"; then
  print_failure "iOS browser data repository is missing session browser storage calls"
elif ! rg -q 'loadSessionTabs' "$ios_browser_data_repository_file"; then
  print_failure "iOS browser data repository is missing shared browser session readback"
elif ! rg -q 'loadRecentHistory' "$ios_browser_data_repository_file"; then
  print_failure "iOS browser data repository is missing shared browser history readback"
elif ! rg -q 'backfillHistoryIfNeeded' "$ios_browser_data_repository_file"; then
  print_failure "iOS browser data repository is missing local history backfill"
elif [[ ! -f "$ios_browser_view_model_file" ]]; then
  print_failure "iOS BrowserViewModel is missing"
elif ! rg -q 'persistBrowserPageData' "$ios_browser_view_model_file"; then
  print_failure "iOS BrowserViewModel does not persist browser-page session/bookmark snapshots"
elif ! rg -q 'restoreSharedBrowserSessionIfNeeded' "$ios_browser_view_model_file"; then
  print_failure "iOS BrowserViewModel does not restore empty local tabs from shared browser session"
elif ! rg -q 'BrowserDataRepository\.shared' \
  ios/Actrium/ViewModels/HistoryViewModel.swift \
  "$ios_browser_view_model_file"; then
  print_failure "iOS history/view model code does not write through BrowserDataRepository"
elif ! rg -q 'loadRecentHistory' ios/Actrium/ViewModels/HistoryViewModel.swift; then
  print_failure "iOS history view model does not read back shared browser history"
elif ! rg -q 'backfillSharedBrowserHistory' ios/Actrium/ViewModels/HistoryViewModel.swift; then
  print_failure "iOS history view model does not backfill local browser-page history"
else
  print_ok "iOS browser history/bookmark/session data uses the shared browser storage adapter"
fi

apple_app_browser_storage_duplicates="$(
  rg -n 'browser_history_pages|browser_history_visits|browser_bookmarks|browser_sessions|browser_session_tabs' \
    ios/Actrium \
    macos/Actrium \
    --glob '*.swift' \
    2>/dev/null || true
)"

if [[ -n "$apple_app_browser_storage_duplicates" ]]; then
  print_failure "Apple app code duplicates browser storage table names instead of using AppleBrowserStorageDatabase"
  printf '%s\n' "$apple_app_browser_storage_duplicates"
else
  print_ok "Apple app code does not duplicate browser storage table names"
fi

apple_database_parity_pairs=(
  "ios/Actrium/Database/HistoryRepository.swift|macos/Actrium/Database/HistoryRepository.swift"
  "ios/Actrium/Database/TagRepository.swift|macos/Actrium/Database/TagRepository.swift"
)

apple_database_parity_failures=()
for pair in "${apple_database_parity_pairs[@]}"; do
  ios_file="${pair%%|*}"
  macos_file="${pair##*|}"

  if [[ ! -f "$ios_file" || ! -f "$macos_file" ]]; then
    apple_database_parity_failures+=("$ios_file <-> $macos_file (missing file)")
    continue
  fi

  if ! cmp -s "$ios_file" "$macos_file"; then
    apple_database_parity_failures+=("$ios_file <-> $macos_file")
  fi
done

if [[ "${#apple_database_parity_failures[@]}" -ne 0 ]]; then
  print_failure "Apple app-local database repositories drifted; extract or update both sides deliberately"
  printf '%s\n' "${apple_database_parity_failures[@]}"
else
  print_ok "Apple app-local database repositories are parity-locked"
fi

asr_post_processor_logs="$(
  rg -n 'Log\.[[:alpha:]]+\(' \
    android/android/app/src/main/java/io/actrium/app/core/asr/TextPostProcessor.kt \
    2>/dev/null || true
)"

if [[ -n "$asr_post_processor_logs" ]]; then
  print_failure "Android ASR text post processor logs recognized speech text"
  printf '%s\n' "$asr_post_processor_logs"
else
  print_ok "Android ASR text post processor avoids recognized-text logging"
fi

if [[ "$failures" -ne 0 ]]; then
  echo
  echo "Rust boundary hygiene failed with $failures issue(s)"
  exit 1
fi

echo
echo "Rust boundary hygiene passed"
