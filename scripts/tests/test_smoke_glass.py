"""Тесты smoke-сценария ``glass`` (сессия 11): перепроверка отчёта (``smoke_checks_glass``) через сквозной
``evaluate_stream``, заметки о гравитации, шаги драйвера (скриншоты на ``glass_live``) и ``device-smoke.sh``
на фейковом adb."""

from __future__ import annotations

import copy
import json
import struct
import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_checks_glass  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import smoke_driver  # noqa: E402
import smoke_wait  # noqa: E402
import test_smoke_device_script as device_script  # noqa: E402


def stage_line(scenario: str, stage: str, **extra: object) -> str:
    payload = {"scenario": scenario, "stage": stage, **extra}
    return f"  1792957061.123  4242  4242 I Fasticamera: FASTICAMERA_STAGE {json.dumps(payload)}\n"


def result_line(payload: dict) -> str:
    return f"  1792957062.000  4242  4242 I Fasticamera: FASTICAMERA_RESULT {json.dumps(payload)}\n"


def glass_report(**overrides: object) -> dict:
    """Здоровый отчёт ``glass``: 30 с, 1800 кадров UI, GPU p95 1,5 мс при бюджете 4,17 мс, 0 пропусков."""
    report: dict = {
        "failure": None,
        "window_ms": 30000,
        "overlay": {
            "active_source": "glass_showcase",
            "installs": 1,
            "removals": 0,
            "frames_drawn": 1800,
            "frames_failed": 0,
            "source_failures": 0,
            "limit_overflows": 0,
            "pyramid_rebuilds": 12,
            "pyramid_reuses": 1788,
            "gpu_time": {"samples": 600, "p50_ms": 0.9, "p95_ms": 1.5, "max_ms": 2.0},
            "last_error": None,
        },
        "ui_gpu_budget": {"refresh_hz": 60.0, "fraction": 0.25, "budget_ms": 4.17},
        "missed_frames": 0,
        "missed_frames_allowed": 0,
        "source_removal": {"state": "delivered"},
        "gravity": {"state": "active", "source": "dedicated_sensor", "samples_in_window": 6000},
        "notes": [],
        "viewfinder": {
            "sensor_fps": 59.95,
            "isp_fps": 59.9,
            "frames_processed": 1800,
            "frames_presented": 1800,
            "cpu_copies": 0,
            "shown_once_ratio": 0.999,
            "dropped": 1,
            "jitter_overflow": 1,
            "repeats": 0,
        },
        "validation": {"mode": "enabled", "errors": 0},
    }
    report.update(overrides)
    return report


def with_overlay(**changes: object) -> dict:
    report = glass_report()
    report["overlay"] = {**report["overlay"], **changes}
    return report


def with_viewfinder(**changes: object) -> dict:
    report = glass_report()
    report["viewfinder"] = {**report["viewfinder"], **changes}
    return report


def verdict_of(report: dict, stages: list[str] | None = None) -> smoke_wait.SmokeVerdict:
    payload = {"scenario": "glass", "status": "passed", "report": report}
    return smoke_wait.evaluate_stream([*(stages or []), result_line(payload)], "glass")


class GlassCheckTest(unittest.TestCase):
    def test_healthy_glass_passes(self) -> None:
        verdict = verdict_of(glass_report())
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        self.assertEqual(verdict.notes, ())

    def test_each_rule_is_rechecked_and_names_its_reason(self) -> None:
        cases = {
            "источник UI не установлен": with_overlay(active_source=None),
            "ни одного кадра": with_overlay(frames_drawn=0),
            "frames_failed": with_overlay(frames_failed=2, last_error="список рисования отклонён"),
            "source_failures": with_overlay(source_failures=1),
            "limit_overflows": with_overlay(limit_overflows=1),
            "пирамида размытия": with_overlay(pyramid_rebuilds=0),
            "нет GPU-времени": with_overlay(gpu_time=None),
            "больше бюджета": with_overlay(gpu_time={"samples": 600, "p50_ms": 2.0, "p95_ms": 4.5, "max_ms": 5.0}),
            "пропущено кадров: 2": glass_report(missed_frames=2),
            "не снят": glass_report(source_removal={"state": "not_delivered", "reason": "рендер завершён"}),
            "ISP 40": with_viewfinder(isp_fps=40.0),
            "cpu_copies": with_viewfinder(cpu_copies=1),
            "не показал кадров": with_viewfinder(frames_presented=0),
            "показано ровно один раз": with_viewfinder(shown_once_ratio=0.8),
            "validation": glass_report(validation={"mode": "enabled", "errors": 3}),
            "длины окна": glass_report(window_ms=0),
        }
        for expected, report in cases.items():
            with self.subTest(expected=expected):
                verdict = verdict_of(report)
                self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED, verdict.message)
                self.assertIn(expected, verdict.message)

    def test_budget_edge_and_validation_disabled_in_release(self) -> None:
        at_budget = with_overlay(gpu_time={"samples": 600, "p50_ms": 2.0, "p95_ms": 4.17, "max_ms": 5.0})
        self.assertEqual(verdict_of(at_budget).exit_code, smoke_wait.EXIT_OK)
        # release: validation-слоя нет, ошибок «не бывает» — это не провал.
        release = glass_report(validation={"mode": "disabled", "errors": 0})
        self.assertEqual(verdict_of(release).exit_code, smoke_wait.EXIT_OK)

    def test_missed_frames_follow_the_allowance_from_the_report(self) -> None:
        tolerant = glass_report(missed_frames=2, missed_frames_allowed=3)
        self.assertEqual(verdict_of(tolerant).exit_code, smoke_wait.EXIT_OK)
        missing = glass_report()
        del missing["missed_frames_allowed"]
        self.assertIn("нет счёта пропущенных кадров", verdict_of(missing).message)

    def test_missing_sections_are_reported_not_guessed(self) -> None:
        for section, expected in (("overlay", "раздела overlay"), ("viewfinder", "раздела viewfinder")):
            with self.subTest(section=section):
                report = glass_report()
                del report[section]
                self.assertIn(expected, verdict_of(report).message)
        no_report = smoke_wait.evaluate_stream(
            [result_line({"scenario": "glass", "status": "passed"})], "glass"
        )
        self.assertIn("нет report", no_report.message)

    def test_a_failed_scenario_is_a_failure_with_its_status(self) -> None:
        payload = {"scenario": "glass", "status": "failed", "error": "источник UI не установлен: рендер не поднимался"}
        verdict = smoke_wait.evaluate_stream([result_line(payload)], "glass")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("рендер не поднимался", verdict.message)


