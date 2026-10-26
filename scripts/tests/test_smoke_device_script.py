"""Сквозные тесты device-smoke.sh новых сценариев с фейковым adb (телефон не нужен).

Фейковый adb пишет каждый вызов в журнал, на ``logcat`` отдаёт заготовленные строки, на
``uiautomator dump`` — заготовленный XML. Проверяем и результат (код выхода), и то, какие
команды ушли на «телефон» и в каком порядке.
"""

from __future__ import annotations

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
FIXTURES = Path(__file__).resolve().parent / "fixtures" / "smoke"
SERIAL = "FAKE123"
PACKAGE = "dev.fasticamera.debug"

FAKE_ADB = r"""#!/usr/bin/env bash
printf '%s\n' "$*" >> "$FAKE_ADB_LOG"
if [[ "${1:-}" == "-s" ]]; then shift 2; fi
case "${1:-}" in
    get-state) echo device ;;
    install) echo Success ;;
    logcat) cat "$FAKE_LOGCAT_FILE" ;;
    exec-out) cat "$FAKE_DUMP_FILE" ;;
    shell)
        case "${2:-}" in
            date) echo 1000.123456789 ;;
            pm)
                if [[ "${3:-}" == "$FAKE_FAILING_PM_COMMAND" ]]; then echo "pm: сбой" >&2; exit 1; fi ;;
        esac ;;
esac
exit 0
"""


def stage_line(scenario: str, stage: str, **extra: object) -> str:
    return f"1792957061.123  4242  4242 I Fasticamera: FASTICAMERA_STAGE {json.dumps({'scenario': scenario, 'stage': stage, **extra})}\n"


def result_line(payload: dict) -> str:
    return f"1792957062.000  4242  4242 I Fasticamera: FASTICAMERA_RESULT {json.dumps(payload)}\n"


PERMISSION_OK = {
    "scenario": "permission",
    "status": "passed",
    "report": {"answers": ["denied", "denied_permanently", "denied_permanently"]},
}
PERMISSION_STAGES = [
    stage_line("permission", "dialog_expected", attempt=1),
    stage_line("permission", "dialog_expected", attempt=2),
    stage_line("permission", "no_dialog_expected", attempt=3),
]
LIFECYCLE_OK = {
    "scenario": "lifecycle",
    "status": "passed",
    "report": {
        "events": ["paused", "surface_destroyed", "surface_created", "surface_changed", "resumed"],
        "live_global_refs_at_ready": 7,
        "live_global_refs_at_end": 7,
        "surface_release_timeouts": 0,
        "activity_recreated": False,
        "touch_samples_dropped": 0,
    },
}


