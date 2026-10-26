"""Тесты драйвера шагов: разбор диалога, стадия → команда adb, обработка стадий в потоке logcat."""

from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_driver  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import smoke_ui  # noqa: E402
import smoke_wait  # noqa: E402

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "smoke"
SERIAL = "FAKE123"
PACKAGE = "dev.fasticamera.debug"
ADB = ["adb", "-s", SERIAL]
DUMP_COMMAND = [*ADB, "exec-out", "uiautomator", "dump", "/dev/tty"]


def fixture(name: str) -> str:
    return (FIXTURES / name).read_text(encoding="utf-8")


def stage_line(scenario: str, stage: str, **extra: object) -> str:
    payload = {"scenario": scenario, "stage": stage, **extra}
    return f"  1792957061.123  4242  4242 I Fasticamera: FASTICAMERA_STAGE {json.dumps(payload)}\n"


def result_line(payload: dict) -> str:
    return f"  1792957062.000  4242  4242 I Fasticamera: FASTICAMERA_RESULT {json.dumps(payload)}\n"


class FindDenyButtonTest(unittest.TestCase):
    def test_aosp_permission_controller_button_center(self) -> None:
        self.assertEqual(smoke_ui.find_deny_button(fixture("dialog_aosp.xml")), smoke_ui.TapPoint(540, 1560))

    def test_google_permission_controller_dont_ask_again_button(self) -> None:
        point = smoke_ui.find_deny_button(fixture("dialog_google_dont_ask_again.xml"))
        self.assertEqual(point, smoke_ui.TapPoint(540, 1560))

    def test_allow_button_is_never_taken_for_deny(self) -> None:
        self.assertIsNone(smoke_ui.find_deny_button(fixture("no_button.xml")))

    def test_dump_tail_after_hierarchy_is_ignored(self) -> None:
        point = smoke_ui.find_deny_button(fixture("with_dump_tail.xml"))
        self.assertEqual(point, smoke_ui.TapPoint(540, 1560))

    def test_truncated_dump_raises_parse_error(self) -> None:
        with self.assertRaises(smoke_ui.UiDumpParseError):
            smoke_ui.find_deny_button(fixture("broken.xml"))

    def test_garbage_bounds_raise_parse_error(self) -> None:
        with self.assertRaises(smoke_ui.UiDumpParseError):
            smoke_ui.find_deny_button(fixture("bad_bounds.xml"))

    def test_uiautomator_error_text_raises_parse_error(self) -> None:
        with self.assertRaises(smoke_ui.UiDumpParseError):
            smoke_ui.find_deny_button("ERROR: null root node returned by UiTestAutomationBridge.")

    def test_bounds_center_rounds_down(self) -> None:
        self.assertEqual(smoke_ui.parse_bounds_center("[0,0][101,51]"), smoke_ui.TapPoint(50, 25))


class FakeAdb:
    """Фейковый adb: записывает команды, на dump отдаёт заготовленные ответы по очереди (последний повторяется).

    Питание моделируется по факту OnePlus 11 / OxygenOS: ``KEYCODE_POWER`` переключает Awake <-> Dozing,
    ``KEYCODE_WAKEUP`` включает, ``KEYCODE_SLEEP`` игнорируется; ``dumpsys power`` печатает ``mWakefulness``.
    """

    def __init__(
        self,
        dump_outputs: list[str] | None = None,
        failing_prefix: list[str] | None = None,
        wakefulness: str = "Awake",
        power_key_works: bool = True,
    ) -> None:
        self.commands: list[list[str]] = []
        self._dump_outputs = list(dump_outputs or [])
        self._failing_prefix = failing_prefix
        self.wakefulness = wakefulness
        self._power_key_works = power_key_works

    def keycodes(self) -> list[str]:
        """Все нажатые ``input keyevent`` по порядку."""
        return [command[-1] for command in self.commands if "keyevent" in command]

    def __call__(self, command: list[str]) -> smoke_driver.AdbResult:
        self.commands.append(command)
        if self._failing_prefix is not None and command[: len(self._failing_prefix)] == self._failing_prefix:
            return smoke_driver.AdbResult(1, "", "device offline")
        if command == DUMP_COMMAND:
            output = self._dump_outputs.pop(0) if len(self._dump_outputs) > 1 else (self._dump_outputs or [""])[0]
            return smoke_driver.AdbResult(0, output, "")
        if command == [*ADB, "shell", "dumpsys", "power"]:
            return smoke_driver.AdbResult(0, f"Power Manager State:\n  mWakefulness={self.wakefulness}\n", "")
        if "keyevent" in command and self._power_key_works:
            if command[-1] == "KEYCODE_POWER":
                self.wakefulness = "Dozing" if self.wakefulness == "Awake" else "Awake"
            elif command[-1] == "KEYCODE_WAKEUP":
                self.wakefulness = "Awake"
        return smoke_driver.AdbResult(0, "", "")


