"""Тесты smoke-сценария ``screen`` (сессия 12): перепроверка отчёта (``smoke_checks_screen``) через сквозной
``evaluate_stream``, заметка о косвенных шагах, жесты из стадии в драйвере (точные команды ``adb``, контракт с Rust)
и ``device-smoke.sh``."""

from __future__ import annotations

import copy
import json
import subprocess
import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_checks_screen  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import smoke_driver  # noqa: E402
import smoke_wait  # noqa: E402
from test_smoke_driver import ADB, WM_SIZE, FakeAdb, RecordingActions, SizedAdb, make_actions  # noqa: E402
from test_smoke_glass import result_line, stage_line  # noqa: E402

CONTRACT = json.loads((Path(__file__).resolve().parent / "fixtures" / "smoke" / "screen_gesture_contract.json").read_text())
"""Жесты в том виде, в каком их пишет Rust (`screen_gesture.rs`); те же значения проверяет Rust-тест."""

STEP_NAMES = (
    "focus_tap_center",
    "focus_tap_top_left",
    "focus_tap_bottom_right",
    "lock_long_press",
    "unlock_tap",
    "menu_open_swipe",
    "menu_probe_tap_opened",
    "menu_reopen_swipe",
    "menu_close_swipe",
    "menu_probe_tap_closed",
    "ev_icon_tap",
    "ev_slider_swipe",
    "ev_reset_tap",
    "ev_close_tap",
    "ev_probe_tap_closed",
    "modes_swipe_left",
    "modes_swipe_right",
    "volume_up_shutter",
)


def healthy_report(**overrides: object) -> dict:
    """Здоровый отчёт ``screen``: 18 шагов прошли, экран без отказов, GPU UI 1,5 мс при бюджете 8,33 мс."""
    report: dict = {
        "failure": None,
        "window_ms": 20000,
        "steps": [{"name": name, "ok": True} for name in STEP_NAMES],
        "steps_inferred": 0,
        "unfinished": [],
        "plan_failure": None,
        "screen": {
            "frames_produced": 1200,
            "frame_failures": 0,
            "effects_overflowed": 0,
            "unhandled_gestures": 0,
            "gestures_cancelled": 0,
            "touches_received": 300,
            "input_overflowed": 0,
            "input_receiver_gone": 0,
            "input_dropped_without_screen": 0,
            "executor_refusals": 0,
            "haptics_backpressure": 0,
            "results_with_unknown_origin": 0,
        },
        "overlay": {
            "active_source": "camera_screen",
            "frames_drawn": 1200,
            "frames_failed": 0,
            "source_failures": 0,
            "limit_overflows": 0,
            "gpu_time": {"samples": 600, "p50_ms": 0.9, "p95_ms": 1.5, "max_ms": 2.0},
            "last_error": None,
        },
        "ui_gpu_budget": {"refresh_hz": 60.0, "fraction": 0.5, "budget_ms": 8.33},
        "late_frames": 0,
        "viewfinder": {
            "sensor_fps": 59.95,
            "isp_fps": 59.9,
            "frames_presented": 1200,
            "cpu_copies": 0,
            "shown_once_ratio": 1.0,
            "dropped": 0,
            "jitter_overflow": 0,
            "repeats": 0,
        },
        "validation": {"mode": "enabled", "errors": 0},
    }
    report.update(overrides)
    return report


def passed(report: dict) -> dict:
    return {"scenario": "screen", "status": "passed", "report": report}


