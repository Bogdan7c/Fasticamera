"""Сессия 16a: `viewfinder-stage-budget` на продуктовом экране — шаги драйвера (свайпы входа и выхода, удерживаемое
касание фоновым процессом), проверка результата с новыми строками и флаги `device-smoke.sh` (без телефона)."""

from __future__ import annotations

import subprocess
import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_driver  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import smoke_wait  # noqa: E402
import test_smoke_device_script as device_script  # noqa: E402
from smoke_driver_background import BackgroundGestureError, BackgroundGestures  # noqa: E402
from test_smoke_driver import ADB, PACKAGE, WM_SIZE, RecordingActions, SizedAdb  # noqa: E402
from test_smoke_viewfinder import passed, result_line, stage_budget_look_fields  # noqa: E402

SCENARIO = "viewfinder-stage-budget"


def swipe_stage(step: str = "modes_swipe_left") -> dict:
    return {
        "scenario": SCENARIO,
        "stage": "budget_gesture",
        "step": step,
        "gesture": {"kind": "swipe", "x": 0.75, "y": 0.45, "end_x": 0.25, "end_y": 0.45, "duration_ms": 250},
    }


def hold_stage(window: int, needs_new_press: bool, hold_ms: int = 60_000) -> dict:
    return {
        "scenario": SCENARIO,
        "stage": "budget_hold_touch",
        "row": "display_120hz",
        "window": window,
        "needs_new_press": needs_new_press,
        "windows_covered": 1,
        "gesture": {"kind": "long_press", "x": 0.03, "y": 0.85, "hold_ms": hold_ms},
    }


class FakeProcess:
    """Фоновый процесс: код выхода задаёт тест; `finish_at` — когда (по часам) он «заканчивается»."""

    def __init__(self, clock: "FakeClock", finish_at: float | None, code: int = 0, stderr: str = "") -> None:
        self._clock = clock
        self._finish_at = finish_at
        self._code = code
        self._stderr = stderr
        self.killed = False

    def poll(self) -> int | None:
        if self._finish_at is not None and self._clock.now >= self._finish_at:
            return self._code
        return None

    def wait(self, timeout: float | None = None) -> int:
        if self._finish_at is not None and (timeout is None or self._clock.now + timeout >= self._finish_at):
            self._clock.now = max(self._clock.now, self._finish_at)
            return self._code
        raise subprocess.TimeoutExpired("adb", timeout or 0.0)

    def kill(self) -> None:
        self.killed = True

    def stderr_text(self) -> str:
        return self._stderr


class FakeClock:
    def __init__(self) -> None:
        self.now = 0.0

    def monotonic(self) -> float:
        return self.now


class BackgroundGesturesTest(unittest.TestCase):
    def test_start_returns_at_once_without_waiting_for_the_gesture(self) -> None:
        clock = FakeClock()
        process = FakeProcess(clock, finish_at=60.0)
        spawned: list[list[str]] = []

        def spawn(command: list[str]) -> FakeProcess:
            spawned.append(command)
            return process

        gestures = BackgroundGestures(spawn, clock.monotonic)
        gestures.start(["adb", "hold"], "удержание", 60.0)
        self.assertEqual(spawned, [["adb", "hold"]])
        self.assertEqual(clock.now, 0.0, "драйвер не ждал минуту")
        self.assertIsNone(process.poll())

    def test_wait_all_returns_when_every_process_finished_successfully(self) -> None:
        clock = FakeClock()
        processes = iter([FakeProcess(clock, 60.0), FakeProcess(clock, 120.0)])
        gestures = BackgroundGestures(lambda _command: next(processes), clock.monotonic)
        gestures.start(["adb", "a"], "первое", 60.0)
        gestures.start(["adb", "b"], "второе", 120.0)
        gestures.wait_all()
        self.assertEqual(clock.now, 120.0)
        gestures.wait_all()  # повторный вызов безвреден: ждать нечего

    def test_a_process_that_failed_is_reported_by_the_next_start_not_swallowed(self) -> None:
        clock = FakeClock()
        broken = FakeProcess(clock, 1.0, code=1, stderr="device offline")
        processes = iter([broken, FakeProcess(clock, 100.0)])
        gestures = BackgroundGestures(lambda _command: next(processes), clock.monotonic)
        gestures.start(["adb", "a"], "удержание в (43, 2733)", 60.0)
        clock.now = 5.0
        with self.assertRaisesRegex(BackgroundGestureError, r"удержание в \(43, 2733\).*кодом 1.*device offline"):
            gestures.start(["adb", "b"], "второе", 60.0)

    def test_a_hung_process_is_killed_and_reported_at_the_end(self) -> None:
        clock = FakeClock()
        hung = FakeProcess(clock, finish_at=None)
        gestures = BackgroundGestures(lambda _command: hung, clock.monotonic)
        gestures.start(["adb", "a"], "удержание", 10.0)
        with self.assertRaisesRegex(BackgroundGestureError, "не закончилось к сроку"):
            gestures.wait_all()
        self.assertTrue(hung.killed)

    def test_a_failure_discovered_at_the_end_is_reported(self) -> None:
        clock = FakeClock()
        broken = FakeProcess(clock, 3.0, code=255, stderr="boom")
        gestures = BackgroundGestures(lambda _command: broken, clock.monotonic)
        gestures.start(["adb", "a"], "удержание", 3.0)
        with self.assertRaisesRegex(BackgroundGestureError, "кодом 255.*boom"):
            gestures.wait_all()


