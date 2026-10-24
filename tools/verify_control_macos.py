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
                                            "lifetime::tests", "archive::tests",
                                            "quarantine::tests", "util::tests"], default="control::tests")
    parser.add_argument("--fault-library", type=pathlib.Path)
    arguments = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("the native macOS runtime is required")

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
            "DYLD_INSERT_LIBRARIES",
        ):
            environment.pop(name, None)
        environment["REAP_NATIVE_CONTROL_FIXTURE_ROOT"] = str(runtime / "fixtures")
        environment["REAP_NATIVE_CAPTURE_CWD_ROOT"] = str(output / "caller-directories")
        selection = [str(executable), arguments.filter, "--test-threads=1", "--nocapture"]
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
                cwd=output if arguments.filter == "archive::tests" else None,
                stdout=log, stderr=subprocess.STDOUT, timeout=60,
            )
        receipt["native_exit"] = run.returncode
        result = run.returncode
        content = (output / "native.log").read_text()
        required = {
            "control::tests": ["native_control_survives_a_real_filesystem_worker_stall",
                               "native_confirmation_requires_the_original_live_client",
                               "native_capture_pins_actual_cwd_after_client_exit_and_path_replacement"],
            "service::tests": ["native_admission_survives_control_restart_and_writer_process_death",
                               "native_alias_writer_cannot_replace_the_locked_owner",
                               "malformed_native_requests_do_not_poison_the_writer",
                               "native_resolved_admission_binds_caller_namespace_and_replays_after_ticket_expiry",
                               "native_invalid_namespaces_and_untrusted_receipts_never_create_jobs",
                               "native_prepare_capacity_recovers_after_idle_ticket_expiry"],
            "launch::tests": ["unresolved_launch_reserves_all_volumes_across_restart_without_blocking_other_volumes",
                              "ungranted_child_exits_without_effects_and_ambiguous_intent_is_not_replaced",
                              "durable_grant_survives_writer_restart_and_release_requires_native_exit",
                              "two_helpers_remain_owned_and_retain_worker_slots_until_all_exits_are_proved",
                              "sequential_resolution_and_bulk_launches_never_replay_a_retired_launch_sequence",
                              "retired_helper_reservations_cannot_create_another_process_after_restart",
                              "grant_helper_and_exit_retries_refuse_replaced_journal_bindings"],
            "lifetime::tests": ["native_writer_requires_inspection_and_publishes_bound_identity",
                                "native_rename_preserves_grace_and_permission_copy_and_replacement_do_not",
                                "honored_children_leave_group_accounting_and_unbound_children_block_caps"],
            "archive::tests": ["durable_commit_replay_and_independent_retrieval_preserve_sources_and_identity",
                               "synchronization_faults_never_acknowledge_an_incomplete_commit",
                               "missing_dependencies_and_omitted_snapshot_units_cannot_publish"],
            "quarantine::tests": ["activity_stream_retains_a_late_protection_path_without_retaining_host_wide_output",
                                  "activity_stream_requires_complete_valid_bounded_fields_before_a_negative_guard",
                                  "scratch_retire_moves_to_quarantine_with_owner"],
            "util::tests": ["guard_probe_refuses_combined_output_above_the_capture_bound",
                            "streamed_probe_consumes_complete_evidence_above_the_diagnostic_capture_bound"],
        }[arguments.filter]
        if not all(case in content for case in required) or "test result: ok." not in content:
            result = 1
        if arguments.fault_library:
            faults = {
                "control::tests": [],
                "service::tests": ["native_blocked_sync_does_not_ack_early_or_replace_the_writer",
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
        safe_to_remove = not (arguments.filter == "launch::tests" and result != 0)
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