class FakeClock:
    """Время без реального ожидания: ``sleep`` двигает часы."""

    def __init__(self) -> None:
        self.now = 0.0
        self.sleeps: list[float] = []

    def monotonic(self) -> float:
        return self.now

    def sleep(self, seconds: float) -> None:
        self.sleeps.append(seconds)
        self.now += seconds


def make_actions(adb: FakeAdb, clock: FakeClock | None = None) -> smoke_driver.AdbDeviceActions:
    clock = clock or FakeClock()
    return smoke_driver.AdbDeviceActions(SERIAL, PACKAGE, run_adb=adb, sleep=clock.sleep, clock=clock.monotonic)


class AdbDeviceActionsTest(unittest.TestCase):
    def test_press_home_command(self) -> None:
        adb = FakeAdb()
        make_actions(adb).press_home()
        self.assertEqual(adb.commands, [[*ADB, "shell", "input", "keyevent", "KEYCODE_HOME"]])

    def test_return_to_app_uses_launcher_intent_without_restart_flag(self) -> None:
        adb = FakeAdb()
        make_actions(adb).return_to_app()
        self.assertEqual(
            adb.commands,
            [[*ADB, "shell", "am", "start", "-W", "-a", "android.intent.action.MAIN",
              "-c", "android.intent.category.LAUNCHER", "-n", f"{PACKAGE}/dev.fasticamera.MainActivity"]],
        )
        self.assertNotIn("-S", adb.commands[0])

    def test_adb_failure_becomes_driver_error(self) -> None:
        adb = FakeAdb(failing_prefix=[*ADB, "shell", "input", "keyevent"])
        with self.assertRaisesRegex(smoke_driver.DriverError, "device offline"):
            make_actions(adb).press_home()

    def test_am_start_error_text_with_zero_exit_code_is_failure(self) -> None:
        def run_adb(command: list[str]) -> smoke_driver.AdbResult:
            return smoke_driver.AdbResult(0, "Error: Activity class does not exist.\n", "")

        actions = smoke_driver.AdbDeviceActions(SERIAL, PACKAGE, run_adb=run_adb)
        with self.assertRaisesRegex(smoke_driver.DriverError, "Activity class"):
            actions.return_to_app()

    def test_deny_dialog_taps_center_of_button(self) -> None:
        adb = FakeAdb([fixture("dialog_aosp.xml")])
        make_actions(adb).deny_permission_dialog()
        self.assertEqual(adb.commands, [DUMP_COMMAND, [*ADB, "shell", "input", "tap", "540", "1560"]])

    def test_deny_dialog_waits_until_dialog_appears(self) -> None:
        adb = FakeAdb([fixture("no_button.xml"), fixture("broken.xml"), fixture("dialog_google_dont_ask_again.xml")])
        clock = FakeClock()
        make_actions(adb, clock).deny_permission_dialog()
        self.assertEqual(adb.commands.count(DUMP_COMMAND), 3)
        self.assertEqual(adb.commands[-1], [*ADB, "shell", "input", "tap", "540", "1560"])
        self.assertEqual(clock.sleeps, [smoke_driver.DIALOG_POLL_INTERVAL_SECONDS] * 2)

    def test_deny_dialog_times_out_with_hint_and_no_tap(self) -> None:
        adb = FakeAdb([fixture("no_button.xml")])
        clock = FakeClock()
        with self.assertRaises(smoke_driver.DriverError) as raised:
            make_actions(adb, clock).deny_permission_dialog()
        self.assertIn("экран заблокирован", str(raised.exception))
        self.assertIn("кнопки отказа нет", str(raised.exception))
        self.assertGreaterEqual(clock.now, smoke_driver.DIALOG_TIMEOUT_SECONDS)
        self.assertTrue(all(command == DUMP_COMMAND for command in adb.commands))

    def test_deny_dialog_timeout_reports_broken_dump(self) -> None:
        adb = FakeAdb([fixture("broken.xml")])
        with self.assertRaisesRegex(smoke_driver.DriverError, "дамп не XML"):
            make_actions(adb).deny_permission_dialog()


class RecordingActions:
    """Фейковые действия: порядок вызовов."""

    def __init__(self) -> None:
        self.calls: list[str] = []

    def press_home(self) -> None:
        self.calls.append("press_home")

    def pause(self, seconds: float) -> None:
        self.calls.append(f"pause({seconds:g})")

    def return_to_app(self) -> None:
        self.calls.append("return_to_app")

    def deny_permission_dialog(self) -> None:
        self.calls.append("deny_permission_dialog")


