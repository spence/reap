#!/usr/bin/env python3
"""Run the owned native transport fixtures from the internal runtime volume."""

import argparse
import hashlib
import json
import os
import pathlib
import platform
import plistlib
import shutil
import stat
import subprocess
import sys
import tempfile


def launchctl(*arguments):
    return subprocess.run(
        ["/bin/launchctl", *arguments], capture_output=True, text=True, timeout=5
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("test_executable", type=pathlib.Path)
    parser.add_argument("--output", required=True, type=pathlib.Path)
    parser.add_argument("--filter", choices=["control::tests", "service::tests", "launch::tests",
                                            "lifetime::tests", "archive::tests", "archive::sqlite::tests",
                                            "quarantine::tests", "util::tests", "worker::tests",
                                            "journal::tests", "volumes::", "dispatch::tests",
                                            "client::tests", "preservation::tests", "git_index::tests",
                                            "git_index::recovery::tests", "files::tests",
                                            "files::applying::tests",
                                            "checkpoint::tests"], default="control::tests")
    parser.add_argument("--fault-library", type=pathlib.Path)
    parser.add_argument("--worker-binary", type=pathlib.Path)
    arguments = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("the native macOS runtime is required")
    if arguments.worker_binary and arguments.filter not in {"worker::tests", "dispatch::tests"}:
        parser.error("--worker-binary requires the worker or dispatch selection")

    source = arguments.test_executable.resolve(strict=True)
    output = arguments.output.absolute()
    output.mkdir(parents=True, exist_ok=False)
    runtime = pathlib.Path(tempfile.mkdtemp(prefix="reap-control-runtime-", dir="/private/tmp"))
    receipt = {"runtime": str(runtime), "labels_absent": [], "native_exit": None}
    result = 1
    try:
        # launchd and dyld must not open the test executable on an external volume.
        assert runtime.stat().st_dev == pathlib.Path("/System/Volumes/Data").stat().st_dev
        executable = runtime / "reap-native-test"
        shutil.copyfile(source, executable)
        executable.chmod(0o700)
        digest = hashlib.sha256()
        with executable.open("rb") as binary:
            for chunk in iter(lambda: binary.read(65536), b""):
                digest.update(chunk)
        receipt["binary_sha256"] = digest.hexdigest()
        environment = dict(os.environ)
        for name in (
            "REAP_NATIVE_CONTROL_PROBE", "REAP_NATIVE_CONTROL_SERVICE",
            "REAP_NATIVE_CAPTURE_CLIENT", "REAP_NATIVE_CAPTURE_RESULT",
            "REAP_INTERNAL_CONFIG_SNAPSHOT",
            "REAP_TEST_WORKER_BIN",
            "DYLD_INSERT_LIBRARIES",
        ):
            environment.pop(name, None)
        environment["REAP_NATIVE_CONTROL_FIXTURE_ROOT"] = str(runtime / "fixtures")
        environment["REAP_NATIVE_CAPTURE_CWD_ROOT"] = str(output / "caller-directories")
        if arguments.worker_binary:
            worker = arguments.worker_binary.resolve(strict=True)
            environment["REAP_TEST_WORKER_BIN"] = str(worker)
            receipt["worker_binary"] = str(worker)
            receipt["worker_sha256"] = hashlib.sha256(worker.read_bytes()).hexdigest()
        selection = [str(executable), arguments.filter, "--test-threads=1", "--nocapture"]
        receipt["skipped_test_driver_cases"] = []
        if arguments.worker_binary and arguments.filter == "dispatch::tests":
            # these fault gates exist only in the separate native test-driver proof
            cases = [
                "restarted_files_preview_keeps_its_live_bulk_slot_and_never_relaunches",
                "native_files_apply_recovers_interrupted_units_without_repeating_a_move",
                "native_files_apply_restart_keeps_recorded_effects_when_final_output_is_lost",
                "native_files_apply_refuses_configuration_revoked_during_final_activity",
                "native_files_apply_deletion_recovers_partial_prefix_without_resuming_recursion",
                "native_files_apply_deletion_preserves_new_keep_replacements_and_revoked_configuration",
                "native_files_apply_recovers_index_publication_without_repeating_or_losing_payloads",
            ]
            for name in cases:
                case = "dispatch::tests::" + name
                selection.extend(["--skip", case])
                receipt["skipped_test_driver_cases"].append(case)
        if arguments.fault_library:
            library = runtime / "fault_fullsync.dylib"
            shutil.copyfile(arguments.fault_library.resolve(strict=True), library)
            environment["REAP_NATIVE_CONTROL_FAULT_LIBRARY"] = str(library)
            receipt["fault_library_sha256"] = hashlib.sha256(library.read_bytes()).hexdigest()
            selection.append("--include-ignored")
        with (output / "native.log").open("wb") as log:
            # this executable contains only the owned transport fixtures, not agent providers.
            run = subprocess.run(
                selection, env=environment,
                cwd=output if arguments.filter == "archive::tests" else runtime,
                stdout=log, stderr=subprocess.STDOUT,
                timeout=300 if arguments.filter == "dispatch::tests" else 180 if arguments.filter == "journal::tests" else 60,
            )
        receipt["native_exit"] = run.returncode
        result = run.returncode
        content = (output / "native.log").read_text()
        required = {
            "control::tests": ["native_control_survives_a_real_filesystem_worker_stall",
                               "native_confirmation_requires_the_original_live_client",
                               "native_capture_pins_actual_cwd_after_client_exit_and_path_replacement"],
            "service::tests": ["native_admission_survives_control_restart_and_writer_process_death",
                               "native_files_apply_client_commits_payload_and_index_and_replays_one_result",
                               "native_result_pages_keep_complete_reports_and_survive_writer_restart",
                               "native_alias_writer_cannot_replace_the_locked_owner",
                               "malformed_native_requests_do_not_poison_the_writer",
                               "native_resolved_admission_binds_caller_namespace_and_replays_after_ticket_expiry",
                               "native_invalid_namespaces_and_untrusted_receipts_never_create_jobs",
                               "native_prepare_capacity_recovers_after_idle_ticket_expiry",
                               "native_client_prepare_admit_executes_check_and_returns_durable_terminal",
                               "native_stalled_bulk_keeps_its_slot_without_blocking_ping_or_status"],
            "launch::tests": ["unresolved_launch_reserves_all_volumes_across_restart_without_blocking_other_volumes",
                              "ungranted_child_exits_without_effects_and_ambiguous_intent_is_not_replaced",
                              "durable_grant_survives_writer_restart_and_release_requires_native_exit",
                              "two_helpers_remain_owned_and_retain_worker_slots_until_all_exits_are_proved",
                              "sequential_resolution_and_bulk_launches_never_replay_a_retired_launch_sequence",
                              "retired_helper_reservations_cannot_create_another_process_after_restart",
                              "grant_helper_and_exit_retries_refuse_replaced_journal_bindings",
                              "launch_records_boot_identity_before_any_process_exists"],
            "lifetime::tests": ["native_writer_requires_inspection_and_publishes_bound_identity",
                                "native_rename_preserves_grace_and_permission_copy_and_replacement_do_not",
                                "honored_children_leave_group_accounting_and_unbound_children_block_caps"],
            "archive::tests": ["durable_commit_replay_and_independent_retrieval_preserve_sources_and_identity",
                               "synchronization_faults_never_acknowledge_an_incomplete_commit",
                               "missing_dependencies_and_omitted_snapshot_units_cannot_publish",
                               "real_sqlite_snapshot_evidence_survives_custody_and_private_retrieval"],
            "archive::sqlite::tests": ["committed_wal_and_concurrent_commits_use_one_pinned_snapshot",
                                      "exclusive_source_lock_fails_without_wait_or_source_change",
                                      "actual_finish_ok_without_done_is_not_completion",
                                      "allowance_is_enforced_before_backup_writes_and_exact_size_succeeds",
                                      "expired_and_interrupted_budgets_never_acknowledge_a_snapshot",
                                      "unsafe_existing_and_link_paths_preserve_original_bytes"],
            "quarantine::tests": ["activity_stream_retains_a_late_protection_path_without_retaining_host_wide_output",
                                  "shared_tags_survive_linked_retirement_but_private_refs_need_preservation",
                                  "activity_stream_requires_complete_valid_bounded_fields_before_a_negative_guard",
                                  "scratch_retire_moves_to_quarantine_with_owner"],
            "checkpoint::tests": ["complete_checkpoints_require_exact_grants_binding_frames_and_eof",
                                  "safe_existing_parent_modes_keep_new_working_directories_private",
                                  "corrupt_lengths_digests_and_record_counts_never_publish_partial_input",
                                  "replaced_symlink_hardlink_and_fifo_working_files_survive_refusal",
                                  "frame_and_working_file_limits_are_sticky_without_complete_output",
                                  "path_records_preserve_native_bytes_and_refuse_unsafe_components"],
            "files::tests": ["discovery_cursor_yields_and_retains_complete_scoped_retention_inputs",
                             "discovery_cursor_failure_is_sticky_and_partial_inputs_remain_protected",
                             "discovery_cursor_revalidates_inputs_changed_between_quanta"],
            "util::tests": ["guard_probe_refuses_combined_output_above_the_capture_bound",
                            "streamed_probe_consumes_complete_evidence_above_the_diagnostic_capture_bound"],
            "worker::tests": ["files_quantum_is_a_latched_nonpublishing_query_not_a_process_launch",
                              "native_symlink_unit_moves_only_the_link_and_recovers_without_target_access",
                              "bounded_activity_reads_preserve_buffered_readiness_without_publishing_before_eof",
                              "production_resolver_retains_ownership_across_restart_and_never_respawns_a_retry",
                              "first_seen_lookups_use_cache_pages_and_batches_not_one_round_trip_per_declaration",
                              "invalid_first_seen_pages_refuse_without_partial_credit_or_repeated_requests",
                              "first_seen_requests_require_the_files_profile_and_an_uncancelled_grant",
                              "paged_worker_frames_require_complete_ordered_payload_eof_and_the_files_profile",
                              "native_probe_requires_helper_grant_both_eofs_and_clean_complete_evidence",
                              "ungranted_runtime_exit_has_no_target_result_and_cannot_clear_an_ambiguous_intent",
                              "granted_native_resolver_returns_complete_identity_and_physical_resources_without_mutation",
                              "incomplete_overlong_or_trailing_result_bytes_never_become_complete_evidence",
                              "invalid_native_directory_identities_cannot_be_complete_resolver_evidence"],
            "git_index::recovery::tests": [
                "non_branch_refs_preserve_unique_commits_and_names_in_main_and_linked_metadata",
                "pushed_clean_checkout_has_native_recovery_with_loose_and_packed_objects",
                "dirty_staged_untracked_ignored_and_unpushed_branch_work_survives",
                "branch_ancestry_upstream_and_actual_pack_deltas_have_bounded_evidence",
                "configuration_cannot_run_filters_fsmonitor_hooks_or_external_reads",
                "changed_sources_and_new_references_invalidate_cached_recovery",
                "unsupported_objects_links_and_capacity_never_become_recovery"],
            "git_index::tests": ["checkpoint_restore_retains_original_index_config_and_alias_bindings",
                                 "native_owned_removal_retires_only_exact_git_dependencies",
                                 "native_indexes_keep_tracked_paths_across_versions_and_hash_kinds",
                                 "malformed_truncated_checksum_and_capacity_evidence_never_proves_untracked",
                                 "native_extended_flags_stages_and_skip_hash_remain_tracked",
                                 "compressed_paths_are_bounded_before_index_decode",
                                 "native_tracking_matches_git_for_linked_worktrees_and_revalidates_index_changes",
                                 "native_tracking_never_executes_monitor_settings_and_refuses_split_dependencies",
                                 "native_alias_binding_changes_cannot_reuse_a_negative_tracking_verdict",
                                 "nearer_repository_cannot_reuse_an_outer_untracked_verdict",
                                 "all_conflict_stage_pairs_remain_tracked",
                                 "optional_cache_payloads_never_enter_the_recursive_extension_decoder",
                                 "native_sha256_tracking_and_worktree_settings_are_bound_to_the_checkout"],
            "journal::tests": ["unpublished_working_output_is_not_adopted_repeated_or_forgotten_by_compaction",
                               "native_metadata_preparation_cleans_only_owned_unpublished_files",
                               "native_metadata_publication_replaces_only_exact_prior_content_and_never_repeats",
                               "native_metadata_publication_refuses_changed_inputs_and_preservation_roles",
                               "native_metadata_capture_refuses_links_special_files_and_oversized_content",
                               "native_symlink_replacement_cannot_use_the_original_link_permission",
                               "native_unlink_removes_only_prepared_entries_and_retained_permission_cannot_repeat",
                               "native_unlink_refuses_replacements_nonempty_directories_and_changed_guards",
                               "recovery_bounds_unknown_working_directories_without_adopting_or_deleting_them",
                               "prior_boot_intents_release_without_pids_but_same_boot_and_legacy_unknown_intents_stay",
                               "first_seen_facts_publish_with_complete_files_results_and_cache_loss_restarts_grace",
                               "result_pages_survive_atomic_phase_recovery_and_retire_with_their_epoch",
                               "files_result_reservations_keep_small_job_capacity_and_protect_every_active_key",
                               "prior_boot_launch_cannot_grant_current_processes_or_create_helper_intents",
                               "writer_restart_reconciles_prior_boot_worker_and_ungranted_helper_before_admission"],
            "volumes::": ["native_system_and_data_share_one_resource",
                          "native_target_aliases_and_held_rename_share_keys",
                          "unknown_virtual_raid_and_incomplete_topologies_refuse",
                          "external_tunneled_nvme_requires_exact_endpoint"],
            "dispatch::tests": ["native_files_uncontended_scan_uses_one_bulk_worker",
                                "native_files_apply_recovers_index_publication_without_repeating_or_losing_payloads",
                                "native_files_apply_unknown_index_inputs_preserve_payloads_without_publication",
                                "native_files_apply_deletes_the_permitted_tree_without_touching_link_targets",
                                "native_files_apply_deletion_recovers_partial_prefix_without_resuming_recursion",
                                "native_files_apply_deletion_preserves_new_keep_replacements_and_revoked_configuration",
                                "native_files_apply_retires_the_authorized_unit_and_preserves_kept_work",
                                "native_files_apply_recovers_interrupted_units_without_repeating_a_move",
                                "native_files_apply_advances_unit_permissions_and_recomputes_remaining_caps",
                                "native_files_apply_restart_keeps_recorded_effects_when_final_output_is_lost",
                                "native_files_apply_refuses_configuration_revoked_during_final_activity",
                                "native_files_handoff_yields_to_waiting_jobs_and_publishes_grace_only_at_completion",
                                "native_fifo_waiter_precedes_later_jobs_when_an_owned_predecessor_releases_mid_tick",
                                "native_files_checkpoint_restart_corruption_cancellation_and_replaced_data_fail_closed",
                                "native_check_pipeline_completes_once_with_local_store_and_protected_survivors",
                                "native_files_first_seen_survives_rename_and_restart_but_not_copy_or_policy_change",
                                "native_files_incomplete_observation_never_becomes_grace_credit",
                                "native_files_recovery_proves_packed_git_and_preserves_ignored_data_without_git_helpers",
                                "native_files_recovery_covers_nested_enclosing_and_scratch_units",
                                "native_files_multiroot_result_preserves_every_report_beyond_the_inline_limit",
                                "native_files_tracking_uses_one_bound_index_without_git_process_helpers",
                                "native_files_batches_activity_helpers_and_preserves_open_and_retained_work",
                                "changed_native_scope_caller_config_and_grant_resources_refuse",
                                "special_oversized_external_and_incomplete_check_inputs_never_report_success",
                                "restart_retains_surviving_child_and_compaction_preserves_recovery_evidence",
                                "durable_resolver_and_bulk_results_recover_across_each_publication_boundary"],
            "client::tests": ["accepted_pending_and_unknown_never_return_completed_success",
                              "result_page_receipts_keep_the_original_outcome_and_reject_missing_or_changed_evidence",
                              "malformed_or_other_key_terminal_evidence_cannot_return_success",
                              "lost_admission_reply_keeps_the_same_key_and_reports_possible_acceptance",
                              "missing_native_service_is_prompt_and_never_falls_back_to_filesystem_work"],
            "files::applying::tests": ["repeated_guards_reuse_temporary_evaluator_capacity_for_large_inventories",
                                       "repeated_validation_reuses_ancestor_capacity_without_clearing_refusals"],
            "preservation::tests": ["ancestors_descendants_and_late_roles_preserve_bytes_but_not_siblings",
                                    "parent_components_keep_native_meaning_and_cannot_bypass_role_checks",
                                    "child_links_are_leaves_and_parent_aliases_cannot_hide_roles",
                                    "unreadable_subtrees_and_declarations_are_not_empty_evidence",
                                    "stale_cargo_plan_revalidates_roles_and_counts_only_removed_siblings",
                                    "a_declaration_lost_after_entry_observation_is_not_absence_evidence",
                                    "metadata_guards_refuse_role_aliases_without_retaining_unrelated_containers"],
        }[arguments.filter]
        skipped = {case.rsplit("::", 1)[-1] for case in receipt["skipped_test_driver_cases"]}
        required = [case for case in required if case not in skipped]
        if not all(case in content for case in required) or "test result: ok." not in content:
            result = 1
        if arguments.fault_library:
            faults = {
                "control::tests": [],
                "service::tests": ["native_client_waits_for_cold_writer_within_original_admission_budget",
                                   "native_blocked_sync_does_not_ack_early_or_replace_the_writer",
                                   "native_recovery_flushes_an_unacknowledged_pending_record_before_publication",
                                   "native_blocked_context_open_preserves_resolver_slot_and_abandons_late_capture"],
                "launch::tests": ["coordinator_death_during_grant_sync_never_releases_an_ungranted_child"],
            }.get(arguments.filter, [])
            if not all(case in content for case in faults) or "0 ignored;" not in content:
                result = 1
        caller_root = output / "caller-directories"
        if caller_root.exists():
            receipt["caller_directory_root"] = str(caller_root.resolve())
            receipt["caller_directory_device"] = caller_root.stat().st_dev
            receipt["runtime_device"] = runtime.stat().st_dev
    finally:
        # a failed launch fixture can retain an unresolved owned descendant.
        safe_to_remove = result == 0
        for path in runtime.glob("fixtures/*/*.plist"):
            data = plistlib.loads(path.read_bytes())
            expected = [str(runtime / "reap-native-test"), "--exact",
                        arguments.filter + "::native::server_process", "--nocapture"]
            if (data["ProgramArguments"] != expected or
                    data["EnvironmentVariables"]["REAP_NATIVE_CONTROL_SERVICE"] != data["Label"]):
                safe_to_remove = False
                continue
            target = f"user/{os.getuid()}/{data['Label']}"
            if launchctl("print", target).returncode == 0:
                launchctl("bootout", target)
            if launchctl("print", target).returncode != 113:
                safe_to_remove = False
            else:
                receipt["labels_absent"].append(target)
            (output / f"{data['Label']}.plist").write_bytes(path.read_bytes())
        if safe_to_remove:
            # retain regular fixture evidence, including journal records, before runtime removal.
            fixtures = runtime / "fixtures"
            for parent, directories, files in os.walk(fixtures, followlinks=False):
                for name in files:
                    path = pathlib.Path(parent) / name
                    if not stat.S_ISREG(path.lstat().st_mode):
                        continue
                    saved = output / "fixtures" / path.relative_to(fixtures)
                    saved.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(path, saved)
            shutil.rmtree(runtime)
        receipt["runtime_removed"] = not runtime.exists()
        if not receipt["runtime_removed"]:
            result = 1
        receipt["verification_exit"] = result
        (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
        print(json.dumps(receipt))
    return result


if __name__ == "__main__":
    sys.exit(main())
