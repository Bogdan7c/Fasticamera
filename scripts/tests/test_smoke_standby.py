"""Тесты smoke-сценария ``standby`` (13b): перепроверка сводки (``smoke_checks_standby``) через сквозной
``evaluate_stream``, заметки с цифрами и ``device-smoke.sh standby`` на фейковом adb (скрипт ничего не нажимает:
намерения объектива сценарий кладёт в ящик движка сам)."""

from __future__ import annotations

import copy
import json
import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_checks_standby  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import smoke_driver  # noqa: E402
import smoke_wait  # noqa: E402
import test_smoke_device_script as device_script  # noqa: E402


def result_line(payload: dict) -> str:
    return f"  1792957062.000  4242  4242 I Fasticamera: FASTICAMERA_RESULT {json.dumps(payload)}\n"


def swap(leg: str, to_role: str, **overrides: object) -> dict:
    record: dict = {
        "leg": leg,
        "to_role": to_role,
        "method": "promoted_standby",
        "observed_ms": 130,
        "engine_elapsed_ms": 110,
    }
    record.update(overrides)
    return record


def standby_report(**overrides: object) -> dict:
    """Здоровая сводка ``standby``: два обмена повышением, обе камеры ~60 к/с, тёплые закрыты, дом — ``wide``."""
    report: dict = {
        "stage": "done",
        "failure": None,
        "home_camera": "2",
        "neighbour_camera": "3",
        "time_to_ready_ms": 260,
        "swaps": [swap("to_neighbour", "ultra_wide"), swap("back_home", "wide")],
        "windows": [
            {"name": "alone", "primary_fps": 60.0, "standby_fps": 0.0, "both": False},
            {"name": "both_running", "primary_fps": 59.8, "standby_fps": 59.5, "both": True},
            {"name": "after_to_neighbour", "primary_fps": 59.7, "standby_fps": 59.6, "both": True},
            {"name": "after_back_home", "primary_fps": 59.9, "standby_fps": 59.4, "both": True},
        ],
        "min_fps_while_both_run": 59.4,
        "release_ms": 1700,
        "final_role": "wide",
        "stream": {
            "standby_streams_begun": 1,
            "standby_begins_refused": 0,
            "standby_frames_returned": 400,
            "standby_promotions": 2,
            "leaked_leases": 0,
            "frames_leaked": 0,
        },
        "engine": {
            "promotions": 2,
            "promotion_fallbacks": 0,
            "standby_failures": 0,
            "switches_failed": 0,
            "stale_backend_events": 0,
        },
        "backend": [{"slot": 0, "results_offered": 300, "capture_failures": 0, "swaps": 2}],
    }
    report.update(overrides)
    return report


def verdict_of(report: dict) -> smoke_wait.SmokeVerdict:
    payload = {"scenario": "standby", "status": "passed", "report": report}
    return smoke_wait.evaluate_stream([result_line(payload)], "standby")


def with_section(name: str, **changes: object) -> dict:
    """Здоровый отчёт, в котором вложенная секция ``name`` испорчена ровно в этих полях."""
    report = standby_report()
    report[name] = {**report[name], **changes}
    return report


