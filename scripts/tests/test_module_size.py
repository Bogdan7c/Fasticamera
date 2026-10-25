"""Тесты ratchet размера модулей: запускаем настоящий скрипт на временных деревьях."""

from __future__ import annotations

import subprocess
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
SCRIPT_PATH = SCRIPTS_DIR / "module-size-check.py"
REAL_POLICY_PATH = SCRIPTS_DIR / "module-size-policy.toml"

EXIT_OK, EXIT_VIOLATIONS, EXIT_USAGE_ERROR = 0, 1, 2

POLICY_HEAD = """schema_version = 1
soft_limit_lines = 700
hard_limit_lines = 800
roots = ["crates", "tools"]
extensions = [".rs", ".py"]
exclude_dir_names = ["target", "build", "fixtures"]

# комментарий выше baseline должен пережить переписывание
"""


class SizeCheckTestCase(unittest.TestCase):
    def setUp(self) -> None:
        self._temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(self._temp_dir.cleanup)
        self.root = Path(self._temp_dir.name)
        self.policy_path = self.root / "policy.toml"
        self.write_policy({})

    def write_policy(self, baseline: dict[str, int]) -> None:
        entries = "".join(f'"{path}" = {lines}\n' for path, lines in baseline.items())
        self.policy_path.write_text(POLICY_HEAD + "[baseline]\n" + entries, encoding="utf-8")

    def make_file(self, relative_path: str, lines: int) -> None:
        target = self.root / relative_path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text("// строка\n" * lines, encoding="utf-8")

    def run_check(self, *extra_arguments: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPT_PATH), "--root", str(self.root),
             "--policy", str(self.policy_path), *extra_arguments],
            capture_output=True, text=True, check=False)

    def read_baseline(self) -> dict[str, int]:
        return tomllib.loads(self.policy_path.read_text(encoding="utf-8"))["baseline"]


class LimitTests(SizeCheckTestCase):
    def test_file_over_hard_limit_is_error_with_numbers(self) -> None:
        self.make_file("crates/x/src/a.rs", 801)
        result = self.run_check()
        self.assertEqual(result.returncode, EXIT_VIOLATIONS)
        self.assertIn("crates/x/src/a.rs: 801 строка > жёсткий предел 800", result.stdout)

    def test_file_exactly_at_hard_limit_is_fine(self) -> None:
        self.make_file("crates/x/src/a.rs", 800)
        self.assertEqual(self.run_check().returncode, EXIT_OK)

    def test_file_between_limits_is_warning_only(self) -> None:
        self.make_file("crates/x/src/a.rs", 750)
        result = self.run_check()
        self.assertEqual(result.returncode, EXIT_OK)
        self.assertIn("crates/x/src/a.rs: 750 строк > мягкий предел 700", result.stdout)
        self.assertIn("предупреждений: 1, ошибок: 0", result.stdout)

    def test_last_line_without_newline_is_counted(self) -> None:
        target = self.root / "crates/x/src/a.rs"
        target.parent.mkdir(parents=True)
        target.write_text("x\n" * 800 + "последняя без перевода", encoding="utf-8")
        self.assertEqual(self.run_check().returncode, EXIT_VIOLATIONS)

    def test_excluded_directories_and_extensions_are_not_scanned(self) -> None:
        self.make_file("crates/x/target/huge.rs", 5000)
        self.make_file("crates/x/build/huge.rs", 5000)
        self.make_file("crates/x/tests/fixtures/huge.rs", 5000)
        self.make_file("crates/x/notes.md", 5000)
        result = self.run_check()
        self.assertEqual(result.returncode, EXIT_OK, result.stdout)
        self.assertIn("Просмотрено файлов: 0", result.stdout)

    def test_missing_roots_are_skipped_silently(self) -> None:
        result = self.run_check()  # ни crates/, ни tools/ нет
        self.assertEqual(result.returncode, EXIT_OK)
        self.assertEqual(result.stderr, "")


class BaselineRatchetTests(SizeCheckTestCase):
    LEGACY = "crates/x/src/legacy.rs"

    def test_legacy_file_growth_is_error(self) -> None:
        self.write_policy({self.LEGACY: 900})
        self.make_file(self.LEGACY, 901)
        result = self.run_check()
        self.assertEqual(result.returncode, EXIT_VIOLATIONS)
        self.assertIn("legacy-файл вырос: 901 строка > записанных в baseline 900", result.stdout)

    def test_legacy_file_at_recorded_size_is_ok(self) -> None:
        self.write_policy({self.LEGACY: 900})
        self.make_file(self.LEGACY, 900)
        self.assertEqual(self.run_check().returncode, EXIT_OK)

    def test_legacy_file_shrunk_below_hard_limit_suggests_removal(self) -> None:
        self.write_policy({self.LEGACY: 900})
        self.make_file(self.LEGACY, 790)
        result = self.run_check()
        self.assertEqual(result.returncode, EXIT_OK)
        self.assertIn("уберите запись из baseline", result.stdout)

    def test_stale_baseline_entry_is_warning(self) -> None:
        self.write_policy({"crates/gone.rs": 900})
        result = self.run_check()
        self.assertEqual(result.returncode, EXIT_OK)
        self.assertIn("устаревшая запись baseline", result.stdout)


