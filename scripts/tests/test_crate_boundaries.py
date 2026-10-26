"""End-to-end тесты guardrail-а границ crate-ов на настоящем `cargo metadata` (без сети)."""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path

TESTS_DIR = Path(__file__).resolve().parent
SCRIPTS_DIR = TESTS_DIR.parent
SCRIPT_PATH = SCRIPTS_DIR / "check-crate-boundaries.py"
REAL_RULES_PATH = SCRIPTS_DIR / "crate-boundaries.toml"
FIXTURES_DIR = TESTS_DIR / "fixtures" / "boundaries"
FIXTURE_RULES_PATH = FIXTURES_DIR / "rules.toml"

EXIT_OK, EXIT_VIOLATIONS, EXIT_USAGE_ERROR = 0, 1, 2


def run_guardrail(*arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run([sys.executable, str(SCRIPT_PATH), *arguments],
                          capture_output=True, text=True, check=False)


class FixtureWorkspaceTestCase(unittest.TestCase):
    """База: копирует фикстуру во временный каталог (cargo пишет туда Cargo.lock, не в репо)."""

    def setUp(self) -> None:
        self._temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(self._temp_dir.cleanup)
        self.temp_path = Path(self._temp_dir.name)

    def check_fixture(self, fixture_name: str, rules_path: Path = FIXTURE_RULES_PATH
                      ) -> subprocess.CompletedProcess[str]:
        workspace_copy = self.temp_path / fixture_name
        shutil.copytree(FIXTURES_DIR / fixture_name, workspace_copy)
        return run_guardrail("--manifest-path", str(workspace_copy / "Cargo.toml"),
                             "--rules", str(rules_path), "--offline")

    def write_rules(self, text: str) -> Path:
        rules_path = self.temp_path / "custom-rules.toml"
        rules_path.write_text(text, encoding="utf-8")
        return rules_path


class DependencyGraphTests(FixtureWorkspaceTestCase):
    def test_transitive_forbidden_dependency_is_reported_with_chain_and_rule(self) -> None:
        result = self.check_fixture("forbidden")
        self.assertEqual(result.returncode, EXIT_VIOLATIONS, result.stdout + result.stderr)
        self.assertIn("camera-core → camera-contract → jni", result.stdout)
        self.assertIn("neutral-no-platform", result.stdout)

    def test_clean_graph_passes(self) -> None:
        result = self.check_fixture("allowed")
        self.assertEqual(result.returncode, EXIT_OK, result.stdout + result.stderr)
        self.assertIn("нарушений: 0", result.stdout)

    def test_android_only_target_dependency_is_not_lost(self) -> None:
        result = self.check_fixture("android-only")
        self.assertEqual(result.returncode, EXIT_VIOLATIONS, result.stdout + result.stderr)
        self.assertIn("camera-core → ndk", result.stdout)

    def test_dev_dependency_is_ignored(self) -> None:
        result = self.check_fixture("dev-only")
        self.assertEqual(result.returncode, EXIT_OK, result.stdout + result.stderr)

    def test_metadata_json_option_skips_cargo(self) -> None:
        """--metadata-json: тот же вердикт по готовому JSON, без запуска cargo."""
        workspace_copy = self.temp_path / "forbidden"
        shutil.copytree(FIXTURES_DIR / "forbidden", workspace_copy)
        metadata = subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--offline",
             "--manifest-path", str(workspace_copy / "Cargo.toml")],
            capture_output=True, text=True, check=True).stdout
        metadata_path = self.temp_path / "metadata.json"
        metadata_path.write_text(metadata, encoding="utf-8")
        result = run_guardrail("--metadata-json", str(metadata_path), "--rules", str(FIXTURE_RULES_PATH))
        self.assertEqual(result.returncode, EXIT_VIOLATIONS, result.stdout + result.stderr)
        self.assertIn("camera-core → camera-contract → jni", result.stdout)