class GlassGravityTest(unittest.TestCase):
    def test_unavailable_gravity_passes_with_a_note(self) -> None:
        note = "гравитация недоступна (на телефоне нет датчика гравитации): блик от наклона не проверен"
        report = glass_report(gravity={"state": "unavailable", "reason": "нет датчика"}, notes=[note])
        verdict = verdict_of(report)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        self.assertEqual(verdict.notes, (note,))

    def test_a_silent_sensor_passes_with_a_note(self) -> None:
        report = glass_report(
            gravity={"state": "active", "source": "filtered_accelerometer", "samples_in_window": 0},
            notes=["гравитация (filtered_accelerometer) не дала ни одного показания за окно"],
        )
        verdict = verdict_of(report)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        self.assertEqual(len(verdict.notes), 1)

    def test_malformed_gravity_fails(self) -> None:
        self.assertIn("gravity.state", verdict_of(glass_report(gravity={"state": "maybe"})).message)
        no_count = glass_report(gravity={"state": "active", "source": "injected"})
        self.assertIn("samples_in_window", verdict_of(no_count).message)

    def test_notes_helper_ignores_foreign_shapes(self) -> None:
        self.assertEqual(smoke_checks_glass.glass_notes({"report": {"notes": "строка"}}), [])
        self.assertEqual(smoke_checks_glass.glass_notes({"report": {"notes": ["a", 3, "b"]}}), ["a", "b"])
        self.assertEqual(smoke_checks_glass.glass_notes({}), [])


class RecordingActions:
    """Фейк ``DeviceActions``: только записывает вызовы."""

    def __init__(self) -> None:
        self.calls: list[str] = []

    def __getattr__(self, name: str):
        return lambda *arguments: self.calls.append(name if not arguments else f"{name}:{arguments[0]}")


class GlassDriverTest(unittest.TestCase):
    def test_two_shots_are_taken_on_glass_live_only(self) -> None:
        actions = RecordingActions()
        driver = smoke_driver.StageDriver("glass", actions)
        driver.handle_stage({"scenario": "glass", "stage": "glass_live"})
        self.assertEqual(actions.calls, ["screencap:viewfinder_a", "pause:1.5", "screencap:viewfinder_b"])
        quiet = RecordingActions()
        other = smoke_driver.StageDriver("glass", quiet)
        for stage in ("started", "viewfinder_live", "progress", "detail_glass"):
            other.handle_stage({"scenario": "glass", "stage": stage})
        self.assertEqual(quiet.calls, [], "скриншоты только на glass_live, а не на viewfinder_live под ним")

    def test_glass_is_a_registered_screenshot_scenario(self) -> None:
        self.assertIn("glass", smoke_wait.CHECKS)
        self.assertIn("glass", smoke_wait.NOTES)
        self.assertIn("glass", smoke_wait.SCREENSHOT_SCENARIOS)
        self.assertEqual(set(smoke_driver.SCENARIO_STEPS["glass"]), {"glass_live"})
        self.assertNotIn("glass", smoke_wait.NEUTRAL_CARD_SCENARIOS)


class DeviceSmokeGlassTest(unittest.TestCase):
    """``device-smoke.sh glass`` на фейковом adb (тот же, что у ``test_smoke_device_script``)."""

    setUp = device_script.DeviceSmokeScenariosTest.setUp
    run_smoke = device_script.DeviceSmokeScenariosTest.run_smoke
    adb_calls = device_script.DeviceSmokeScenariosTest.adb_calls

    def test_glass_takes_two_screenshots_into_a_fresh_artifacts_dir_and_judges_them(self) -> None:
        # Фейковый экран не меняется между снимками: Rust-отчёт здоров, провал даёт только пост-проверка скриншотов.
        width, height = 8, 10
        pixels = bytes((90, 120, 60, 255)) * (width * height)
        self.dump_file.write_bytes(struct.pack("<IIII", width, height, 1, 0) + pixels)
        artifacts = self.work_dir / "shots"
        payload = {"scenario": "glass", "status": "passed", "report": copy.deepcopy(glass_report())}
        completed = self.run_smoke(
            "glass",
            [stage_line("glass", "glass_live"), result_line(payload)],
            extra_arguments=("--artifacts-dir", str(artifacts)),
        )
        screencaps = [call for call in self.adb_calls() if call == "exec-out screencap"]
        self.assertEqual(len(screencaps), 2, self.adb_calls())
        self.assertEqual(completed.returncode, 1, completed.stderr)
        self.assertIn("не меняется", completed.stderr)
        self.assertIn(f"каталог артефактов (скриншоты): {artifacts}", completed.stderr)

    def test_neutral_card_has_no_meaning_for_glass(self) -> None:
        completed = self.run_smoke("glass", [], extra_arguments=("--neutral-card",))
        self.assertEqual(completed.returncode, 64, completed.stderr)


if __name__ == "__main__":
    unittest.main()