class UpdateBaselineTests(SizeCheckTestCase):
    LEGACY = "crates/x/src/legacy.rs"

    def test_update_lowers_recorded_size(self) -> None:
        self.write_policy({self.LEGACY: 900})
        self.make_file(self.LEGACY, 850)
        result = self.run_check("--update-baseline")
        self.assertEqual(result.returncode, EXIT_OK, result.stdout)
        self.assertEqual(self.read_baseline(), {self.LEGACY: 850})

    def test_update_removes_entry_when_file_fits_hard_limit(self) -> None:
        self.write_policy({self.LEGACY: 900})
        self.make_file(self.LEGACY, 700)
        self.run_check("--update-baseline")
        self.assertEqual(self.read_baseline(), {})

    def test_update_removes_stale_entry(self) -> None:
        self.write_policy({"crates/gone.rs": 900})
        self.run_check("--update-baseline")
        self.assertEqual(self.read_baseline(), {})

    def test_update_never_raises_recorded_size(self) -> None:
        self.write_policy({self.LEGACY: 900})
        self.make_file(self.LEGACY, 950)
        result = self.run_check("--update-baseline")
        self.assertEqual(result.returncode, EXIT_VIOLATIONS)
        self.assertEqual(self.read_baseline(), {self.LEGACY: 900})

    def test_update_does_not_adopt_new_oversized_file(self) -> None:
        self.make_file("crates/x/src/new.rs", 900)
        result = self.run_check("--update-baseline")
        self.assertEqual(result.returncode, EXIT_VIOLATIONS)
        self.assertEqual(self.read_baseline(), {})

    def test_update_preserves_rest_of_policy_file(self) -> None:
        self.write_policy({self.LEGACY: 900})
        self.make_file(self.LEGACY, 850)
        self.run_check("--update-baseline")
        text = self.policy_path.read_text(encoding="utf-8")
        self.assertTrue(text.startswith(POLICY_HEAD))
        self.assertIn("комментарий выше baseline должен пережить", text)


class AdoptTests(SizeCheckTestCase):
    def test_adopt_records_oversized_file_and_makes_check_pass(self) -> None:
        self.make_file("crates/x/src/old.rs", 900)
        result = self.run_check("--adopt", "crates/x/src/old.rs")
        self.assertEqual(result.returncode, EXIT_OK, result.stdout)
        self.assertEqual(self.read_baseline(), {"crates/x/src/old.rs": 900})
        self.assertIn("--adopt: crates/x/src/old.rs заведён в baseline", result.stdout)

    def test_adopt_of_small_file_is_usage_error(self) -> None:
        self.make_file("crates/x/src/small.rs", 100)
        result = self.run_check("--adopt", "crates/x/src/small.rs")
        self.assertEqual(result.returncode, EXIT_USAGE_ERROR)
        self.assertEqual(self.read_baseline(), {})

    def test_adopt_of_already_recorded_file_is_usage_error(self) -> None:
        self.write_policy({"crates/x/src/old.rs": 900})
        self.make_file("crates/x/src/old.rs", 950)
        result = self.run_check("--adopt", "crates/x/src/old.rs")
        self.assertEqual(result.returncode, EXIT_USAGE_ERROR)
        self.assertEqual(self.read_baseline(), {"crates/x/src/old.rs": 900})


class ConfigurationErrorTests(SizeCheckTestCase):
    def test_broken_toml_gives_exit_2(self) -> None:
        self.policy_path.write_text("soft_limit_lines = [", encoding="utf-8")
        result = self.run_check()
        self.assertEqual(result.returncode, EXIT_USAGE_ERROR)
        self.assertIn("битый TOML", result.stderr)

    def test_soft_not_below_hard_gives_exit_2(self) -> None:
        self.policy_path.write_text(
            POLICY_HEAD.replace("soft_limit_lines = 700", "soft_limit_lines = 900"), encoding="utf-8")
        result = self.run_check()
        self.assertEqual(result.returncode, EXIT_USAGE_ERROR)
        self.assertIn("soft_limit_lines", result.stderr)

    def test_missing_policy_file_gives_exit_2(self) -> None:
        self.policy_path.unlink()
        self.assertEqual(self.run_check().returncode, EXIT_USAGE_ERROR)


class RealPolicyFileTests(unittest.TestCase):
    def test_real_policy_parses_and_limits_are_sane(self) -> None:
        document = tomllib.loads(REAL_POLICY_PATH.read_text(encoding="utf-8"))
        self.assertLess(document["soft_limit_lines"], document["hard_limit_lines"])
        self.assertIn("baseline", document)
        self.assertIn("fixtures", document["exclude_dir_names"])

    def test_real_policy_is_accepted_by_the_checker(self) -> None:
        with tempfile.TemporaryDirectory() as empty_root:
            result = subprocess.run(
                [sys.executable, str(SCRIPT_PATH), "--root", empty_root, "--policy", str(REAL_POLICY_PATH)],
                capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, EXIT_OK, result.stderr)


if __name__ == "__main__":
    unittest.main()