class ClassificationTests(FixtureWorkspaceTestCase):
    def test_unclassified_workspace_crate_is_an_error_with_hint(self) -> None:
        rules_path = self.write_rules(
            'schema_version = 1\nroles = ["platform_neutral"]\n'
            '[crates]\n"camera-core" = "platform_neutral"\n')
        result = self.check_fixture("allowed", rules_path)
        self.assertEqual(result.returncode, EXIT_VIOLATIONS, result.stdout + result.stderr)
        self.assertIn("'camera-contract' не классифицирован", result.stdout)
        self.assertIn("[crates]", result.stdout)

    def test_future_crates_are_info_not_error(self) -> None:
        rules_path = self.write_rules(
            'schema_version = 1\nroles = ["platform_neutral"]\n'
            '[crates]\n"camera-core" = "platform_neutral"\n"camera-contract" = "platform_neutral"\n'
            '"isp-model" = "platform_neutral"\n')
        result = self.check_fixture("allowed", rules_path)
        self.assertEqual(result.returncode, EXIT_OK, result.stdout + result.stderr)
        self.assertIn("(будущие модули): isp-model", result.stdout)


class ConfigurationErrorTests(FixtureWorkspaceTestCase):
    def test_broken_toml_gives_exit_2(self) -> None:
        result = self.check_fixture("allowed", self.write_rules("это [не toml"))
        self.assertEqual(result.returncode, EXIT_USAGE_ERROR)
        self.assertIn("битый TOML", result.stderr)

    def test_unknown_role_in_crates_gives_exit_2(self) -> None:
        rules_path = self.write_rules(
            'schema_version = 1\nroles = ["platform_neutral"]\n[crates]\n"camera-core" = "wizard"\n')
        result = self.check_fixture("allowed", rules_path)
        self.assertEqual(result.returncode, EXIT_USAGE_ERROR)
        self.assertIn("неизвестная роль 'wizard'", result.stderr)

    def test_unknown_role_in_rule_gives_exit_2(self) -> None:
        rules_path = self.write_rules(
            'schema_version = 1\nroles = ["platform_neutral"]\n[crates]\n"camera-core" = "platform_neutral"\n'
            '[[rules]]\nid = "r"\napplies_to = ["ghost"]\nforbid = ["jni"]\nreason = "x"\n')
        result = self.check_fixture("allowed", rules_path)
        self.assertEqual(result.returncode, EXIT_USAGE_ERROR)
        self.assertIn("неизвестная роль 'ghost'", result.stderr)

    def test_missing_manifest_gives_exit_2(self) -> None:
        result = run_guardrail("--manifest-path", str(self.temp_path / "nope" / "Cargo.toml"),
                               "--rules", str(FIXTURE_RULES_PATH))
        self.assertEqual(result.returncode, EXIT_USAGE_ERROR)
        self.assertIn("не найден манифест", result.stderr)

    def test_failing_cargo_metadata_gives_exit_2(self) -> None:
        broken_manifest = self.temp_path / "Cargo.toml"
        broken_manifest.write_text("[package\n", encoding="utf-8")
        result = run_guardrail("--manifest-path", str(broken_manifest),
                               "--rules", str(FIXTURE_RULES_PATH), "--offline")
        self.assertEqual(result.returncode, EXIT_USAGE_ERROR)
        self.assertIn("cargo metadata завершился", result.stderr)


