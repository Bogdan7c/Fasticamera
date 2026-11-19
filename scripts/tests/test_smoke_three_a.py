"""Тесты smoke-сценария ``3a`` (сессия 08): проверки отчёта (``smoke_checks_three_a``) через сквозной
``evaluate_stream``, шаги драйвера (скриншоты на ``awb_ready``) и ``device-smoke.sh 3a`` на фейковом adb
(``--neutral-card`` принимается для ``3a``, но не для чужих сценариев)."""

from __future__ import annotations

import copy
import struct
import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_driver  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import smoke_wait  # noqa: E402
import test_smoke_device_script as device_script  # noqa: E402
from test_smoke_viewfinder import RecordingActions, passed, result_line, stage_line  # noqa: E402


def three_a_report() -> dict:
    """Здоровый отчёт ``3a`` (форма — ``three_a_report.rs``, сводка)."""
    return {
        "state_at_end": "streaming",
        "live": {"companion": {"state": "active", "width": 640, "height": 480}, "af_state": "passive_focused",
                 "ae_state": "converged"},
        "awb": {"ready": True, "hidden": 3, "forced": 0, "max_hidden": 10, "awb_vs_hal": "ok"},
        "focus": {
            "taps": [{"x": 0.5, "y": 0.5, "result": "focused"}] * 5,
            "region_effect_diopters": 0.8,
            "triggers_sent": 5,
            "results_without_request_context": 0,
            "results_with_unknown_origin": 0,
        },
        "manual": {
            "requested_exposure_ns": 1_000_000,
            "requested_iso": 400,
            "applied": {"exposure_ns": 996_220, "iso": 400, "latency_frames": 6},
            "adjustments_for_in_range": 0,
            "long_shutter": {"requested_ns": 100_000_000, "frame_budget_ns": 16_666_667, "limited_event_seen": True,
                             "applied_ns": 16_600_000},
        },
        "ev": {
            "points": [
                {"bias_ev": 0.0, "verifiable": True, "ratio_to_baseline": 1.0},
                {"bias_ev": 1.0, "verifiable": True, "ratio_to_baseline": 2.05},
                {"bias_ev": -1.0, "verifiable": True, "ratio_to_baseline": 0.49},
            ],
            "ev_unverifiable_at_limit": False,
        },
        "lock": {"locked": True, "non_locked_after_lock": 0, "back_to_continuous": True},
        "bracket": {
            "captured": True,
            "frames": [{"index": i, "frame_number": 100 + i} for i in range(4)],
            "frame_gaps": 0,
            "skipped_delta": 4,
            "presented_during": 30,
        },
        "restore_failure": None,
    }


def own_ae_report() -> dict:
    """Здоровый отчёт ``3a`` с нашим AE: маркеры нашего исполнителя, HAL при AE_MODE_OFF молчит (``inactive``)."""
    report = copy.deepcopy(three_a_report())
    report["live"].update(ae_source="own", ae_state="inactive")
    for point in report["ev"]["points"]:
        point.update(ae_converged=True, ae_limit_reached=False)
    report["expected_ae_source"] = "own"
    report["lock"].update(
        ae_source="own",
        max_product_deviation=0.0,
        observed_scene_range_ev=2.4,
        product_frozen_after_frames=9,
        prompt_prepare="queued",
        prompt_hold_start="queued",
        prompt_hold_end="queued",
    )
    report["own_ae"] = {
        "progress_at_end": {"kind": "settled"},
        "settled_seen": True,
        "holding_seen": True,
        "limit_reached_seen": False,
        "worst_shortfall_ev": None,
        "target_not_reached_ticks": 0,
        "results_checked": {"repeating": 120, "focus_trigger": 6, "burst": 4, "unknown": 0},
        "vendor_ae_results": 0,
        "first_vendor_ae_frame": None,
        "engine": {"results_observed": 360, "results_with_hal_ae_active": 0},
    }
    return report


def edited(path: str, value: object) -> dict:
    """Здоровый отчёт с одним изменённым полем по пути ``a.b.c``."""
    report = copy.deepcopy(three_a_report())
    node = report
    *parents, last = path.split(".")
    for key in parents:
        node = node[int(key)] if isinstance(node, list) else node[key]
    node[last] = value
    return report


def verdict_of(report: dict) -> smoke_wait.SmokeVerdict:
    return smoke_wait.evaluate_stream([result_line(passed("3a", report))], "3a")