class DeviceSmokeScenariosTest(unittest.TestCase):
    def setUp(self) -> None:
        temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(temp_dir.cleanup)
        self.work_dir = Path(temp_dir.name)
        fake_bin = self.work_dir / "bin"
        fake_bin.mkdir()
        fake_adb = fake_bin / "adb"
        fake_adb.write_text(FAKE_ADB, encoding="utf-8")
        fake_adb.chmod(0o755)
        self.fake_bin = fake_bin
        self.adb_log = self.work_dir / "adb.log"
        self.adb_log.write_text("", encoding="utf-8")
        self.apk = self.work_dir / "app.apk"
        self.apk.write_bytes(b"")
        self.logcat_file = self.work_dir / "logcat.txt"
        self.dump_file = self.work_dir / "dump.xml"
        self.dump_file.write_text((FIXTURES / "dialog_aosp.xml").read_text(encoding="utf-8"), encoding="utf-8")

    def run_smoke(self, scenario: str, logcat_lines: list[str], failing_pm_command: str = "") -> subprocess.CompletedProcess[str]:
        self.logcat_file.write_text("".join(logcat_lines), encoding="utf-8")
        environment = dict(os.environ)
        environment.update(
            PATH=f"{self.fake_bin}{os.pathsep}{environment['PATH']}",
            ANDROID_SERIAL=SERIAL,
            FASTICAMERA_APK_PATH=str(self.apk),
            FAKE_ADB_LOG=str(self.adb_log),
            FAKE_LOGCAT_FILE=str(self.logcat_file),
            FAKE_DUMP_FILE=str(self.dump_file),
            FAKE_FAILING_PM_COMMAND=failing_pm_command,
        )
        return subprocess.run(
            ["bash", str(SCRIPTS_DIR / "device-smoke.sh"), "--no-build", "--timeout", "20", scenario],
            capture_output=True, text=True, env=environment, timeout=60,
        )

    def adb_calls(self) -> list[str]:
        """Журнал вызовов adb без префикса ``-s <serial>``; заодно проверяет, что serial был везде."""
        calls = []
        for line in self.adb_log.read_text(encoding="utf-8").splitlines():
            self.assertTrue(line.startswith(f"-s {SERIAL} "), f"вызов adb без явного serial: {line}")
            calls.append(line[len(f"-s {SERIAL} "):])
        return calls

    def index_of(self, calls: list[str], fragment: str) -> int:
        for position, call in enumerate(calls):
            if fragment in call:
                return position
        self.fail(f"в журнале adb нет вызова с {fragment!r}: {calls}")

    # --- точка отсчёта logcat ---

    def test_logcat_starts_from_device_time_with_milliseconds(self) -> None:
        """С целыми секундами прогон, начатый в ту же секунду, что и конец прошлого, читал его
        строку результата (дефект сессии 03): отсчёт обязан быть с миллисекундами."""
        completed = self.run_smoke("lifecycle", [result_line(LIFECYCLE_OK)])
        self.assertEqual(completed.returncode, 0, completed.stderr)
        logcat = self.adb_calls()[self.index_of(self.adb_calls(), "logcat -v epoch")]
        self.assertIn("-T 1000.123 ", logcat)

    # --- permission ---

    def test_permission_resets_before_start_and_restores_after_success(self) -> None:
        completed = self.run_smoke("permission", [*PERMISSION_STAGES, result_line(PERMISSION_OK)])
        self.assertEqual(completed.returncode, 0, completed.stderr)
        calls = self.adb_calls()
        revoke = self.index_of(calls, f"shell pm revoke {PACKAGE} android.permission.CAMERA")
        clear_before = self.index_of(calls, f"shell pm clear-permission-flags {PACKAGE} android.permission.CAMERA user-set user-fixed")
        launch = self.index_of(calls, "am start -S -W")
        taps = [i for i, call in enumerate(calls) if call == "shell input tap 540 1560"]
        grant = self.index_of(calls, f"shell pm grant {PACKAGE} android.permission.CAMERA")
        self.assertLess(revoke, launch)
        self.assertLess(clear_before, launch)
        self.assertEqual(len(taps), 2, calls)  # два диалога, третий запрос без диалога
        self.assertTrue(all(launch < tap < grant for tap in taps), calls)
        self.assertEqual(calls[-1], f"shell pm clear-permission-flags {PACKAGE} android.permission.CAMERA user-set user-fixed")

    def test_permission_is_restored_when_scenario_fails(self) -> None:
        failed = {"scenario": "permission", "status": "failed", "error": "запрос не ушёл"}
        completed = self.run_smoke("permission", [result_line(failed)])
        self.assertEqual(completed.returncode, 1, completed.stderr)
        self.assertIn(f"shell pm grant {PACKAGE} android.permission.CAMERA", self.adb_calls())

    def test_permission_is_restored_when_no_result_arrives(self) -> None:
        completed = self.run_smoke("permission", [])
        self.assertEqual(completed.returncode, 3, completed.stderr)
        self.assertIn(f"shell pm grant {PACKAGE} android.permission.CAMERA", self.adb_calls())

    def test_restore_failure_is_printed_but_does_not_mask_exit_code(self) -> None:
        failed = {"scenario": "permission", "status": "failed", "error": "запрос не ушёл"}
        completed = self.run_smoke("permission", [result_line(failed)], failing_pm_command="grant")
        self.assertEqual(completed.returncode, 1, completed.stderr)
        self.assertIn("не удалось вернуть разрешение камеры", completed.stderr)

    def test_restore_failure_does_not_turn_success_into_failure(self) -> None:
        completed = self.run_smoke("permission", [*PERMISSION_STAGES, result_line(PERMISSION_OK)], failing_pm_command="grant")
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn("не удалось вернуть разрешение камеры", completed.stderr)

    def test_failed_reset_stops_before_launch_and_still_restores(self) -> None:
        completed = self.run_smoke("permission", [result_line(PERMISSION_OK)], failing_pm_command="revoke")
        self.assertEqual(completed.returncode, 4, completed.stderr)
        calls = self.adb_calls()
        self.assertFalse(any("am start" in call for call in calls), calls)
        self.assertIn(f"shell pm grant {PACKAGE} android.permission.CAMERA", calls)

    def test_missing_dialog_fails_scenario_with_hint(self) -> None:
        self.dump_file.write_text((FIXTURES / "no_button.xml").read_text(encoding="utf-8"), encoding="utf-8")
        completed = self.run_smoke("permission", [stage_line("permission", "dialog_expected", attempt=1)])
        self.assertEqual(completed.returncode, 1, completed.stderr)
        self.assertIn("кнопка отказа", completed.stderr)
        self.assertIn(f"shell pm grant {PACKAGE} android.permission.CAMERA", self.adb_calls())

    # --- lifecycle ---

    def test_lifecycle_sends_home_then_returns_app_and_touches_no_permissions(self) -> None:
        lines = [stage_line("lifecycle", "ready"), stage_line("lifecycle", "backgrounded"), result_line(LIFECYCLE_OK)]
        completed = self.run_smoke("lifecycle", lines)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        calls = self.adb_calls()
        launch = self.index_of(calls, "am start -S -W")
        home = self.index_of(calls, "shell input keyevent KEYCODE_HOME")
        back = self.index_of(calls, "shell am start -W -a android.intent.action.MAIN -c android.intent.category.LAUNCHER "
                                    f"-n {PACKAGE}/dev.fasticamera.MainActivity")
        self.assertLess(launch, home)
        self.assertLess(home, back)
        self.assertFalse(any(" pm " in f" {call} " for call in calls), calls)

    def test_lifecycle_leak_makes_script_fail(self) -> None:
        leaking = json.loads(json.dumps(LIFECYCLE_OK))
        leaking["report"]["live_global_refs_at_end"] = 9
        completed = self.run_smoke("lifecycle", [stage_line("lifecycle", "ready"), result_line(leaking)])
        self.assertEqual(completed.returncode, 1, completed.stderr)
        self.assertIn("утечка глобальных JNI-ссылок", completed.stderr)

    # --- сценарии без действий ---

    def test_motion_runs_without_extra_adb_actions(self) -> None:
        motion = {"scenario": "motion", "status": "passed", "report": {
            "gyro": {"samples": 1200, "rate_hz": 401.7}, "orientation": "portrait", "accelerometer_samples": 300}}
        completed = self.run_smoke("motion", [result_line(motion)])
        self.assertEqual(completed.returncode, 0, completed.stderr)
        calls = self.adb_calls()
        self.assertFalse(any("input" in call or " pm " in f" {call} " for call in calls), calls)

    def test_unknown_scenario_is_usage_error(self) -> None:
        completed = self.run_smoke("nope", [])
        self.assertEqual(completed.returncode, 64, completed.stderr)


if __name__ == "__main__":
    unittest.main()