class SourceRuleTests(unittest.TestCase):
    """Правила по исходникам: `extern "C"` разрешён только в одном файле crate-а."""

    FFI_FILE = "src/android/hardware_buffer_ffi.rs"
    RULES = (
        'schema_version = 1\nroles = ["gpu"]\n[crates]\n"gpu-vulkan" = "gpu"\n'
        '[[source_rules]]\nid = "ffi-only-in-safety-module"\ncrate = "gpu-vulkan"\n'
        'pattern = \'extern "C"\'\nallowed_files = ["src/android/hardware_buffer_ffi.rs"]\n'
        'reason = "один safety-модуль"\n'
    )

    def setUp(self) -> None:
        self._temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(self._temp_dir.cleanup)
        self.root = Path(self._temp_dir.name)
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers = ["gpu-vulkan"]\nresolver = "3"\n', encoding="utf-8")
        crate = self.root / "gpu-vulkan"
        (crate / "src" / "android").mkdir(parents=True)
        (crate / "Cargo.toml").write_text(
            '[package]\nname = "gpu-vulkan"\nversion = "0.1.0"\nedition = "2024"\n', encoding="utf-8")
        (crate / "src" / "lib.rs").write_text("pub fn clean() {}\n", encoding="utf-8")
        self.rules_path = self.root / "rules.toml"
        self.rules_path.write_text(self.RULES, encoding="utf-8")

    def write_source(self, relative_path: str, text: str) -> None:
        path = self.root / "gpu-vulkan" / relative_path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")

    def check(self) -> subprocess.CompletedProcess[str]:
        return run_guardrail("--manifest-path", str(self.root / "Cargo.toml"),
                             "--rules", str(self.rules_path), "--offline")

    def test_pattern_outside_allowed_file_is_reported_with_file_and_line(self) -> None:
        self.write_source("src/sneaky.rs", 'fn a() {}\n\nunsafe extern "C" {\n    fn bad();\n}\n')
        result = self.check()
        self.assertEqual(result.returncode, EXIT_VIOLATIONS, result.stdout + result.stderr)
        self.assertIn("gpu-vulkan/src/sneaky.rs:3:", result.stdout)
        self.assertIn("ffi-only-in-safety-module", result.stdout)

    def test_pattern_in_allowed_file_passes(self) -> None:
        self.write_source(self.FFI_FILE, 'unsafe extern "C" {\n    fn good();\n}\n')
        result = self.check()
        self.assertEqual(result.returncode, EXIT_OK, result.stdout + result.stderr)
        self.assertIn("нарушений: 0", result.stdout)

    def test_pattern_in_line_comment_is_ignored_but_in_code_before_comment_is_not(self) -> None:
        self.write_source("src/docs.rs", '// extern "C" упомянут в комментарии\n/// extern "C" в доке\n')
        self.assertEqual(self.check().returncode, EXIT_OK)
        self.write_source("src/trailing.rs", 'unsafe extern "C" {} // хвостовой комментарий\n')
        result = self.check()
        self.assertEqual(result.returncode, EXIT_VIOLATIONS, result.stdout)
        self.assertIn("gpu-vulkan/src/trailing.rs:1:", result.stdout)

    def test_pattern_in_tests_dir_is_also_scanned(self) -> None:
        self.write_source("tests/ffi.rs", 'extern "C" fn callback() {}\n')
        result = self.check()
        self.assertEqual(result.returncode, EXIT_VIOLATIONS, result.stdout + result.stderr)
        self.assertIn("gpu-vulkan/tests/ffi.rs:1:", result.stdout)

    def test_other_abi_strings_are_not_matched(self) -> None:
        self.write_source("src/callback.rs", 'unsafe extern "system" fn callback() {}\n')
        self.assertEqual(self.check().returncode, EXIT_OK)

    def rules_with(self, extra: str) -> None:
        self.rules_path.write_text(
            'schema_version = 1\nroles = ["gpu"]\n[crates]\n"gpu-vulkan" = "gpu"\n' + extra,
            encoding="utf-8")

    def test_unknown_key_in_source_rule_is_config_error(self) -> None:
        self.rules_with('[[source_rules]]\nid = "r"\ncrate = "gpu-vulkan"\npattern = "x"\n'
                        'reason = "y"\nallow_files = ["a.rs"]\n')
        result = self.check()
        self.assertEqual(result.returncode, EXIT_USAGE_ERROR, result.stdout)
        self.assertIn("allow_files", result.stderr)

    def test_unknown_crate_in_source_rule_is_config_error(self) -> None:
        self.rules_with('[[source_rules]]\nid = "r"\ncrate = "ghost"\npattern = "x"\nreason = "y"\n')
        result = self.check()
        self.assertEqual(result.returncode, EXIT_USAGE_ERROR, result.stdout)
        self.assertIn("'ghost'", result.stderr)

    def test_missing_pattern_and_escaping_allowed_path_are_config_errors(self) -> None:
        self.rules_with('[[source_rules]]\nid = "r"\ncrate = "gpu-vulkan"\nreason = "y"\n')
        result = self.check()
        self.assertEqual(result.returncode, EXIT_USAGE_ERROR)
        self.assertIn("pattern", result.stderr)
        self.rules_with('[[source_rules]]\nid = "r"\ncrate = "gpu-vulkan"\npattern = "x"\n'
                        'reason = "y"\nallowed_files = ["../outside.rs"]\n')
        result = self.check()
        self.assertEqual(result.returncode, EXIT_USAGE_ERROR)
        self.assertIn("allowed_files", result.stderr)

    def test_real_rules_file_forbids_extern_c_everywhere_but_ffi_module(self) -> None:
        document = tomllib.loads(REAL_RULES_PATH.read_text(encoding="utf-8"))
        rules = {rule["id"]: rule for rule in document["source_rules"]}
        rule = rules["gpu-ffi-only-in-safety-module"]
        self.assertEqual(rule["crate"], "gpu-vulkan")
        self.assertEqual(rule["pattern"], 'extern "C"')
        self.assertEqual(rule["allowed_files"], [self.FFI_FILE])