class ThreeACheckTest(unittest.TestCase):
    def test_healthy_run_passes(self) -> None:
        self.assertEqual(verdict_of(three_a_report()).exit_code, smoke_wait.EXIT_OK)

    def test_notes_not_failures_are_not_rejected(self) -> None:
        # Справка, не провал: далёкая от HAL белая, EV у предела сенсора, NotFocused по центру.
        report = three_a_report()
        report["awb"]["awb_vs_hal"] = "warn"
        report["ev"]["ev_unverifiable_at_limit"] = True
        report["ev"]["points"][1].update(verifiable=False, ratio_to_baseline=1.0)
        report["focus"]["taps"][0] = {"x": 0.5, "y": 0.5, "result": "not_focused"}
        self.assertEqual(verdict_of(report).exit_code, smoke_wait.EXIT_OK)

    def test_each_rule_is_rechecked(self) -> None:
        cases = {
            "вспомогательный поток 3A": edited("live.companion", {"state": "unavailable"}),
            "AF_STATE": edited("live.af_state", None),
            "AE_STATE": edited("live.ae_state", None),
            "AWB не готов": edited("awb.ready", False),
            "скрыл 11 кадров": edited("awb.hidden", 11),
            "принудительных показов": edited("awb.forced", 2),
            "тап в центр закончился 'timed_out'": edited("focus.taps", [{"result": "timed_out"}] * 5),
            "trigger_not_delivered": edited("focus.taps.1.result", "trigger_not_delivered"),
            "без контекста запроса": edited("focus.results_without_request_context", 3),
            "отправлено триггеров 4": edited("focus.triggers_sent", 4),
            "region_effect_diopters": self._without("focus", "region_effect_diopters"),
            "ручное exposure_ns": edited("manual.applied.exposure_ns", 1_200_000),
            "ручное iso": edited("manual.applied.iso", 800),
            "ControlsAdjusted на значения в диапазоне": edited("manual.adjustments_for_in_range", 1),
            "LimitedByFrameDuration": edited("manual.long_shutter.limited_event_seen", False),
            "длинная выдержка в результате": edited("manual.long_shutter.applied_ns", 40_000_000),
            "точек EV меньше": edited("ev.points", []),
            "EV +1": edited("ev.points.1.ratio_to_baseline", 0.9),
            "AE_STATE не стал Locked": edited("lock.locked", False),
            "не Locked": edited("lock.non_locked_after_lock", 2),
            "непрерывный режим": edited("lock.back_to_continuous", False),
            "серия не снята": edited("bracket.captured", False),
            "не подряд": edited("bracket.frame_gaps", 1),
            "порядке индексов": edited("bracket.frames", [{"index": 1}, {"index": 0}]),
            "пропустил кадров серии": edited("bracket.skipped_delta", 0),
            "видоискатель не показал": edited("bracket.presented_during", 0),
            "не возвращены": edited("restore_failure", "очередь полна"),
            "состояние камеры": edited("state_at_end", "error"),
        }
        for needle, report in cases.items():
            with self.subTest(needle):
                verdict = verdict_of(report)
                self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED, verdict.message)
                self.assertIn(needle, verdict.message)

    @staticmethod
    def _without(section: str, key: str) -> dict:
        report = copy.deepcopy(three_a_report())
        del report[section][key]
        return report

    def test_rust_failure_is_a_failure(self) -> None:
        verdict = smoke_wait.evaluate_stream(
            [result_line({"scenario": "3a", "status": "failed", "error": "AE_STATE не стал Locked"})], "3a"
        )
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)

    def test_three_a_has_expectations_and_is_a_screenshot_scenario(self) -> None:
        self.assertIn("3a", smoke_wait.CHECKS)
        self.assertIn("3a", smoke_wait.SCREENSHOT_SCENARIOS)
        self.assertIn("3a", smoke_wait.NEUTRAL_CARD_SCENARIOS)


class DriverStepsTest(unittest.TestCase):
    def test_three_a_takes_two_shots_on_awb_ready_and_ignores_viewfinder_live(self) -> None:
        actions = RecordingActions()
        driver = smoke_driver.StageDriver("3a", actions)
        driver.handle_stage({"scenario": "3a", "stage": "viewfinder_live"})
        self.assertEqual(actions.calls, [])
        driver.handle_stage({"scenario": "3a", "stage": "awb_ready", "ready": True})
        self.assertEqual(actions.calls, ["screencap:viewfinder_a", "pause:1.5", "screencap:viewfinder_b"])


