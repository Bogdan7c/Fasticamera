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


def seamless_switch(step: str, leg: str, **overrides: object) -> dict:
    """Здоровая бесшовная смена: повышение тёплой, 0 удержанных/чёрных/скрытых, разрыв 1 период, обе камеры 59 к/с."""
    entry: dict = {
        "step": step,
        "leg": leg,
        "method": "promoted_standby",
        "events": 1,
        "held": 0,
        "black": 0,
        "hidden": 0,
        "gap_periods": 1.02,
        "frames_missed": 0,
        "gap_ms": 17.0,
        "tgt_milli": 1000,
        "sw_milli": 1004,
        "ready_ms": 240.0,
        "fps_min": 59.2,
        "closed_ms": None,
    }
    entry.update(overrides)
    return entry


def seamless_release(step: str, **overrides: object) -> dict:
    entry = seamless_switch(step, "release", method=None, events=0, gap_periods=None, frames_missed=None, gap_ms=None, ready_ms=None, fps_min=None)
    entry["closed_ms"] = 3700.0
    entry.update(overrides)
    return entry


def healthy_seamless() -> list[dict]:
    return [
        seamless_switch("11_wheel_seamless_out", "out"),
        seamless_switch("12_wheel_seamless_back", "back"),
        seamless_release("13_wheel_seamless_release"),
        seamless_switch("14_pinch_seamless_out", "out"),
        seamless_switch("15_pinch_seamless_back", "back"),
        seamless_release("16_pinch_seamless_release"),
    ]


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
        "black_frames_before_first_picture": 0,
        "held_frames": 96,
        "reveal_faults": {
            "revealed_by_timeout": 0,
            "ended_without_stream": 0,
            "ended_without_presentable_frame": 0,
            "black_camera_unavailable": 0,
            "black_picture_unavailable": 0,
        },
        "switches_failed": 0,
        "switches_completed": 6,
        "lens_intents_published": 240,
        "leaked_leases": 0,
        "standby_failures": 0,
        "promotion_fallbacks": 0,
        "seamless": healthy_seamless(),
        "tap_hidden": [{"step": "01_tap_0.6", "hidden": 2}, {"step": "02_tap_1", "hidden": 2}],
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
            "удержание не началось": zoom_report(black_frames_before_first_picture=30),
            "неудачных смен": zoom_report(switches_failed=1),
            "выполнено шагов": zoom_report(steps_done=7),
            "не выполнено шагов": zoom_report(unfinished=2),
            "числа шагов плана": zoom_report(steps_planned=None),
            "ни одной смены": zoom_report(switches_observed=0),
            "max_switch_ms": zoom_report(max_switch_ms=None),
            "max_first_live_frame_ms": zoom_report(max_first_live_frame_ms=None),
            "reveal_faults.revealed_by_timeout=1": zoom_report(
                reveal_faults={**zoom_report()["reveal_faults"], "revealed_by_timeout": 1}
            ),
            "reveal_faults.black_picture_unavailable=2": zoom_report(
                reveal_faults={**zoom_report()["reveal_faults"], "black_picture_unavailable": 2}
            ),
            "reveal_faults.ended_without_presentable_frame=1": zoom_report(
                reveal_faults={**zoom_report()["reveal_faults"], "ended_without_presentable_frame": 1}
            ),
            "reveal_faults.black_camera_unavailable=1": zoom_report(
                reveal_faults={**zoom_report()["reveal_faults"], "black_camera_unavailable": 1}
            ),
            "reveal_faults.ended_without_stream=1": zoom_report(
                reveal_faults={**zoom_report()["reveal_faults"], "ended_without_stream": 1}
            ),
            "нет reveal_faults": zoom_report(reveal_faults=None),
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

    def test_each_seamless_rule_is_rechecked_and_names_its_reason(self) -> None:
        def with_step(index: int, **changes: object) -> dict:
            steps = healthy_seamless()
            steps[index] = {**steps[index], **changes}
            return zoom_report(seamless=steps)

        cases = {
            "reopened": with_step(0, method="reopened"),
            "ожидалось 'promoted_standby'": with_step(1, method=None),
            "событий LensSwitched 2": with_step(0, events=2),
            "удержанных кадров 3": with_step(0, held=3),
            "чёрных кадров 1": with_step(3, black=1),
            "скрытых непрерывностью кадров 2": with_step(4, hidden=2),
            "пропущено кадров 3": with_step(0, gap_periods=4.0, frames_missed=3),
            "пропущено кадров None": with_step(0, frames_missed=None),
            "частота камер, пока стримят обе, 41.0": with_step(1, fps_min=41.0),
            "смена вверх случилась при 1105 тыс.": with_step(1, sw_milli=1105),
            "в момент смены не измерен": with_step(4, sw_milli=None),
            "ready_ms=None": with_step(0, ready_ms=None),
            "тёплая не закрылась": with_step(2, closed_ms=None),
            "leaked_leases=1": zoom_report(leaked_leases=1),
            "standby_failures=1": zoom_report(standby_failures=1),
            "promotion_fallbacks=2": zoom_report(promotion_fallbacks=2),
            "бесшовных смен 2": zoom_report(seamless=healthy_seamless()[:2] + [healthy_seamless()[2]]),
            "нет списка бесшовных шагов": zoom_report(seamless=None),
        }
        for expected, report in cases.items():
            with self.subTest(expected=expected):
                verdict = verdict_of(report)
                self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED, verdict.message)
                self.assertIn(expected, verdict.message)

    def test_up_to_two_missed_frames_pass_even_with_jittered_periods_and_three_fail(self) -> None:
        def report_with_gap(periods: float, missed: int) -> dict:
            steps = healthy_seamless()
            steps[0] = {**steps[0], "gap_periods": periods, "frames_missed": missed}
            return zoom_report(seamless=steps)

        # 2,0003 периода — один пропущенный кадр с дрожанием времени показа (так было на телефоне): проходит.
        self.assertEqual(verdict_of(report_with_gap(2.0003, 1)).exit_code, smoke_wait.EXIT_OK)
        # Два пропуска — допуск владельца (решение 132); три — провал.
        self.assertEqual(verdict_of(report_with_gap(3.0003, 2)).exit_code, smoke_wait.EXIT_OK)
        self.assertEqual(verdict_of(report_with_gap(4.0, 3)).exit_code, smoke_wait.EXIT_SCENARIO_FAILED)

    def test_hover_swaps_are_a_note_not_a_failure(self) -> None:
        steps = healthy_seamless()
        steps.insert(2, seamless_switch("13_wheel_seamless_hover", "hover", events=9))
        verdict = verdict_of(zoom_report(seamless=steps))
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        notes = [note for note in verdict.notes if "колебания у отметки" in note]
        self.assertEqual(len(notes), 1, verdict.notes)
        self.assertIn("смен 9", notes[0])

    def test_many_hidden_frames_in_a_tap_is_a_note_not_a_failure(self) -> None:
        report = zoom_report(tap_hidden=[{"step": "01_tap_0.6", "hidden": 10}, {"step": "02_tap_1", "hidden": 4}])
        verdict = verdict_of(report)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        notes = [note for note in verdict.notes if "NOTE" in note]
        self.assertEqual(len(notes), 1, verdict.notes)
        self.assertIn("01_tap_0.6", notes[0])
        self.assertIn("10", notes[0])

    def test_seamless_notes_carry_the_phone_numbers(self) -> None:
        joined = " ".join(verdict_of(zoom_report()).notes)
        self.assertIn("240.0 мс", joined)
        self.assertIn("3700.0 мс", joined)

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

    def test_zoom_with_a_black_no_picture_frame_fails_even_when_picture_expected_blacks_are_zero(self) -> None:
        report = zoom_report(black_frames_before_first_picture=1)
        problems = smoke_checks_zoom.check_zoom({"status": "passed", "report": report})
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("удержание не началось", problems[0])
        self.assertIn("black_frames_before_first_picture=1", problems[0])
        # Сводка без поля (старый сценарий) тоже не проходит: чёрные кадры «нет картинки» не измерены.
        stale = zoom_report()
        del stale["black_frames_before_first_picture"]
        self.assertTrue(smoke_checks_zoom.check_zoom({"status": "passed", "report": stale}))

    def test_the_usage_header_documents_zoom(self) -> None:
        text = (SCRIPTS_DIR / "device-smoke.sh").read_text(encoding="utf-8")
        header = text.split("set -euo pipefail", 1)[0]
        self.assertIn("zoom (240;", header)
        self.assertNotIn("zoom (150;", header, "прогон идёт около 240 с: старый таймаут 150 с обрывал бы его")

    def test_neutral_card_has_no_meaning_for_zoom(self) -> None:
        completed = self.run_smoke("zoom", [], extra_arguments=("--neutral-card",))
        self.assertEqual(completed.returncode, 64, completed.stderr)


if __name__ == "__main__":
    unittest.main()