class RealRulesFileTests(unittest.TestCase):
    def setUp(self) -> None:
        self.document = tomllib.loads(REAL_RULES_PATH.read_text(encoding="utf-8"))

    def test_roles_in_crates_and_rules_are_known(self) -> None:
        known_roles = set(self.document["roles"])
        self.assertTrue(set(self.document["crates"].values()) <= known_roles)
        for rule in self.document["rules"]:
            self.assertTrue(set(rule.get("applies_to", [])) <= known_roles, rule["id"])

    def test_key_crates_are_classified(self) -> None:
        crates = self.document["crates"]
        self.assertEqual(crates["fasticamera-app"], "composition_root")
        self.assertEqual(crates["camera-contract"], "platform_neutral")

    def test_real_rules_file_passes_the_checker_on_fixture_graph(self) -> None:
        """Настоящий файл правил принимается скриптом (код не 2) на фейковом графе."""
        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_copy = Path(temp_dir) / "forbidden"
            shutil.copytree(FIXTURES_DIR / "forbidden", workspace_copy)
            result = run_guardrail("--manifest-path", str(workspace_copy / "Cargo.toml"),
                                   "--rules", str(REAL_RULES_PATH), "--offline")
        self.assertEqual(result.returncode, EXIT_VIOLATIONS, result.stdout + result.stderr)
        self.assertIn("camera-core → camera-contract → jni", result.stdout)
        self.assertIn("neutral-no-platform", result.stdout)
        self.assertIn("camera-contract → jni: запрещено правилом neutral-no-platform", result.stdout)

    def test_contract_depending_on_another_workspace_crate_violates_leaf_rule(self) -> None:
        """Настоящее правило contract-is-leaf: словарь не зависит от других crate-ов проекта."""
        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_copy = Path(temp_dir) / "contract-not-leaf"
            shutil.copytree(FIXTURES_DIR / "contract-not-leaf", workspace_copy)
            result = run_guardrail("--manifest-path", str(workspace_copy / "Cargo.toml"),
                                   "--rules", str(REAL_RULES_PATH), "--offline")
        self.assertEqual(result.returncode, EXIT_VIOLATIONS, result.stdout + result.stderr)
        self.assertIn("camera-contract → isp-model: запрещено правилом contract-is-leaf", result.stdout)


if __name__ == "__main__":
    unittest.main()