class BackgroundRecordingActions(RecordingActions):
    """Фейковые действия с журналом вызовов, включая фоновые нажатия."""

    def hold_point_in_background(self, point: smoke_driver.ScreenFraction, duration: smoke_driver.DurationMs) -> None:
        self.calls.append(f"hold_in_background({point.x:g},{point.y:g},{duration.value})")

    def wait_for_background_gestures(self) -> None:
        self.calls.append("wait_for_background")


class StageDriverTableTest(unittest.TestCase):
    def drive(self, *stages: dict) -> list[str]:
        actions = BackgroundRecordingActions()
        driver = smoke_driver.StageDriver(SCENARIO, actions)
        for stage in stages:
            driver.handle_stage(stage)
        driver.finish()
        return actions.calls

    def test_entry_and_exit_swipes_are_performed_from_the_stage_gesture(self) -> None:
        calls = self.drive(swipe_stage("modes_swipe_left"), swipe_stage("modes_swipe_right"))
        self.assertEqual(
            calls[:2], ["swipe(0.75,0.45->0.25,0.45,250)", "swipe(0.75,0.45->0.25,0.45,250)"]
        )

    def test_hold_is_started_in_the_background_once_per_press_and_never_blocks(self) -> None:
        calls = self.drive(
            hold_stage(0, True, 60_000), hold_stage(3, True, 120_000), hold_stage(4, False, 60_000)
        )
        # Окно 4 покрыто нажатием окна 3 (`needs_new_press: false`) — второго нажатия нет; в конце — ожидание.
        self.assertEqual(
            calls,
            ["hold_in_background(0.03,0.85,60000)", "hold_in_background(0.03,0.85,120000)", "wait_for_background"],
        )

    def test_background_step_accepts_only_a_long_press(self) -> None:
        bad = {**hold_stage(0, True), "gesture": {"kind": "tap", "x": 0.5, "y": 0.5}}
        with self.assertRaisesRegex(smoke_driver.DriverError, "только long_press"):
            self.drive(bad)

    def test_other_scenarios_and_stages_do_nothing(self) -> None:
        self.assertEqual(self.drive({**hold_stage(0, True), "scenario": "screen"}), ["wait_for_background"])


class AdbBackgroundHoldTest(unittest.TestCase):
    def test_hold_is_one_input_swipe_command_started_in_the_background_after_reading_the_screen_size(self) -> None:
        adb = SizedAdb(1440, 3216)
        spawned: list[list[str]] = []
        clock = FakeClock()

        def spawn(command: list[str]) -> FakeProcess:
            spawned.append(command)
            return FakeProcess(clock, finish_at=60.0)

        actions = smoke_driver.AdbDeviceActions(
            "FAKE123", PACKAGE, run_adb=adb, spawn_adb=spawn, clock=clock.monotonic
        )
        driver = smoke_driver.StageDriver(SCENARIO, actions)
        driver.handle_stage(hold_stage(0, True, 60_000))
        # ровно одна команда `input` на всё нажатие, в фоне; синхронно ушёл только `wm size`
        self.assertEqual(adb.commands, [WM_SIZE])
        self.assertEqual(
            spawned, [[*ADB, "shell", "input", "swipe", "43", "2733", "43", "2733", "60000"]]
        )
        self.assertEqual(clock.now, 0.0, "драйвер не ждал минуту")
        driver.finish()
        self.assertEqual(clock.now, 60.0, "в конце прогона драйвер дождался нажатия")

    def test_finish_turns_a_background_failure_into_a_driver_error(self) -> None:
        clock = FakeClock()
        actions = smoke_driver.AdbDeviceActions(
            "FAKE123",
            PACKAGE,
            run_adb=SizedAdb(1080, 2412),
            spawn_adb=lambda command: FakeProcess(clock, 1.0, code=1, stderr="no device"),
            clock=clock.monotonic,
        )
        driver = smoke_driver.StageDriver(SCENARIO, actions)
        driver.handle_stage(hold_stage(0, True, 1_000))
        with self.assertRaisesRegex(smoke_driver.DriverError, "no device"):
            driver.finish()


