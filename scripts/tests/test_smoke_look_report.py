"""14o: вид и основа в отчётах `viewfinder` и `viewfinder-stage-budget`, строки вида, размытие фона раздела и флаг
`--tone-base` у `device-smoke.sh` (без телефона)."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_checks_viewfinder  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import smoke_wait  # noqa: E402
import test_smoke_device_script as device_script  # noqa: E402
from smoke_dumpsys import CheckOutcome  # noqa: E402
from test_smoke_stage_budget_surface import SCENARIO, budget_report  # noqa: E402
from test_smoke_viewfinder import (  # noqa: E402
    look_report,
    passed,
    result_line,
    stage_budget_look_fields,
    verdict_of,
    viewfinder_report,
)


def budget_problems(report: dict) -> list[str]:
    return smoke_checks_viewfinder.check_viewfinder_stage_budget(passed(SCENARIO, report))


def look_row_report(rows: list[str], **overrides: object) -> dict:
    """Отчёт с одними строками вида на экране калибровки."""
    report = budget_report(
        requested_rows=rows,
        table=[f"{row} | 1.00 / 1.20 | 5.0 ± 1.0 | measured" for row in rows],
        scopes={"request": {"kind": "waveform"}, "float32_controls": {"rounding_mode_rte": True}},
        **stage_budget_look_fields(rows),
    )
    report.update(overrides)
    return report


class ViewfinderLookReportTest(unittest.TestCase):
    def test_a_viewfinder_report_without_the_picture_look_fails(self) -> None:
        report = viewfinder_report()
        del report["look"]["picture"]
        verdict = verdict_of("viewfinder", report)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("look.picture", verdict.message)

    def test_every_field_of_the_look_report_is_required_and_checked(self) -> None:
        for key in ("tone_base", "recipe_fingerprint", "layer_neutral", "wheels_active", "tone_active", "measurement_look"):
            with self.subTest(missing=key):
                look = look_report()
                del look[key]
                problems = smoke_checks_viewfinder.look_report_problems(look, "look")
                self.assertTrue(any(key in problem for problem in problems), problems)

    def test_a_bad_base_or_fingerprint_is_a_problem(self) -> None:
        self.assertTrue(smoke_checks_viewfinder.look_report_problems(look_report(tone_base="reinhard"), "look"))
        self.assertTrue(smoke_checks_viewfinder.look_report_problems(look_report(fingerprint="XYZ"), "look"))
        self.assertTrue(smoke_checks_viewfinder.look_report_problems(look_report(fingerprint="ABCDEF0123456789"), "look"))
        self.assertEqual(smoke_checks_viewfinder.look_report_problems(look_report(), "look"), [])


class StageBudgetLookReportTest(unittest.TestCase):
    def test_the_report_without_look_fields_fails_loudly(self) -> None:
        for key in ("look", "backdrop_pyramid_rebuilds"):
            with self.subTest(missing=key):
                report = budget_report()
                del report[key]
                self.assertTrue(any(key in problem for problem in budget_problems(report)), key)

    def test_a_look_row_needs_distinct_recipes_in_both_windows(self) -> None:
        self.assertEqual(budget_problems(look_row_report(["look_wheels", "look_tone"])), [])
        report = look_row_report(["look_wheels"])
        report["look_rows"][0]["off"]["recipe_fingerprint"] = report["look_rows"][0]["on"]["recipe_fingerprint"]
        self.assertTrue(any("мерила бы ноль" in problem for problem in budget_problems(report)))

    def test_a_requested_look_row_missing_from_the_report_is_a_problem(self) -> None:
        report = look_row_report(["look_wheels"])
        del report["look_rows"]
        self.assertTrue(any("look_wheels нет видов окон" in problem for problem in budget_problems(report)))

    def test_look_rows_are_allowed_on_every_surface(self) -> None:
        for surface in ("bare_viewfinder", "photo_screen", "calibrate_screen"):
            with self.subTest(surface=surface):
                report = look_row_report(["look_tone_base_agx"], surface=surface)
                if surface != "calibrate_screen":
                    report["scopes"] = {}
                self.assertEqual(budget_problems(report), [])

    def test_calibrate_must_show_zero_backdrop_blur_builds(self) -> None:
        self.assertEqual(budget_problems(budget_report(backdrop_pyramid_rebuilds=0)), [])
        problems = budget_problems(budget_report(backdrop_pyramid_rebuilds=12))
        self.assertTrue(any("пирамида размытия" in problem and "12" in problem for problem in problems), problems)
        unmeasured = budget_problems(budget_report(backdrop_pyramid_rebuilds=None))
        self.assertTrue(any("окон с диагностикой не было" in problem for problem in unmeasured), unmeasured)

    def test_blur_builds_outside_calibrate_are_not_judged(self) -> None:
        report = budget_report(
            surface="photo_screen",
            requested_rows=["ui_overlay"],
            table=["ui_overlay | 1.50 / 1.80 | 20.0 ± 1.0 | measured"],
            scopes={},
            backdrop_pyramid_rebuilds=5,
        )
        self.assertEqual(budget_problems(report), [])


class ExpectedToneBaseTest(unittest.TestCase):
    def expected(self, scenario: str, report: dict, base: str) -> list[str]:
        return smoke_checks_viewfinder.expected_tone_base_problems(passed(scenario, report), base)

    def test_the_shown_base_equal_to_the_requested_passes_for_both_scenarios(self) -> None:
        aces = look_report(tone_base="aces")
        self.assertEqual(
            self.expected("viewfinder", viewfinder_report(look={"picture": aces}), "aces"), []
        )
        self.assertEqual(self.expected(SCENARIO, budget_report(look=aces), "aces"), [])

    def test_a_different_shown_base_fails_and_explains_the_preset_cause(self) -> None:
        for scenario, report in (
            ("viewfinder", viewfinder_report()),
            (SCENARIO, budget_report()),
        ):
            with self.subTest(scenario=scenario):
                problems = self.expected(scenario, report, "aces")
                self.assertEqual(len(problems), 1)
                self.assertIn("'agx'", problems[0])
                self.assertIn("'aces'", problems[0])
                self.assertIn("пресета", problems[0])

    def test_smoke_wait_applies_the_expectation_on_top_of_the_other_post_check(self) -> None:
        other = lambda _result: CheckOutcome(["чужая проблема"], ["заметка"])  # noqa: E731
        check = smoke_wait.with_expected_tone_base(other, "legacy")
        outcome = check(passed("viewfinder", viewfinder_report()))
        self.assertEqual(outcome.notes, ["заметка"])
        self.assertEqual(outcome.problems[0], "чужая проблема")
        self.assertEqual(len(outcome.problems), 2)
        self.assertEqual(smoke_wait.with_expected_tone_base(None, "agx")(passed("viewfinder", viewfinder_report())).problems, [])

    def test_the_command_line_accepts_only_known_bases(self) -> None:
        base = ["--serial", "s", "--since-epoch", "1.000", "--package", "p", "--scenario", "viewfinder", "--timeout", "1"]
        self.assertEqual(smoke_wait.parse_arguments([*base, "--expect-tone-base", "filmic"]).expect_tone_base, "filmic")
        with self.assertRaises(SystemExit):
            smoke_wait.parse_arguments([*base, "--expect-tone-base", "reinhard"])


class DeviceSmokeToneBaseFlagTest(unittest.TestCase):
    """`device-smoke.sh --tone-base` на фейковом adb."""

    setUp = device_script.DeviceSmokeScenariosTest.setUp
    run_smoke = device_script.DeviceSmokeScenariosTest.run_smoke
    adb_calls = device_script.DeviceSmokeScenariosTest.adb_calls

    def budget_line(self, base: str) -> str:
        report = budget_report(
            surface="bare_viewfinder",
            requested_rows=["look_layer"],
            table=["look_layer | 1.00 / 1.20 | 5.0 ± 1.0 | measured"],
            scopes={},
            **stage_budget_look_fields(["look_layer"]),
        )
        report["look"] = look_report(tone_base=base)
        return result_line(passed(SCENARIO, report))

    def test_the_base_reaches_the_intent_as_an_extra(self) -> None:
        completed = self.run_smoke(SCENARIO, [self.budget_line("filmic")], extra_arguments=("--tone-base", "filmic"))
        self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
        start = next(call for call in self.adb_calls() if "am start" in call)
        self.assertIn("-e tone_base filmic", start)

    def test_without_the_flag_no_tone_base_extra_is_sent(self) -> None:
        self.run_smoke(SCENARIO, [self.budget_line("agx")])
        start = next(call for call in self.adb_calls() if "am start" in call)
        self.assertNotIn("tone_base", start)

    def test_a_bad_base_or_a_scenario_without_a_shown_base_is_a_usage_error(self) -> None:
        for arguments, scenario in (
            (("--tone-base", "reinhard"), "viewfinder"),
            (("--tone-base", "agx;ls"), "viewfinder"),
            (("--tone-base", "agx"), "capture"),
            (("--tone-base", "agx"), "glass"),
        ):
            with self.subTest(arguments=arguments, scenario=scenario):
                completed = self.run_smoke(scenario, [], extra_arguments=arguments)
                self.assertEqual(completed.returncode, 64, completed.stderr)
                self.assertFalse(any("am start" in call for call in self.adb_calls()))

    def test_the_run_fails_loudly_when_the_phone_shows_another_base(self) -> None:
        completed = self.run_smoke(SCENARIO, [self.budget_line("agx")], extra_arguments=("--tone-base", "aces"))
        self.assertEqual(completed.returncode, 1, completed.stdout + completed.stderr)
        self.assertIn("пресета", completed.stderr)
        self.assertIn("'agx'", completed.stderr)

    def test_the_run_passes_when_the_shown_base_is_the_requested_one(self) -> None:
        completed = self.run_smoke(SCENARIO, [self.budget_line("aces")], extra_arguments=("--tone-base", "aces"))
        self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
        start = next(call for call in self.adb_calls() if "am start" in call)
        self.assertIn("-e tone_base aces", start)

    def test_without_the_flag_any_shown_base_passes(self) -> None:
        completed = self.run_smoke(SCENARIO, [self.budget_line("legacy")])
        self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)


if __name__ == "__main__":
    unittest.main()