class StageDriverTest(unittest.TestCase):
    def drive(self, scenario: str, *stage_events: dict) -> list[str]:
        actions = RecordingActions()
        driver = smoke_driver.StageDriver(scenario, actions)
        for stage_event in stage_events:
            driver.handle_stage(stage_event)
        return actions.calls

    def test_lifecycle_ready_presses_home(self) -> None:
        self.assertEqual(self.drive("lifecycle", {"scenario": "lifecycle", "stage": "ready"}), ["press_home"])

    def test_lifecycle_backgrounded_pauses_then_returns(self) -> None:
        calls = self.drive("lifecycle", {"scenario": "lifecycle", "stage": "backgrounded"})
        self.assertEqual(calls, [f"pause({smoke_driver.BACKGROUND_PAUSE_SECONDS:g})", "return_to_app"])

    def test_permission_dialog_expected_denies(self) -> None:
        calls = self.drive("permission", {"scenario": "permission", "stage": "dialog_expected", "attempt": 1})
        self.assertEqual(calls, ["deny_permission_dialog"])

    def test_no_dialog_expected_does_nothing(self) -> None:
        calls = self.drive("permission", {"scenario": "permission", "stage": "no_dialog_expected", "attempt": 3})
        self.assertEqual(calls, [])

    def test_stage_of_other_scenario_is_ignored(self) -> None:
        self.assertEqual(self.drive("lifecycle", {"scenario": "permission", "stage": "dialog_expected"}), [])

    def test_scenario_without_steps_ignores_stages(self) -> None:
        self.assertEqual(self.drive("motion", {"scenario": "motion", "stage": "ready"}), [])


class EvaluateStreamWithStagesTest(unittest.TestCase):
    """Сквозное: строки logcat → стадии → команды adb → вердикт по результату."""

    LIFECYCLE_RESULT = {
        "scenario": "lifecycle",
        "status": "passed",
        "report": {
            "events": ["paused", "surface_destroyed", "surface_created", "surface_changed", "resumed"],
            "live_global_refs_at_ready": 5,
            "live_global_refs_at_end": 5,
            "surface_release_timeouts": 0,
            "activity_recreated": False,
            "touch_samples_dropped": 0,
        },
    }

    def run_stream(self, scenario: str, lines: list[str], adb: FakeAdb) -> smoke_wait.SmokeVerdict:
        driver = smoke_driver.StageDriver(scenario, make_actions(adb))
        return smoke_wait.evaluate_stream(lines, scenario, driver.handle_stage)

    def test_lifecycle_stages_run_commands_in_order_and_result_is_judged(self) -> None:
        adb = FakeAdb()
        lines = [
            stage_line("lifecycle", "ready"),
            stage_line("lifecycle", "backgrounded"),
            result_line(self.LIFECYCLE_RESULT),
        ]
        verdict = self.run_stream("lifecycle", lines, adb)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        self.assertEqual(len(adb.commands), 2)
        self.assertEqual(adb.commands[0], [*ADB, "shell", "input", "keyevent", "KEYCODE_HOME"])
        self.assertEqual(adb.commands[1][:5], [*ADB, "shell", "am"])
        self.assertIn(f"{PACKAGE}/dev.fasticamera.MainActivity", adb.commands[1])

    def test_result_right_after_slow_stage_is_not_lost(self) -> None:
        # Результат идёт сразу за стадией, на которой драйвер занят опросом диалога:
        # строка не должна пропасть (дефект 2 сессии 01).
        adb = FakeAdb([fixture("no_button.xml"), fixture("dialog_aosp.xml")])
        lines = [
            stage_line("permission", "dialog_expected", attempt=1),
            result_line({"scenario": "permission", "status": "passed",
                         "report": {"answers": ["denied", "denied_permanently", "denied_permanently"]}}),
        ]
        verdict = self.run_stream("permission", lines, adb)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        self.assertEqual(adb.commands[-1][-3:], ["tap", "540", "1560"])

    def test_stage_of_other_scenario_runs_no_commands(self) -> None:
        adb = FakeAdb()
        lines = [stage_line("permission", "dialog_expected", attempt=1), result_line(self.LIFECYCLE_RESULT)]
        verdict = self.run_stream("lifecycle", lines, adb)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        self.assertEqual(adb.commands, [])

    def test_driver_failure_fails_scenario_with_stage_in_message(self) -> None:
        adb = FakeAdb(failing_prefix=[*ADB, "shell", "input", "keyevent"])
        lines = [stage_line("lifecycle", "ready"), result_line(self.LIFECYCLE_RESULT)]
        verdict = self.run_stream("lifecycle", lines, adb)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("'ready'", verdict.message)
        self.assertIn("device offline", verdict.message)

    def test_dialog_never_appears_fails_with_hint(self) -> None:
        adb = FakeAdb([fixture("no_button.xml")])
        verdict = self.run_stream("permission", [stage_line("permission", "dialog_expected", attempt=1)], adb)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("кнопка отказа", verdict.message)

    def test_broken_stage_json_fails(self) -> None:
        verdict = smoke_wait.evaluate_stream(["I Fasticamera: FASTICAMERA_STAGE {oops\n"], "lifecycle", lambda _event: None)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("стадии не JSON", verdict.message)

    def test_stream_without_handler_still_skips_stage_lines(self) -> None:
        lines = [stage_line("lifecycle", "ready"), result_line(self.LIFECYCLE_RESULT)]
        verdict = smoke_wait.evaluate_stream(lines, "lifecycle")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)


if __name__ == "__main__":
    unittest.main()