class CheckScreenTest(unittest.TestCase):
    def problems(self, report: dict) -> list[str]:
        return smoke_checks_screen.check_screen(passed(report))

    def test_registered_for_the_wait_script_with_notes(self) -> None:
        self.assertIn("screen", smoke_wait.CHECKS)
        self.assertIn("screen", smoke_wait.NOTES)
        self.assertNotIn("screen", smoke_wait.SCREENSHOT_SCENARIOS)

    def test_green_result_passes_end_to_end_without_notes_when_every_step_is_proven_directly(self) -> None:
        # С прямым снимком состояния экрана (панель, плашка, шаги EV, режим) косвенных шагов нет: заметок нет.
        verdict = smoke_wait.evaluate_stream([result_line(passed(healthy_report()))], "screen")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        self.assertFalse(any("косвенно" in note for note in verdict.notes), verdict.notes)

    def test_green_result_with_inferred_steps_passes_with_a_note(self) -> None:
        verdict = smoke_wait.evaluate_stream([result_line(passed(healthy_report(steps_inferred=3)))], "screen")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        self.assertTrue(any("3 шагов доказаны косвенно" in note for note in verdict.notes), verdict.notes)

    def test_healthy_report_has_no_problems(self) -> None:
        self.assertEqual(self.problems(healthy_report()), [])

    def test_failed_scenario_status_is_a_red_verdict(self) -> None:
        failed = {"scenario": "screen", "status": "failed", "error": "шаг focus_tap_center: ..."}
        verdict = smoke_wait.evaluate_stream([result_line(failed)], "screen")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED, verdict.message)

    def test_a_step_that_did_not_pass_is_named(self) -> None:
        report = healthy_report()
        report["steps"][3] = {"name": "lock_long_press", "ok": False}
        problems = self.problems(report)
        self.assertTrue(any("lock_long_press" in problem for problem in problems), problems)

    def test_missing_required_step_and_unfinished_steps_are_problems(self) -> None:
        report = healthy_report()
        report["steps"] = [step for step in report["steps"] if step["name"] != "volume_up_shutter"]
        report["unfinished"] = ["volume_up_shutter"]
        problems = self.problems(report)
        self.assertTrue(any("обязательный шаг 'volume_up_shutter'" in problem for problem in problems), problems)
        self.assertTrue(any("не выполнены шаги плана" in problem for problem in problems), problems)

    def test_plan_failure_text_is_reported(self) -> None:
        problems = self.problems(healthy_report(plan_failure="шаг ev_icon_tap: ..."))
        self.assertTrue(any("план провален" in problem for problem in problems), problems)

    def test_each_nonzero_screen_counter_is_a_problem(self) -> None:
        for name in smoke_checks_screen.ZERO_SCREEN_COUNTERS:
            with self.subTest(name):
                report = healthy_report()
                report["screen"][name] = 2
                problems = self.problems(report)
                self.assertTrue(any(f"screen.{name}=2" in problem for problem in problems), problems)

    def test_a_screen_that_drew_nothing_is_a_problem(self) -> None:
        report = healthy_report()
        report["screen"]["frames_produced"] = 0
        self.assertTrue(any("screen.frames_produced" in problem for problem in self.problems(report)))

    def test_another_ui_source_and_ui_failures_are_problems(self) -> None:
        report = healthy_report()
        report["overlay"]["active_source"] = "glass_showcase"
        report["overlay"]["frames_failed"] = 3
        problems = self.problems(report)
        self.assertTrue(any("glass_showcase" in problem for problem in problems), problems)
        self.assertTrue(any("overlay.frames_failed=3" in problem for problem in problems), problems)

    def test_ui_gpu_time_over_the_budget_is_a_problem_and_equal_passes(self) -> None:
        report = healthy_report()
        report["overlay"]["gpu_time"]["p95_ms"] = 8.33
        self.assertEqual(self.problems(report), [])
        report["overlay"]["gpu_time"]["p95_ms"] = 8.34
        self.assertTrue(any("GPU-время кадра UI" in problem for problem in self.problems(report)))

    def test_late_frames_are_only_a_number(self) -> None:
        self.assertEqual(self.problems(healthy_report(late_frames=900)), [])
        self.assertTrue(any("late_frames" in problem for problem in self.problems(healthy_report(late_frames=None))))

    def test_viewfinder_must_be_alive_and_validation_clean(self) -> None:
        report = healthy_report()
        report["viewfinder"]["cpu_copies"] = 1
        report["validation"]["errors"] = 2
        problems = self.problems(report)
        self.assertTrue(any("cpu_copies" in problem for problem in problems), problems)
        self.assertTrue(any("validation" in problem for problem in problems), problems)

    def test_empty_report_gives_problems_not_a_crash(self) -> None:
        problems = smoke_checks_screen.check_screen({"scenario": "screen", "status": "passed", "report": {}})
        self.assertTrue(problems)
        self.assertEqual(copy.deepcopy(problems), problems)

    def test_ev_steps_note_says_the_proof_stops_at_the_engine(self) -> None:
        report = healthy_report(steps_inferred=0)
        report["ev_proof"] = "engine_desired_value"
        notes = smoke_checks_screen.screen_notes(passed(report))
        self.assertEqual(len(notes), 1, notes)
        self.assertIn("дошло до движка", notes[0])
        self.assertIn("HAL", notes[0])
        # Заметка не делает результат красным.
        self.assertEqual(self.problems(report), [])

    def test_unknown_ev_proof_value_gives_no_note(self) -> None:
        report = healthy_report(steps_inferred=0)
        report["ev_proof"] = "something_else"
        self.assertEqual(smoke_checks_screen.screen_notes(passed(report)), [])

    def test_no_note_when_every_step_is_direct(self) -> None:
        self.assertEqual(smoke_checks_screen.screen_notes(passed(healthy_report(steps_inferred=0))), [])