class DeviceSmokeThreeATest(unittest.TestCase):
    """``device-smoke.sh 3a`` на фейковом adb."""

    setUp = device_script.DeviceSmokeScenariosTest.setUp
    run_smoke = device_script.DeviceSmokeScenariosTest.run_smoke
    adb_calls = device_script.DeviceSmokeScenariosTest.adb_calls

    def test_three_a_runs_takes_screenshots_on_awb_ready_and_reports_a_pass(self) -> None:
        width, height = 8, 10
        self.dump_file.write_bytes(struct.pack("<IIII", width, height, 1, 0) + bytes((128, 128, 128, 255)) * 80)
        completed = self.run_smoke(
            "3a",
            [stage_line("3a", "awb_ready", ready=True), result_line(passed("3a", three_a_report()))],
            extra_arguments=("--artifacts-dir", str(self.work_dir / "shots")),
        )
        start = next(call for call in self.adb_calls() if "am start" in call)
        self.assertIn("-e scenario 3a", start)
        self.assertEqual(len([call for call in self.adb_calls() if call == "exec-out screencap"]), 2)
        # Скриншоты однотонные: пост-проверка «меняется» может провалить прогон, но отчёт 3a разобран.
        self.assertNotIn("AF_STATE", completed.stderr + completed.stdout)

    def test_neutral_card_is_accepted_for_three_a_but_not_for_other_scenarios(self) -> None:
        accepted = self.run_smoke("3a", [result_line(passed("3a", three_a_report()))], extra_arguments=("--neutral-card",))
        self.assertNotEqual(accepted.returncode, 64, accepted.stderr)
        rejected = self.run_smoke("camera-catalog", [], extra_arguments=("--neutral-card",))
        self.assertEqual(rejected.returncode, 64, rejected.stderr)


if __name__ == "__main__":
    unittest.main()


class OwnAeCheckTest(unittest.TestCase):
    """Сессия 08a: проверки отчёта ``3a`` при нашем AE."""

    @staticmethod
    def _edited_own(mutate) -> dict:
        report = own_ae_report()
        mutate(report)
        return report

    def test_tap_and_burst_missing_from_the_scenario_sample_is_not_a_failure(self) -> None:
        # Сценарий видит ~каждый третий кадр: тап и серия могут не попасть в выборку, полный учёт — у движка.
        report = own_ae_report()
        report["own_ae"]["results_checked"].update(focus_trigger=0, burst=0)
        self.assertEqual(verdict_of(report).exit_code, smoke_wait.EXIT_OK)

    def test_healthy_run_with_our_ae_passes_without_hal_ae_state(self) -> None:
        report = own_ae_report()
        report["live"]["ae_state"] = None  # у нашего AE AE_STATE справка, не правило
        self.assertEqual(verdict_of(report).exit_code, smoke_wait.EXIT_OK)

    def test_each_our_ae_rule_is_rechecked(self) -> None:
        cases = {
            "результатов с активным AE_STATE вендора 3": lambda r: r["own_ae"].update(vendor_ae_results=3),
            "не проверено ни одного результата повторяющегося запроса": lambda r: r["own_ae"]["results_checked"].update(
                repeating=0
            ),
            "движок с запуска насчитал 2 результатов с активным AE HAL": lambda r: r["own_ae"]["engine"].update(
                results_with_hal_ae_active=2
            ),
            "движок не насчитал результатов": lambda r: r["own_ae"]["engine"].update(results_observed=0),
            "нет учёта движка": lambda r: r["own_ae"].pop("engine"),
            "нет results_checked": lambda r: r["own_ae"].pop("results_checked"),
            "ни разу не дошёл": lambda r: r["own_ae"].update(
                settled_seen=False, holding_seen=False, limit_reached_seen=False
            ),
            "наш AE не сошёлся": lambda r: r["ev"]["points"][1].update(ae_converged=False),
            "не дошёл до Holding": lambda r: r["lock"].update(locked=False),
            "вышел из Holding": lambda r: r["lock"].update(non_locked_after_lock=2),
            "плавала": lambda r: r["lock"].update(max_product_deviation=0.2),
            "в результатах HAL нет AF_STATE": lambda r: r["live"].update(af_state=None),
        }
        for needle, mutate in cases.items():
            with self.subTest(needle):
                verdict = verdict_of(self._edited_own(mutate))
                self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED, verdict.message)
                self.assertIn(needle, verdict.message)

    def test_engine_running_another_ae_than_the_composition_chose_fails(self) -> None:
        own_but_vendor = three_a_report()
        own_but_vendor["expected_ae_source"] = "own"
        verdict = verdict_of(own_but_vendor)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED, verdict.message)
        self.assertIn("а композиция поставила 'own'", verdict.message)
        fallback = own_ae_report()
        fallback["expected_ae_source"] = "vendor"
        fallback["ae_fallback_reason"] = "рендер недоступен"
        verdict = verdict_of(fallback)
        self.assertIn("замена: рендер недоступен", verdict.message)

    def test_every_shifted_ev_point_at_the_limit_means_ev_was_not_verified(self) -> None:
        report = own_ae_report()
        for point in report["ev"]["points"][1:]:
            point.update(ae_converged=False, ae_limit_reached=True, verifiable=False, ratio_to_baseline=1.0)
        report["own_ae"].update(limit_reached_seen=True)
        verdict = verdict_of(report)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED, verdict.message)
        self.assertIn("EV не проверен", verdict.message)

    def test_limit_reached_point_is_an_honest_outcome_not_a_failure(self) -> None:
        report = own_ae_report()
        report["ev"]["points"][1].update(
            ae_converged=False, ae_limit_reached=True, verifiable=False, ratio_to_baseline=1.0
        )
        report["own_ae"].update(limit_reached_seen=True, worst_shortfall_ev=-1.0, target_not_reached_ticks=40)
        self.assertEqual(verdict_of(report).exit_code, smoke_wait.EXIT_OK)

    def test_own_ae_section_in_a_vendor_run_is_a_contradiction(self) -> None:
        report = three_a_report()
        report["own_ae"] = own_ae_report()["own_ae"]
        verdict = verdict_of(report)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED, verdict.message)
        self.assertIn("экспозицию вёл вендорский AE", verdict.message)