class StandbyCheckTest(unittest.TestCase):
    def test_healthy_standby_passes_and_reports_the_numbers_as_notes(self) -> None:
        verdict = verdict_of(standby_report())
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        joined = " ".join(verdict.notes)
        self.assertIn("260 мс", joined)
        self.assertIn("59.4", joined)
        self.assertIn("1700", joined)

    def test_each_broken_rule_gives_a_problem_naming_it(self) -> None:
        broken_windows = standby_report()
        broken_windows["windows"][1] = {"name": "both_running", "primary_fps": 54.0, "standby_fps": 59.0, "both": True}
        only_alone = standby_report(windows=[{"name": "alone", "primary_fps": 60.0, "standby_fps": 0.0, "both": False}])
        cases = {
            "исполнен как 'reopened'": standby_report(
                swaps=[swap("to_neighbour", "ultra_wide"), swap("back_home", "wide", method="reopened")]
            ),
            "обменов ролей": standby_report(swaps=[swap("to_neighbour", "ultra_wide")]),
            "время обмена": standby_report(
                swaps=[swap("to_neighbour", "ultra_wide", observed_ms=None), swap("back_home", "wide")]
            ),
            "просьба → тёплая готова": standby_report(time_to_ready_ms=None),
            "утекло лизингов кадров": with_section("stream", leaked_leases=1),
            "кадры тёплой не возвращались": with_section("stream", standby_frames_returned=0),
            "поток кадров провёл обменов": with_section("stream", standby_promotions=1),
            "поток отказал в тёплой эпохе": with_section("stream", standby_begins_refused=1),
            "тёплая отказывала": with_section("engine", standby_failures=1),
            "бэкенд отверг обмен": with_section("engine", promotion_fallbacks=1),
            "неудачных смен": with_section("engine", switches_failed=1),
            "движок повысил тёплую": with_section("engine", promotions=1),
            "ни одного результата тёплой": standby_report(backend=[{"slot": 0, "results_offered": 0}]),
            "ниже порога": broken_windows,
            "не замерена частота, пока стримят обе": only_alone,
            "не закрылись": standby_report(release_ms=None),
            "остановился на шаге": standby_report(stage="release"),
            "итоговой роли": standby_report(final_role=None),
            "прогон провален": standby_report(failure="warm: не дождались"),
        }
        for fragment, report in cases.items():
            with self.subTest(fragment):
                verdict = verdict_of(report)
                self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED, verdict.message)
                self.assertIn(fragment, verdict.message)

    def test_a_missing_report_is_a_problem(self) -> None:
        verdict = smoke_wait.evaluate_stream([result_line({"scenario": "standby", "status": "passed"})], "standby")
        self.assertIn("нет report", verdict.message)

    def test_rust_failure_with_violations_is_reported_as_failed_status(self) -> None:
        payload = {"scenario": "standby", "status": "failed", "error": "обмен ToNeighbour исполнен как reopened"}
        verdict = smoke_wait.evaluate_stream([result_line(payload)], "standby")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("reopened", verdict.message)

    def test_notes_helper_ignores_foreign_shapes(self) -> None:
        self.assertEqual(smoke_checks_standby.standby_notes({}), [])
        self.assertEqual(smoke_checks_standby.standby_notes({"report": {"time_to_ready_ms": "много"}}), [])

    def test_standby_is_registered_without_screenshots_driver_steps_or_neutral_card(self) -> None:
        self.assertIn("standby", smoke_wait.CHECKS)
        self.assertIn("standby", smoke_wait.NOTES)
        self.assertNotIn("standby", smoke_wait.SCREENSHOT_SCENARIOS)
        self.assertNotIn("standby", smoke_wait.NEUTRAL_CARD_SCENARIOS)
        self.assertNotIn("standby", smoke_driver.SCENARIO_STEPS, "скрипт ничего не нажимает")


class DeviceSmokeStandbyTest(unittest.TestCase):
    """``device-smoke.sh standby`` на фейковом adb (тот же, что у ``test_smoke_device_script``)."""

    setUp = device_script.DeviceSmokeScenariosTest.setUp
    run_smoke = device_script.DeviceSmokeScenariosTest.run_smoke
    adb_calls = device_script.DeviceSmokeScenariosTest.adb_calls

    def test_standby_passes_without_touching_the_phone_through_adb(self) -> None:
        payload = {"scenario": "standby", "status": "passed", "report": copy.deepcopy(standby_report())}
        completed = self.run_smoke("standby", [result_line(payload)])
        self.assertEqual(completed.returncode, 0, completed.stderr)
        calls = self.adb_calls()
        self.assertFalse(any("input" in call or "screencap" in call for call in calls), calls)
        self.assertTrue(any("-e scenario standby" in call for call in calls), calls)

    def test_standby_with_a_reopened_swap_fails_the_script(self) -> None:
        report = standby_report(swaps=[swap("to_neighbour", "ultra_wide", method="reopened"), swap("back_home", "wide")])
        payload = {"scenario": "standby", "status": "passed", "report": report}
        completed = self.run_smoke("standby", [result_line(payload)])
        self.assertEqual(completed.returncode, 1, completed.stderr)
        self.assertIn("reopened", completed.stderr)


if __name__ == "__main__":
    unittest.main()