class StageGestureTest(unittest.TestCase):
    """Жест из поля ``gesture`` стадии → вызов действия (контракт с Rust: `screen_gesture.rs`)."""

    def drive(self, gesture: object, actions: object | None = None) -> RecordingActions:
        recording = actions if actions is not None else RecordingActions()
        smoke_driver.StageDriver("screen", recording).handle_stage(
            {"scenario": "screen", "stage": "screen_gesture", "step": "probe", "gesture": gesture}
        )
        return recording  # type: ignore[return-value]

    def test_the_rust_contract_reaches_each_action_with_the_right_arguments(self) -> None:
        self.assertEqual(self.drive(CONTRACT["tap"]).calls, ["tap_point(0.5,0.25)"])
        self.assertEqual(self.drive(CONTRACT["long_press"]).calls, ["long_press_point(0.1,0.2,800)"])
        self.assertEqual(self.drive(CONTRACT["swipe"]).calls, ["swipe(0.5,0.6->0.5,0.4,300)"])
        self.assertEqual(self.drive(CONTRACT["volume_up"]).calls, ["press_volume_up"])

    def test_the_stage_name_is_the_one_rust_emits(self) -> None:
        self.assertEqual(list(smoke_driver.SCENARIO_STEPS["screen"]), ["screen_gesture"])

    def test_other_stages_and_other_scenarios_do_nothing(self) -> None:
        actions = RecordingActions()
        driver = smoke_driver.StageDriver("screen", actions)
        driver.handle_stage({"scenario": "screen", "stage": "screen_ready", "steps": 18})
        driver.handle_stage({"scenario": "glass", "stage": "screen_gesture", "gesture": CONTRACT["tap"]})
        self.assertEqual(actions.calls, [])

    def test_broken_gestures_are_driver_errors_naming_the_step_and_touch_nothing(self) -> None:
        broken = [
            None,
            "tap",
            {"kind": "pinch"},
            {"kind": "tap", "x": 0.5},
            {"kind": "tap", "x": 1.5, "y": 0.5},
            {"kind": "tap", "x": True, "y": 0.5},
            {"kind": "tap", "x": "0.5", "y": 0.5},
            {"kind": "swipe", "x": 0.5, "y": 0.5, "end_x": 0.5, "end_y": 0.4, "duration_ms": 0},
            {"kind": "swipe", "x": 0.5, "y": 0.5, "end_x": 0.5, "end_y": 0.4, "duration_ms": 12.5},
            {"kind": "long_press", "x": 0.5, "y": 0.5},
        ]
        for gesture in broken:
            with self.subTest(gesture=gesture):
                actions = RecordingActions()
                with self.assertRaises(smoke_driver.DriverError) as raised:
                    self.drive(gesture, actions)
                self.assertIn("probe", str(raised.exception))
                self.assertEqual(actions.calls, [])

    def test_volume_key_is_the_exact_adb_command_and_a_failure_is_reported(self) -> None:
        adb = FakeAdb()
        make_actions(adb).press_volume_up()
        self.assertEqual(adb.commands, [[*ADB, "shell", "input", "keyevent", "KEYCODE_VOLUME_UP"]])
        failing = FakeAdb(failing_prefix=[*ADB, "shell", "input", "keyevent"])
        with self.assertRaises(smoke_driver.DriverError):
            make_actions(failing).press_volume_up()

    def test_contract_gestures_become_exact_adb_commands_through_the_real_driver(self) -> None:
        adb = SizedAdb(1440, 3216)
        driver = smoke_driver.StageDriver("screen", make_actions(adb))
        for kind in ("tap", "long_press", "swipe", "volume_up"):
            driver.handle_stage({"scenario": "screen", "stage": "screen_gesture", "step": kind, "gesture": CONTRACT[kind]})
        self.assertEqual(
            adb.commands,
            [
                WM_SIZE,
                [*ADB, "shell", "input", "tap", "720", "804"],
                WM_SIZE,
                [*ADB, "shell", "input", "swipe", "144", "643", "144", "643", "800"],
                WM_SIZE,
                [*ADB, "shell", "input", "swipe", "720", "1929", "720", "1286", "300"],
                [*ADB, "shell", "input", "keyevent", "KEYCODE_VOLUME_UP"],
            ],
        )

    def test_scenario_stream_runs_gestures_between_stage_and_result(self) -> None:
        actions = RecordingActions()
        driver = smoke_driver.StageDriver("screen", actions)
        lines = [
            stage_line("screen", "screen_gesture", step="focus_tap_center", gesture=CONTRACT["tap"]),
            stage_line("screen", "screen_gesture", step="volume_up_shutter", gesture=CONTRACT["volume_up"]),
            result_line(passed(healthy_report())),
        ]
        verdict = smoke_wait.evaluate_stream(lines, "screen", driver.handle_stage)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        self.assertEqual(actions.calls, ["tap_point(0.5,0.25)", "press_volume_up"])


class DeviceSmokeScriptTest(unittest.TestCase):
    def test_usage_lists_the_screen_scenario(self) -> None:
        completed = subprocess.run(
            ["bash", str(SCRIPTS_DIR / "device-smoke.sh"), "--help"], capture_output=True, text=True, check=False
        )
        self.assertEqual(completed.returncode, 0)
        self.assertIn("screen (180", completed.stderr)
        self.assertIn("Разрешение камеры у пакета", completed.stderr, "шапка не обрезана диапазоном sed")


if __name__ == "__main__":
    unittest.main()