class LockEvidenceTest(unittest.TestCase):
    """Сессия 08c: свидетельства окна удержания блокировки нашего AE и пояснения к вердикту."""

    @staticmethod
    def _lock_edited(**changes: object) -> dict:
        report = own_ae_report()
        report["lock"].update(changes)
        return report

    def test_each_missing_or_malformed_evidence_field_fails(self) -> None:
        for key in ("observed_scene_range_ev", "product_frozen_after_frames", "prompt_prepare", "prompt_hold_start", "prompt_hold_end"):
            with self.subTest(f"нет {key}"):
                report = own_ae_report()
                del report["lock"][key]
                verdict = verdict_of(report)
                self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED, verdict.message)
                self.assertIn(f"нет поля {key}", verdict.message)
        malformed = {
            "observed_scene_range_ev": "много",
            "product_frozen_after_frames": 2.5,
            "prompt_hold_start": {"outcome": "queued"},
            "prompt_prepare": 5,
        }
        for key, value in malformed.items():
            with self.subTest(f"{key}={value!r}"):
                verdict = verdict_of(self._lock_edited(**{key: value}))
                self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED, verdict.message)
                self.assertIn(key, verdict.message)

    def test_null_evidence_is_valid_when_nothing_was_measured(self) -> None:
        report = self._lock_edited(observed_scene_range_ev=None, product_frozen_after_frames=None)
        self.assertEqual(verdict_of(report).exit_code, smoke_wait.EXIT_OK)

    def test_changed_scene_is_a_note_that_says_the_scene_changed(self) -> None:
        verdict = verdict_of(own_ae_report())
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK)
        self.assertTrue(any("менялась" in note and "2.40" in note for note in verdict.notes), verdict.notes)
        self.assertTrue(any("замерла через 9 кадров" in note for note in verdict.notes), verdict.notes)
        self.assertFalse(any("СТАТИЧНОЙ" in note for note in verdict.notes), verdict.notes)

    def test_static_scene_does_not_fail_but_the_verdict_says_so_loudly(self) -> None:
        verdict = verdict_of(self._lock_edited(observed_scene_range_ev=0.0))
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        self.assertTrue(any("СТАТИЧНОЙ сцене" in note for note in verdict.notes), verdict.notes)

    def test_missing_scene_measurement_and_frozen_product_are_noted(self) -> None:
        verdict = verdict_of(self._lock_edited(observed_scene_range_ev=None, product_frozen_after_frames=None))
        self.assertTrue(any("не получен" in note for note in verdict.notes), verdict.notes)
        self.assertTrue(any("не замерла" in note for note in verdict.notes), verdict.notes)

    def test_unshown_prompt_is_noted_with_its_moment(self) -> None:
        verdict = verdict_of(self._lock_edited(prompt_hold_start="not_attached"))
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        self.assertTrue(
            any("на экране не показано — подсказка начала окна" in note for note in verdict.notes), verdict.notes
        )
        self.assertFalse(any("подсказка конца окна" in note for note in verdict.notes), verdict.notes)

    def test_unshown_countdown_is_noted_but_a_missing_countdown_is_not(self) -> None:
        verdict = verdict_of(self._lock_edited(prompt_prepare="backpressure"))
        self.assertTrue(any("на экране не показано — отсчёт перед окном" in note for note in verdict.notes), verdict.notes)
        without_countdown = verdict_of(self._lock_edited(prompt_prepare=None))
        self.assertEqual(without_countdown.exit_code, smoke_wait.EXIT_OK, without_countdown.message)
        self.assertFalse(any("отсчёт" in note for note in without_countdown.notes), without_countdown.notes)

    def test_vendor_run_has_no_lock_notes(self) -> None:
        self.assertEqual(verdict_of(three_a_report()).notes, ())

    def test_vendor_ae_in_a_burst_or_tap_result_fails_even_when_repeating_is_clean(self) -> None:
        report = own_ae_report()
        report["own_ae"].update(vendor_ae_results=1, first_vendor_ae_frame=812)
        verdict = verdict_of(report)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED, verdict.message)
        self.assertIn("результатов с активным AE_STATE вендора 1", verdict.message)