class FinishDriverTest(unittest.TestCase):
    def test_a_background_failure_turns_a_passed_verdict_into_a_failure_and_keeps_details(self) -> None:
        class FailingDriver:
            def finish(self) -> None:
                raise smoke_driver.DriverError("нажатие упало")

        verdict = smoke_wait.SmokeVerdict(smoke_wait.EXIT_OK, "прошёл", details=({"k": 1},))
        outcome = smoke_wait._finish_driver(FailingDriver(), verdict)
        self.assertEqual(outcome.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("нажатие упало", outcome.message)
        self.assertEqual(outcome.details, ({"k": 1},), "подробности не потеряны")

    def test_a_clean_finish_leaves_the_verdict_alone(self) -> None:
        class CleanDriver:
            def finish(self) -> None:
                return None

        verdict = smoke_wait.SmokeVerdict(smoke_wait.EXIT_OK, "прошёл")
        self.assertIs(smoke_wait._finish_driver(CleanDriver(), verdict), verdict)

    def test_a_failure_does_not_hide_an_existing_failed_verdict(self) -> None:
        class FailingDriver:
            def finish(self) -> None:
                raise smoke_driver.DriverError("нажатие упало")

        verdict = smoke_wait.SmokeVerdict(smoke_wait.EXIT_SCENARIO_FAILED, "сценарий не прошёл: причина")
        outcome = smoke_wait._finish_driver(FailingDriver(), verdict)
        self.assertEqual(outcome.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("сценарий не прошёл: причина", outcome.message)
        self.assertIn("нажатие упало", outcome.message)


def budget_report(**overrides: object) -> dict:
    report: dict = {
        **stage_budget_look_fields(),
        "surface": "calibrate_screen",
        "requested_rows": ["color_scopes", "whole_product"],
        "table": [
            "color_scopes | 0.80 / 1.10 | 14.0 ± 2.0 | measured",
            "whole_product | 2.80 / 3.00 | 70.0 ± 4.0 | measured",
            "camera_without_product_gpu | 0.70 / 1.00 | 480.0 (абс.) | baseline",
        ],
        "scopes": {
            "request": {"kind": "waveform", "image_size": {"width": 640, "height": 360}},
            "float32_controls": {"rounding_mode_rte": True},
        },
    }
    report.update(overrides)
    return report


class StageBudgetCheckTest(unittest.TestCase):
    def check(self, report: dict) -> list[str]:
        import smoke_checks_viewfinder

        return smoke_checks_viewfinder.check_viewfinder_stage_budget(passed(SCENARIO, report))

    def test_a_calibrate_result_with_new_rows_the_surface_and_the_scopes_is_accepted(self) -> None:
        self.assertEqual(self.check(budget_report()), [])

    def test_whole_product_needs_its_own_baseline_row(self) -> None:
        report = budget_report(table=budget_report()["table"][:2])
        self.assertTrue(any("camera_without_product_gpu" in problem for problem in self.check(report)))

    def test_the_baseline_line_is_not_counted_as_a_requested_row(self) -> None:
        report = budget_report(requested_rows=["color_scopes", "whole_product", "ui_overlay"])
        self.assertTrue(any("строк таблицы 2, запрошено 3" in problem for problem in self.check(report)))

    def test_surface_must_be_named_and_known(self) -> None:
        missing = budget_report()
        del missing["surface"]
        self.assertTrue(any("surface" in problem for problem in self.check(missing)))
        self.assertTrue(any("surface" in problem for problem in self.check(budget_report(surface="video_screen"))))

    def test_a_row_that_does_not_fit_the_surface_is_a_problem(self) -> None:
        report = budget_report(surface="photo_screen")
        problems = self.check(report)
        self.assertTrue(any("color_scopes не мерится на месте photo_screen" in problem for problem in problems), problems)

    def test_calibrate_result_must_carry_the_scopes_request_and_float32_controls(self) -> None:
        problems = self.check(budget_report(scopes={}))
        self.assertTrue(any("scopes.request" in problem for problem in problems), problems)
        self.assertTrue(any("scopes.float32_controls" in problem for problem in problems), problems)

    def test_a_state_rejected_row_is_a_problem_naming_the_reason_even_if_the_status_was_passed(self) -> None:
        for reason in ("state_not_applied", "display_refresh_not_reached"):
            with self.subTest(reason=reason):
                report = budget_report()
                report["table"] = [f"color_scopes | 0.80 / 1.10 | — | rejected: {reason}", *report["table"][1:]]
                problems = self.check(report)
                self.assertTrue(
                    any("состояние не подтвердилось" in problem and reason in problem for problem in problems), problems
                )

    def test_a_current_rejection_is_told_apart_from_a_state_rejection(self) -> None:
        report = budget_report()
        report["table"] = ["color_scopes | нет замеров | — | rejected: window", *report["table"][1:]]
        self.assertTrue(any("ток не измерен" in problem for problem in self.check(report)))

    def test_display_row_is_allowed_on_every_surface(self) -> None:
        for surface in ("bare_viewfinder", "photo_screen", "calibrate_screen"):
            report = budget_report(
                surface=surface,
                requested_rows=["display_120hz"],
                table=["display_120hz | 0.00 / 0.00 | 10.0 ± 1.0 | measured"],
            )
            if surface != "calibrate_screen":
                report["scopes"] = {}
            self.assertEqual(self.check(report), [], surface)


class DeviceSmokeSurfaceFlagsTest(unittest.TestCase):
    """`device-smoke.sh` на фейковом adb (тот же, что у `test_smoke_device_script`)."""

    setUp = device_script.DeviceSmokeScenariosTest.setUp
    run_smoke = device_script.DeviceSmokeScenariosTest.run_smoke
    adb_calls = device_script.DeviceSmokeScenariosTest.adb_calls
    index_of = device_script.DeviceSmokeScenariosTest.index_of

    OK = passed(
        SCENARIO,
        {
            **stage_budget_look_fields(),
            "surface": "photo_screen",
            "requested_rows": ["ui_overlay"],
            "table": ["ui_overlay | 1.50 / 1.80 | 20.0 ± 1.0 | measured"],
        },
    )

    def start_call(self) -> str:
        return next(call for call in self.adb_calls() if "am start" in call)

    def test_surface_ab_pairs_and_new_rows_reach_the_intent_as_extras(self) -> None:
        completed = self.run_smoke(
            SCENARIO,
            [result_line(self.OK)],
            extra_arguments=(
                "--surface", "photo_screen", "--stages", "ui_overlay,display_120hz", "--ab-pairs", "2",
            ),
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        start = self.start_call()
        self.assertIn("-e stages ui_overlay,display_120hz -e surface photo_screen", start)
        self.assertIn("-e ab_pairs 2", start)

    def test_without_the_new_flags_no_new_extra_is_sent(self) -> None:
        completed = self.run_smoke(SCENARIO, [result_line(self.OK)])
        self.assertEqual(completed.returncode, 0, completed.stderr)
        start = self.start_call()
        self.assertNotIn("surface", start)
        self.assertNotIn("ab_pairs", start)

    def test_bad_surface_pairs_or_the_wrong_scenario_are_usage_errors(self) -> None:
        for arguments, scenario in (
            (("--surface", "video_screen"), SCENARIO),
            (("--surface", "photo_screen;ls"), SCENARIO),
            (("--surface", "photo_screen"), "viewfinder"),
            (("--ab-pairs", "0"), SCENARIO),
            (("--ab-pairs", "три"), SCENARIO),
            (("--ab-pairs", "3"), "viewfinder"),
        ):
            with self.subTest(arguments=arguments, scenario=scenario):
                completed = self.run_smoke(scenario, [], extra_arguments=arguments)
                self.assertEqual(completed.returncode, 64, completed.stderr)

    def test_unchanged_frame_flag_reaches_the_intent_and_wrong_values_are_usage_errors(self) -> None:
        for value in ("skip", "always"):
            with self.subTest(value=value):
                completed = self.run_smoke(SCENARIO, [result_line(self.OK)], extra_arguments=("--unchanged-frame", value))
                self.assertEqual(completed.returncode, 0, completed.stderr)
                self.assertTrue(any(f"-e unchanged_frame {value}" in call for call in self.adb_calls()))
        for value in ("sometimes", "skip;ls"):
            with self.subTest(wrong=value):
                completed = self.run_smoke(SCENARIO, [], extra_arguments=("--unchanged-frame", value))
                self.assertEqual(completed.returncode, 64, completed.stderr)

    def test_without_the_unchanged_frame_flag_no_extra_is_sent(self) -> None:
        # Без флага extra не уходит: Rust берёт умолчание конфига.
        completed = self.run_smoke(SCENARIO, [result_line(self.OK)])
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertNotIn("unchanged_frame", self.start_call())

    def test_stop_app_after_force_stops_only_our_package_after_the_result(self) -> None:
        completed = self.run_smoke(SCENARIO, [result_line(self.OK)], extra_arguments=("--stop-app-after",))
        self.assertEqual(completed.returncode, 0, completed.stderr)
        calls = self.adb_calls()
        ours = f"am force-stop {PACKAGE}"
        self.assertEqual(sum(ours in call for call in calls), 1, calls)
        self.assertGreater(self.index_of(calls, ours), self.index_of(calls, "logcat"), calls)

    def test_without_stop_app_after_our_package_is_not_stopped(self) -> None:
        completed = self.run_smoke(SCENARIO, [result_line(self.OK)])
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertFalse(any(f"am force-stop {PACKAGE}" in call for call in self.adb_calls()))

    def test_stop_app_after_also_runs_after_a_failed_scenario_and_keeps_its_exit_code(self) -> None:
        failed = {"scenario": SCENARIO, "status": "failed", "error": "окно отвергнуто", "report": {}}
        completed = self.run_smoke(SCENARIO, [result_line(failed)], extra_arguments=("--stop-app-after",))
        self.assertEqual(completed.returncode, 1, completed.stderr)
        self.assertTrue(any(f"am force-stop {PACKAGE}" in call for call in self.adb_calls()))

    def test_help_text_documents_the_new_flags(self) -> None:
        completed = subprocess.run(
            ["bash", str(SCRIPTS_DIR / "device-smoke.sh"), "--help"], capture_output=True, text=True, timeout=30
        )
        for flag in ("--surface", "--ab-pairs", "--unchanged-frame", "--stop-app-after", "display_120hz", "calibrate_screen"):
            self.assertIn(flag, completed.stderr)


if __name__ == "__main__":
    unittest.main()


class GestureWaitsForBackgroundHoldTest(unittest.TestCase):
    """Жест не стреляет поверх ещё держащегося пальца: сначала драйвер дожидается завершения нажатия."""

    def make(self) -> tuple[smoke_driver.AdbDeviceActions, SizedAdb, FakeClock, list[FakeProcess]]:
        clock = FakeClock()
        adb = SizedAdb(1440, 3216)
        processes: list[FakeProcess] = []

        def spawn(command: list[str]) -> FakeProcess:
            process = FakeProcess(clock, finish_at=clock.now + 60.0)
            processes.append(process)
            return process

        actions = smoke_driver.AdbDeviceActions(
            "FAKE123", PACKAGE, run_adb=adb, spawn_adb=spawn, clock=clock.monotonic
        )
        return actions, adb, clock, processes

    def test_a_swipe_after_a_background_hold_waits_for_the_hold_to_end(self) -> None:
        actions, adb, clock, _ = self.make()
        driver = smoke_driver.StageDriver(SCENARIO, actions)
        driver.handle_stage(hold_stage(0, True, 60_000))
        self.assertEqual(clock.now, 0.0)
        driver.handle_stage(swipe_stage("modes_swipe_right"))
        self.assertEqual(clock.now, 60.0, "свайп выхода дождался конца удержания")
        swipes = [command for command in adb.commands if "swipe" in command and command[-1] == "250"]
        self.assertEqual(len(swipes), 1, "свайп выполнен один раз, после ожидания")

    def test_a_new_hold_waits_for_a_previous_one_that_is_still_alive(self) -> None:
        actions, _, clock, processes = self.make()
        driver = smoke_driver.StageDriver(SCENARIO, actions)
        driver.handle_stage(hold_stage(0, True, 60_000))
        driver.handle_stage(hold_stage(3, True, 60_000))
        self.assertEqual(len(processes), 2)
        self.assertEqual(clock.now, 60.0, "второе нажатие запущено после конца первого")
