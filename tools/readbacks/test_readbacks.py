import hashlib
import json
from pathlib import Path
import tempfile
import shutil
import unittest
from unittest.mock import patch
import subprocess
import sys

import readbacks
import ci


BASE = "a" * 40
HEAD = "b" * 40


class CaptureCase(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.inventory = {"schema": 1, "configurations": ["default", "debug-ui", "all-features"],
                          "rows": [{"id": "quad", "stage": "final", "width": 1, "height": 1,
                                    "blend_path": "primary", "configurations": ["default", "debug-ui", "all-features"]}]}
        self.base = self.capture("base", BASE, "10")
        self.head = self.capture("head", HEAD, "20")

    def capture(self, name, source, run):
        directory = self.root / name
        directory.mkdir()
        rows = []
        for config in self.inventory["configurations"]:
            path = directory / f"{config}.bin"
            path.write_bytes(bytes([17, 33, 65, 255]))
            input_path = directory / f"{config}.input.bin"
            input_path.write_bytes(b"literal scene")
            rows.append({"id": "quad", "configuration": config, "stage": "final", "role": "render",
                         "blend_path": "primary", "test_id": "tests::goldens::quad", "width": 1,
                         "height": 1, "channels": "RGBA", "file": path.name,
                         "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                         "input_file": input_path.name, "input_sha256": hashlib.sha256(b"literal scene").hexdigest(),
                         "adapter": {"name": "WARP", "backend": "Dx12", "driver": "test",
                                     "vendor": 5140, "device": 140, "device_type": "Cpu", "driver_info": "test"}})
        manifest = {"schema": 1, "source_sha": source, "tested_sha": source, "base_sha": BASE,
                    "run_id": run, "run_attempt": "1", "workflow": "validate", "harness_sha256": "d" * 64,
                    "inventory_sha256": readbacks.json_hash(self.inventory),
                    "runtime": {"os": "Windows", "compiler": "staticdxc", "rustc": "test",
                                "backend": "dx12", "test_threads": "1", "dependencies": "e" * 64},
                    "configurations": {c: {"exit_code": 0, "command": ["cargo", "test"]}
                                       for c in self.inventory["configurations"]}, "rows": rows}
        (directory / "manifest.json").write_text(json.dumps(manifest))
        return directory

    def compare(self):
        return readbacks.compare(self.base, self.head, self.inventory, BASE, HEAD, "10", "20")

    def mutate(self, directory, change):
        path = directory / "manifest.json"
        manifest = json.loads(path.read_text())
        change(manifest)
        path.write_text(json.dumps(manifest))


class ConfigurationGates(CaptureCase):
    def test_sections_partition_frozen_and_supplemental_requirements(self):
        checkout = Path(__file__).resolve().parents[2]
        original = json.dumps(ci.INVENTORY, sort_keys=True)
        keys, gpu_configs, replacement_configs = [], [], []
        for config in ci.COMMANDS:
            for section in ci.windows_sections(checkout, config):
                inventory = ci.configuration_inventory(config, section)
                keys.extend(readbacks.expected_rows(inventory))
                gpu_configs.extend(inventory["gpu_decode_configurations"])
                replacement_configs.extend(inventory["replacement_configurations"])
        self.assertCountEqual(keys, readbacks.expected_rows(ci.INVENTORY))
        self.assertCountEqual(gpu_configs, ["default", "debug-ui", "all-features"])
        self.assertEqual(replacement_configs, ["all-features"])
        self.assertEqual(json.dumps(ci.INVENTORY, sort_keys=True), original)

    def test_section_without_rows_still_rejects_failed_tests(self):
        inventory = ci.configuration_inventory("default", "guest")
        for directory in [self.base, self.head]:
            self.mutate(directory, lambda m: m.update(
                inventory_sha256=readbacks.json_hash(inventory), rows=[],
                configurations={"default": {"exit_code": 0}}))
        self.assertTrue(readbacks.compare(self.base, self.head, inventory, BASE, HEAD, "10", "20")["passed"])
        self.mutate(self.head, lambda m: m["configurations"]["default"].update(exit_code=101))
        report = readbacks.compare(self.base, self.head, inventory, BASE, HEAD, "10", "20", allow_incomplete=True)
        self.assertFalse(report["passed"])
        self.assertIn("failed configuration: default", report["failures"]["candidate"][0])

    def test_selections_partition_every_frozen_row_without_changing_inventory(self):
        original = json.dumps(ci.INVENTORY, sort_keys=True)
        keys = set()
        for config in ["default", "debug-ui", "all-features"]:
            inventory = ci.configuration_inventory(config)
            self.assertEqual(inventory["configurations"], [config])
            rows = readbacks.expected_rows(inventory)
            self.assertEqual(len(rows), {"default": 56, "debug-ui": 56, "all-features": 59}[config])
            self.assertFalse(keys & rows.keys())
            keys.update(rows)
        self.assertEqual(keys, readbacks.expected_rows(ci.INVENTORY).keys())
        self.assertEqual(json.dumps(ci.INVENTORY, sort_keys=True), original)

    def test_selected_gate_downloads_or_bootstraps_only_its_configuration(self):
        for config in ["default", "debug-ui", "all-features"]:
            for section, scenario in [(section, scenario) for section in [None, "readbacks", "guest"]
                                      for scenario in ["download", "bootstrap", "missing-row", "failed-test", "changed-byte", "missing-configuration"]
                                      if section != "guest" or scenario not in ["missing-row", "changed-byte"]]:
                with self.subTest(config=config, section=section, scenario=scenario):
                    output = self.root / f"{config}-{section}-{scenario}"
                    name = f"readbacks-windows-latest-{config}" + (f"-{section}" if section else "")
                    captured = []

                    def capture(checkout, destination, base_sha, harness_checkout, inventory):
                        captured.append(checkout)
                        shutil.copytree(checkout, destination)
                        manifest = json.loads((destination / "manifest.json").read_text())
                        manifest["inventory_sha256"] = readbacks.json_hash(inventory)
                        manifest["configurations"] = {c: manifest["configurations"][c] for c in inventory["configurations"]}
                        keys = readbacks.expected_rows(inventory)
                        manifest["rows"] = [r for r in manifest["rows"] if (r["configuration"], r["id"]) in keys]
                        if checkout == self.head:
                            if scenario == "missing-row":
                                manifest["rows"].clear()
                            elif scenario == "failed-test":
                                manifest["configurations"][config]["exit_code"] = 101
                            elif scenario == "missing-configuration":
                                manifest["configurations"].clear()
                            elif scenario == "changed-byte":
                                path = destination / f"{config}.bin"
                                path.write_bytes(bytes([17, 33, 65, 254]))
                                manifest["rows"][0]["sha256"] = readbacks.file_hash(path)
                        (destination / "manifest.json").write_text(json.dumps(manifest))
                        return manifest

                    def api(endpoint):
                        self.assertIn(f"head_sha={BASE}", endpoint)
                        return {"workflow_runs": [{"id": 10, "head_sha": BASE, "conclusion": "success", "event": "push"}]}

                    def download(repository, run_id, name, destination):
                        expected_name = f"readbacks-windows-latest-{config}" + (f"-{section}" if section else "")
                        self.assertEqual((repository, run_id, name), ("owner/repo", "10", expected_name))
                        if scenario != "download":
                            raise ValueError("missing base artifact")
                        capture(self.base, destination, BASE, self.head, ci.configuration_inventory(config, section))

                    arguments = ["ci.py", "--checkout", str(self.head), "--base-checkout", str(self.base),
                                 "--output", str(output), "--base-sha", BASE, "--repository", "owner/repo",
                                 "--artifact-name", name, "--configuration", config]
                    if section:
                        arguments.extend(["--section", section])
                    with patch.object(sys, "argv", arguments), patch.object(ci, "INVENTORY", self.inventory), \
                            patch.object(ci, "capture", capture), patch.object(ci, "overlay", return_value={}), \
                            patch.object(ci, "gh_json", api), patch.object(ci, "download_artifact", download):
                        result = ci.main()
                    self.assertEqual(result, 0 if scenario in ["download", "bootstrap"] else 1)
                    report = json.loads((output / "comparison.json").read_text())
                    if scenario == "missing-configuration":
                        self.assertIn("configuration inventory differs", report["error"])
                    else:
                        self.assertEqual(report["threshold"], 0)
                    self.assertEqual(report["passed"], result == 0)
                    self.assertTrue(all(row["configuration"] == config for row in report.get("rows", [])))
                    for directory in [output / "candidate", output / ("downloaded-base" if scenario == "download" else "bootstrap-base")]:
                        manifest = json.loads((directory / "manifest.json").read_text())
                        self.assertEqual(list(manifest["configurations"]), [] if scenario == "missing-configuration" and directory.name == "candidate" else [config])
                    if scenario != "download":
                        self.assertEqual(captured, [self.head, self.base])
                        self.assertTrue((output / "bootstrap-reason.json").is_file())

    def test_unknown_configuration_is_rejected_before_capture(self):
        arguments = ["ci.py", "--checkout", str(self.head), "--output", str(self.root / "invalid"),
                     "--base-sha", BASE, "--artifact-name", "readbacks", "--configuration", "missing"]
        with patch.object(sys, "argv", arguments), self.assertRaises(SystemExit) as error:
            ci.main()
        self.assertEqual(error.exception.code, 2)
        self.assertFalse((self.root / "invalid").exists())


class ExactReadbacks(CaptureCase):
    def test_partial_report_compares_available_rows_but_cannot_pass(self):
        self.mutate(self.head, lambda m: m["configurations"]["all-features"].update(
            exit_code=101, failed_tests=["tmem_layouts"]))
        self.mutate(self.head, lambda m: m["rows"].pop())
        report = readbacks.compare(self.base, self.head, self.inventory, BASE, HEAD, "10", "20",
                                   allow_incomplete=True)
        self.assertFalse(report["passed"])
        self.assertFalse(report["complete"])
        self.assertEqual(len(report["rows"]), 2)
        self.assertEqual(report["missing_rows"]["candidate"], [("all-features", "quad")])
        self.assertIn("tmem_layouts", report["failures"]["candidate"][0])

    def test_partial_comparison_still_rejects_corrupted_evidence(self):
        self.mutate(self.head, lambda m: m["configurations"]["default"].update(exit_code=101))
        (self.head / "default.input.bin").write_bytes(b"corrupt")
        with self.assertRaisesRegex(ValueError, "input_sha256"):
            readbacks.compare(self.base, self.head, self.inventory, BASE, HEAD, "10", "20",
                              allow_incomplete=True)

    def test_partial_capture_with_all_rows_still_fails(self):
        self.mutate(self.base, lambda m: m["configurations"]["default"].update(exit_code=101))
        report = readbacks.compare(self.base, self.head, self.inventory, BASE, HEAD, "10", "20",
                                   allow_incomplete=True)
        self.assertFalse(report["passed"])
        self.assertEqual(len(report["rows"]), 3)

    def test_gate_captures_both_sides_and_reports_the_failing_test(self):
        for failed_side in [self.head, self.base]:
            with self.subTest(failed_side=failed_side):
                original = (failed_side / "manifest.json").read_text()
                self.mutate(failed_side, lambda m: m["configurations"]["all-features"].update(
                    exit_code=101, failed_tests=["tmem_layouts"], test_log="all-features/test.log"))
                self.mutate(failed_side, lambda m: m["rows"].pop())
                output = self.root / ("gate-" + failed_side.name)
                captured = []

                def capture(checkout, destination, base_sha, harness_checkout, inventory):
                    captured.append(checkout)
                    shutil.copytree(checkout, destination)
                    return json.loads((destination / "manifest.json").read_text())

                arguments = ["ci.py", "--checkout", str(self.head), "--base-checkout", str(self.base),
                             "--output", str(output), "--base-sha", BASE,
                             "--artifact-name", "readbacks-windows-latest"]
                with patch.object(sys, "argv", arguments), patch.object(ci, "INVENTORY", self.inventory), \
                        patch.object(ci, "capture", capture), patch.object(ci, "overlay", return_value={}), \
                        patch.object(ci, "fetch_base", side_effect=ValueError("missing base artifact")):
                    self.assertEqual(ci.main(), 1)
                self.assertEqual(captured, [self.head, self.base])
                report = json.loads((output / "comparison.json").read_text())
                self.assertFalse(report["passed"])
                self.assertFalse(report["complete"])
                self.assertEqual(len(report["rows"]), 2)
                failure = json.loads((output / "failure.json").read_text())["error"]
                self.assertIn("all-features", failure)
                self.assertIn("tmem_layouts", failure)
                self.assertIn("all-features/test.log", failure)
                (failed_side / "manifest.json").write_text(original)

    def test_identical_parent_capture_passes_all_configurations(self):
        report = self.compare()
        self.assertTrue(report["passed"])
        self.assertEqual(len(report["rows"]), 3)

    def test_alpha_only_byte_mutation_fails_threshold_zero(self):
        (self.head / "default.bin").write_bytes(bytes([17, 33, 65, 254]))
        self.mutate(self.head, lambda m: m["rows"][0].update(
            sha256=hashlib.sha256(bytes([17, 33, 65, 254])).hexdigest()))
        report = self.compare()
        self.assertFalse(report["passed"])
        self.assertEqual(report["rows"][0]["changed_bytes"], 1)
        self.assertEqual(report["rows"][0]["changed_pixels"], 1)
        self.assertEqual(report["rows"][0]["channel_maxima"], [0, 0, 0, 1])

    def test_missing_row_cannot_be_a_passing_skip(self):
        self.mutate(self.head, lambda m: m["rows"].pop())
        with self.assertRaisesRegex(ValueError, "missing rows"):
            self.compare()

    def test_missing_file_fails_even_when_manifest_lists_row(self):
        (self.head / "default.bin").unlink()
        with self.assertRaisesRegex(ValueError, "missing file"):
            self.compare()

    def test_truncated_rgba_row_fails(self):
        (self.head / "default.bin").write_bytes(bytes([17, 33, 65]))
        with self.assertRaisesRegex(ValueError, "byte length"):
            self.compare()

    def test_stale_base_sha_fails(self):
        self.mutate(self.base, lambda m: m.update(source_sha="f" * 40))
        with self.assertRaisesRegex(ValueError, "source SHA"):
            self.compare()

    def test_candidate_for_another_base_cannot_pass(self):
        self.mutate(self.head, lambda m: m.update(base_sha="f" * 40))
        with self.assertRaisesRegex(ValueError, "base SHA"):
            self.compare()

    def test_wrong_run_id_fails(self):
        self.mutate(self.base, lambda m: m.update(run_id="9"))
        with self.assertRaisesRegex(ValueError, "run ID"):
            self.compare()

    def test_overwritten_configuration_fails(self):
        self.mutate(self.head, lambda m: m["rows"][2].update(configuration="default"))
        with self.assertRaisesRegex(ValueError, "duplicate row"):
            self.compare()

    def test_failed_test_command_cannot_pass_with_complete_files(self):
        self.mutate(self.head, lambda m: m["configurations"]["default"].update(exit_code=101))
        with self.assertRaisesRegex(ValueError, "failed configuration"):
            self.compare()

    def test_missing_configuration_fails(self):
        self.mutate(self.head, lambda m: m["configurations"].pop("debug-ui"))
        with self.assertRaisesRegex(ValueError, "configuration inventory"):
            self.compare()

    def test_rgb_only_comparison_policy_is_rejected(self):
        self.mutate(self.head, lambda m: m["rows"][0].update(channels="RGB"))
        with self.assertRaisesRegex(ValueError, "RGBA"):
            self.compare()

    def test_input_or_adapter_mismatch_requires_recapture(self):
        for field, value in [("input_sha256", "f" * 64), ("adapter", {"name": "different"})]:
            with self.subTest(field=field):
                original = (self.head / "manifest.json").read_text()
                self.mutate(self.head, lambda m: m["rows"][0].update({field: value}))
                with self.assertRaisesRegex(ValueError, field):
                    self.compare()
                (self.head / "manifest.json").write_text(original)

    def test_file_hash_detects_corrupted_artifact(self):
        (self.head / "default.bin").write_bytes(bytes([17, 33, 65, 254]))
        with self.assertRaisesRegex(ValueError, "file hash"):
            self.compare()

    def test_path_traversal_is_rejected(self):
        self.mutate(self.head, lambda m: m["rows"][0].update(file="../base/default.bin"))
        with self.assertRaisesRegex(ValueError, "unsafe file"):
            self.compare()

    def test_decode_rows_require_independent_expected_bytes(self):
        self.inventory["rows"][0].update(stage="decode", blend_path="none")
        for directory in [self.base, self.head]:
            def change(manifest):
                manifest["inventory_sha256"] = readbacks.json_hash(self.inventory)
                for row in manifest["rows"]:
                    expected = directory / (row["configuration"] + ".expected.bin")
                    expected.write_bytes(bytes([17, 33, 65, 255]))
                    row.update(stage="decode", role="cpu-oracle", blend_path="none", adapter=None,
                               expected_file=expected.name, expected_sha256=hashlib.sha256(expected.read_bytes()).hexdigest())
            self.mutate(directory, change)
        self.assertTrue(self.compare()["passed"])
        (self.head / "default.expected.bin").write_bytes(bytes([17, 33, 65, 254]))
        with self.assertRaisesRegex(ValueError, "independent literal"):
            self.compare()

    def test_corrupt_input_bytes_cannot_claim_same_input_hash(self):
        (self.head / "default.input.bin").write_bytes(b"other scene")
        with self.assertRaisesRegex(ValueError, "input_sha256"):
            self.compare()

    def test_missing_provenance_is_rejected(self):
        self.mutate(self.head, lambda m: m.update(harness_sha256=""))
        with self.assertRaisesRegex(ValueError, "harness_sha256"):
            self.compare()

    def test_collector_hashes_actual_rgba_and_input_files(self):
        directory = self.root / "collected" / "default" / "decode"
        directory.mkdir(parents=True)
        (directory / "tile.json").write_text(json.dumps({"id": "tile", "stage": "decode"}))
        (directory / "tile.bin").write_bytes(bytes([1, 2, 3, 4]))
        (directory / "tile.input.bin").write_bytes(b"input")
        (directory / "tile.expected.bin").write_bytes(bytes([1, 2, 3, 4]))
        rows, errors = ci.collect_rows(self.root / "collected")
        self.assertEqual(errors, [])
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["sha256"], "9f64a747e1b97f131fabb6b447296c9b6f0201e79fb3c5356e6c77e89b6a806a")
        self.assertEqual(rows[0]["input_sha256"], "c96c6d5be8d08a12e7b5cdc1b207fa6b2430974c86803d8891675e76fd992c20")
        self.assertEqual(rows[0]["file"], "default/decode/tile.bin")

    def test_collector_records_missing_literal_as_capture_error(self):
        directory = self.root / "collected" / "default" / "decode"
        directory.mkdir(parents=True)
        (directory / "tile.json").write_text(json.dumps({"id": "tile", "stage": "decode"}))
        (directory / "tile.bin").write_bytes(bytes([1, 2, 3, 4]))
        (directory / "tile.input.bin").write_bytes(b"input")
        rows, errors = ci.collect_rows(self.root / "collected")
        self.assertEqual(rows, [])
        self.assertIn("tile.expected.bin", errors[0])

    def test_bootstrap_replaces_only_dev_dependency_section(self):
        original = '[package]\nname="base"\n[dependencies]\nwgpu="29"\n[dev-dependencies]\npng="0.18"\n[features]\na=[]\n'
        candidate = '[package]\nname="candidate"\n[dependencies]\nwgpu="30"\n[dev-dependencies]\npng="0.18"\nserde_json="1"\n[features]\na=["changed"]\n'
        self.assertEqual(ci.replace_dev_dependencies(original, candidate),
                         '[package]\nname="base"\n[dependencies]\nwgpu="29"\n[dev-dependencies]\npng="0.18"\nserde_json="1"\n[features]\na=[]\n')

    def test_fresh_checkout_generates_and_retains_one_lockfile(self):
        checkout = self.root / "fresh"
        checkout.mkdir()
        def cargo(command, cwd):
            self.assertEqual(command, ["cargo", "generate-lockfile"])
            (Path(cwd) / "Cargo.lock").write_bytes(b"frozen lock")
        with patch.object(ci.subprocess, "check_call", cargo):
            ci.freeze_lock(checkout)
        self.assertEqual((checkout / "Cargo.lock").read_bytes(), b"frozen lock")
        with patch.object(ci.subprocess, "check_call", side_effect=AssertionError("must retain existing lock")):
            ci.freeze_lock(checkout)

    def test_command_line_reports_a_missing_row_as_failure(self):
        self.mutate(self.head, lambda m: m["rows"].pop())
        inventory = self.root / "inventory.json"
        inventory.write_text(json.dumps(self.inventory))
        report = self.root / "report.json"
        result = subprocess.run([sys.executable, str(Path(readbacks.__file__)),
                                 "--base", str(self.base), "--candidate", str(self.head),
                                 "--base-sha", BASE, "--candidate-sha", HEAD,
                                 "--base-run-id", "10", "--candidate-run-id", "20",
                                 "--inventory", str(inventory), "--output", str(report)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 1)
        self.assertFalse(json.loads(report.read_text())["passed"])

    def test_hardware_adapter_cannot_claim_the_warp_cell(self):
        for directory in [self.base, self.head]:
            self.mutate(directory, lambda m: m["rows"][0]["adapter"].update(device_type="DiscreteGpu"))
        with self.assertRaisesRegex(ValueError, "WARP"):
            self.compare()

    def test_both_manifests_using_fxc_cannot_pass_warp(self):
        for directory in [self.base, self.head]:
            self.mutate(directory, lambda m: m["runtime"].update(compiler="fxc"))
        with self.assertRaisesRegex(ValueError, "staticdxc"):
            self.compare()


if __name__ == "__main__":
    unittest.main()
