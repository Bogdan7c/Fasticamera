"""Тесты smoke-сценария ``zoom`` (сессия 13): перепроверка сводки (``smoke_checks_zoom``) через сквозной
``evaluate_stream``, заметки с цифрами для 13a и ``device-smoke.sh zoom`` на фейковом adb (скрипт ничего не нажимает:
жесты сценарий вкладывает в экран сам)."""

from __future__ import annotations

import copy
import json
import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_checks_zoom  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import smoke_driver  # noqa: E402
import smoke_wait  # noqa: E402
import test_smoke_device_script as device_script  # noqa: E402


def result_line(payload: dict) -> str:
    return f"  1792957062.000  4242  4242 I Fasticamera: FASTICAMERA_RESULT {json.dumps(payload)}\n"


def zoom_report(**overrides: object) -> dict:
    """Здоровая сводка ``zoom``: 10 шагов, 6 смен объектива, чёрных кадров 0, экран оставлен на 1× основной камеры."""
    report: dict = {
        "steps_planned": 10,
        "steps_done": 10,
        "unfinished": 0,
        "switches_observed": 6,
        "max_switch_ms": 420,
        "max_first_live_frame_ms": 610.5,
        "black_frames_while_picture_expected": 0,
        "held_frames": 96,
        "switches_failed": 0,
        "switches_completed": 6,
        "lens_intents_published": 240,
        "final": {"ratio_milli": 1000, "facing": "back", "role": "wide"},
        "failure": None,
    }
    report.update(overrides)
    return report


def verdict_of(report: dict) -> smoke_wait.SmokeVerdict:
    payload = {"scenario": "zoom", "status": "passed", "report": report}
    return smoke_wait.evaluate_stream([result_line(payload)], "zoom")


class ZoomCheckTest(unittest.TestCase):
    def test_healthy_zoom_passes_and_reports_the_numbers_as_notes(self) -> None:
        verdict = verdict_of(zoom_report())
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        joined = " ".join(verdict.notes)
        self.assertIn("420", joined)
        self.assertIn("610.5", joined)
        self.assertIn("96", joined)

    def test_each_rule_is_rechecked_and_names_its_reason(self) -> None:
        cases = {
            "чёрных кадров": zoom_report(black_frames_while_picture_expected=1),
            "неудачных смен": zoom_report(switches_failed=1),
            "выполнено шагов": zoom_report(steps_done=7),
            "не выполнено шагов": zoom_report(unfinished=2),
            "числа шагов плана": zoom_report(steps_planned=None),
            "ни одной смены": zoom_report(switches_observed=0),
            "max_switch_ms": zoom_report(max_switch_ms=None),
            "max_first_live_frame_ms": zoom_report(max_first_live_frame_ms=None),
            "намерений объектива": zoom_report(lens_intents_published=0),
            "провален вне шагов": zoom_report(failure="рендер недоступен"),
            "итогового состояния": zoom_report(final=None),
            "в конце экран на": zoom_report(final={"ratio_milli": 590, "facing": "back", "role": "wide"}),
            "направление": zoom_report(final={"ratio_milli": 1000, "facing": "front", "role": "wide"}),
            "объектив": zoom_report(final={"ratio_milli": 1000, "facing": "back", "role": "ultra_wide"}),
        }
        for expected, report in cases.items():
            with self.subTest(expected=expected):
                verdict = verdict_of(report)
                self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED, verdict.message)
                self.assertIn(expected, verdict.message)

    def test_one_x_tolerance_edges(self) -> None:
        near = zoom_report(final={"ratio_milli": 1020, "facing": "back", "role": "wide"})
        self.assertEqual(verdict_of(near).exit_code, smoke_wait.EXIT_OK)
        far = zoom_report(final={"ratio_milli": 1021, "facing": "back", "role": "wide"})
        self.assertEqual(verdict_of(far).exit_code, smoke_wait.EXIT_SCENARIO_FAILED)

    def test_a_failed_scenario_is_a_failure_with_its_reason(self) -> None:
        payload = {"scenario": "zoom", "status": "failed", "error": "02_tap_1: за 6000 мс движок не сообщил о смене объектива"}
        verdict = smoke_wait.evaluate_stream([result_line(payload)], "zoom")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("02_tap_1", verdict.message)

    def test_missing_report_is_reported_not_guessed(self) -> None:
        verdict = smoke_wait.evaluate_stream([result_line({"scenario": "zoom", "status": "passed"})], "zoom")
        self.assertIn("нет report", verdict.message)

    def test_notes_helper_ignores_foreign_shapes(self) -> None:
        self.assertEqual(smoke_checks_zoom.zoom_notes({}), [])
        self.assertEqual(smoke_checks_zoom.zoom_notes({"report": {"max_switch_ms": "много"}}), [])
        self.assertEqual(len(smoke_checks_zoom.zoom_notes({"report": {"max_switch_ms": 1, "max_first_live_frame_ms": 2}})), 1)

    def test_zoom_is_registered_without_screenshots_driver_steps_or_neutral_card(self) -> None:
        self.assertIn("zoom", smoke_wait.CHECKS)
        self.assertIn("zoom", smoke_wait.NOTES)
        self.assertNotIn("zoom", smoke_wait.SCREENSHOT_SCENARIOS)
        self.assertNotIn("zoom", smoke_wait.NEUTRAL_CARD_SCENARIOS)
        self.assertNotIn("zoom", smoke_driver.SCENARIO_STEPS, "жесты вкладывает сам сценарий, скрипт ничего не нажимает")


class DeviceSmokeZoomTest(unittest.TestCase):
    """``device-smoke.sh zoom`` на фейковом adb (тот же, что у ``test_smoke_device_script``)."""

    setUp = device_script.DeviceSmokeScenariosTest.setUp
    run_smoke = device_script.DeviceSmokeScenariosTest.run_smoke
    adb_calls = device_script.DeviceSmokeScenariosTest.adb_calls

    def test_zoom_passes_without_touching_the_phone_through_adb(self) -> None:
        payload = {"scenario": "zoom", "status": "passed", "report": copy.deepcopy(zoom_report())}
        completed = self.run_smoke("zoom", [result_line(payload)])
        self.assertEqual(completed.returncode, 0, completed.stderr)
        calls = self.adb_calls()
        self.assertFalse(any("input" in call or "screencap" in call for call in calls), calls)
        self.assertTrue(any("-e scenario zoom" in call for call in calls), calls)
        self.assertNotIn("каталог артефактов", completed.stderr)

    def test_zoom_with_a_black_frame_fails_the_script(self) -> None:
        report = zoom_report(black_frames_while_picture_expected=3)
        payload = {"scenario": "zoom", "status": "passed", "report": report}
        completed = self.run_smoke("zoom", [result_line(payload)])
        self.assertEqual(completed.returncode, 1, completed.stderr)
        self.assertIn("чёрных кадров", completed.stderr)

    def test_the_usage_header_documents_zoom(self) -> None:
        text = (SCRIPTS_DIR / "device-smoke.sh").read_text(encoding="utf-8")
        header = text.split("set -euo pipefail", 1)[0]
        self.assertIn("zoom (150;", header)

    def test_neutral_card_has_no_meaning_for_zoom(self) -> None:
        completed = self.run_smoke("zoom", [], extra_arguments=("--neutral-card",))
        self.assertEqual(completed.returncode, 64, completed.stderr)


if __name__ == "__main__":
    unittest.main()
