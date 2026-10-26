"""Тесты smoke-проверки: вердикт по строкам logcat и поведение device-smoke.sh без телефона."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_wait  # noqa: E402  (путь к scripts/ добавлен строкой выше)

PANIC_LOCATION = "crates/fasticamera-app/src/scenario/intentional_panic.rs:10:5"


def result_line(payload: dict) -> str:
    """Строка logcat в формате `-v epoch`, как её печатает телефон."""
    return f"  1792957061.123  4242  4242 I Fasticamera: FASTICAMERA_RESULT {json.dumps(payload)}\n"


def hello_payload(**report_overrides: object) -> dict:
    report = {
        "build": {"crate_version": "0.1.0", "git_revision": "8459f12abcd0", "abi": "arm64-v8a", "debug_assertions": True},
        "abi": "arm64-v8a",
        "build_type": "debug",
        "log_filter": "debug",
        "log_write_failures": 0,
    }
    report.update(report_overrides)
    return {"scenario": "hello", "status": "passed", "report": report}


def panic_payload(location: str = PANIC_LOCATION) -> dict:
    return {
        "scenario": "panic",
        "status": "panicked",
        "panic": {"message": "намеренная паника", "location": location, "thread": "main"},
    }


PANIC_LOG_LINE = f"  1792957061.100  4242  4242 F Fasticamera: panic at {PANIC_LOCATION} (thread main): намеренная паника\n"


class EvaluateStreamTest(unittest.TestCase):
    def test_hello_passes(self) -> None:
        verdict = smoke_wait.evaluate_stream([result_line(hello_payload())], "hello")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)

    def test_hello_with_wrong_abi_fails(self) -> None:
        verdict = smoke_wait.evaluate_stream([result_line(hello_payload(abi="x86_64"))], "hello")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("abi='x86_64'", verdict.message)

    def test_hello_without_git_revision_fails(self) -> None:
        payload = hello_payload()
        payload["report"]["build"]["git_revision"] = None
        verdict = smoke_wait.evaluate_stream([result_line(payload)], "hello")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("ревизии git", verdict.message)

    def test_failed_status_is_reported_with_error(self) -> None:
        payload = {"scenario": "hello", "status": "failed", "error": "нативная часть не инициализирована"}
        verdict = smoke_wait.evaluate_stream([result_line(payload)], "hello")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("не инициализирована", verdict.message)

    def test_result_of_other_scenario_is_skipped(self) -> None:
        lines = [result_line(panic_payload()), result_line(hello_payload())]
        verdict = smoke_wait.evaluate_stream(lines, "hello")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)

    def test_no_result_line_means_no_result(self) -> None:
        lines = ["  1792957061.000  4242  4242 I Fasticamera: fasticamera::runtime: нативная часть инициализирована\n"]
        verdict = smoke_wait.evaluate_stream(lines, "hello")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_NO_RESULT)

    def test_broken_json_fails(self) -> None:
        verdict = smoke_wait.evaluate_stream(["I Fasticamera: FASTICAMERA_RESULT {oops\n"], "hello")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("не JSON", verdict.message)

    def test_process_crash_fails_before_result(self) -> None:
        lines = [
            "  1792957061.000  4242  4242 E AndroidRuntime: FATAL EXCEPTION: main\n",
            result_line(hello_payload()),
        ]
        verdict = smoke_wait.evaluate_stream(lines, "hello")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("упал", verdict.message)

    def test_crash_of_our_package_fails_and_crash_of_another_app_is_ignored(self) -> None:
        package = "dev.example.fasticamera"
        other_crash = [
            "  1792957061.000  900  900 E AndroidRuntime: FATAL EXCEPTION: main\n",
            "  1792957061.001  900  900 E AndroidRuntime: Process: com.other.app, PID: 900\n",
            "  1792957061.002  900  900 E AndroidRuntime: java.lang.IllegalStateException: boom\n",
        ]
        verdict = smoke_wait.evaluate_stream(
            [*other_crash, result_line(hello_payload())], "hello", package=package
        )
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, "чужое падение не ломает наш прогон")

        our_crash = [
            "  1792957061.000  4242  4242 E AndroidRuntime: FATAL EXCEPTION: main\n",
            f"  1792957061.001  4242  4242 E AndroidRuntime: Process: {package}, PID: 4242\n",
        ]
        verdict = smoke_wait.evaluate_stream([*our_crash, result_line(hello_payload())], "hello", package=package)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("упал", verdict.message)

        # Дополнительный процесс приложения (пакет:суффикс) тоже наш; похожее имя — нет.
        for process, ours in ((f"{package}:remote", True), (f"{package}x", False)):
            lines = [
                "E AndroidRuntime: FATAL EXCEPTION: Thread-2\n",
                f"E AndroidRuntime: Process: {process}, PID: 7\n",
                result_line(hello_payload()),
            ]
            exit_code = smoke_wait.evaluate_stream(lines, "hello", package=package).exit_code
            self.assertEqual(exit_code, smoke_wait.EXIT_SCENARIO_FAILED if ours else smoke_wait.EXIT_OK, process)

    def test_crash_without_process_line_is_not_attributed_to_us(self) -> None:
        lines = [
            "E AndroidRuntime: FATAL EXCEPTION IN SYSTEM PROCESS: android.ui\n",
            "E AndroidRuntime: java.lang.RuntimeException: x\n",
            "E AndroidRuntime: at a.b.C.d(C.java:1)\n",
            "E AndroidRuntime: at a.b.C.e(C.java:2)\n",
            result_line(hello_payload()),
        ]
        verdict = smoke_wait.evaluate_stream(lines, "hello", package="dev.example.fasticamera")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)

    def test_panic_passes_when_hook_logged_same_location(self) -> None:
        verdict = smoke_wait.evaluate_stream([PANIC_LOG_LINE, result_line(panic_payload())], "panic")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)

    def test_panic_without_hook_log_fails(self) -> None:
        verdict = smoke_wait.evaluate_stream([result_line(panic_payload())], "panic")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("panic hook", verdict.message)

    def test_panic_in_wrong_place_fails(self) -> None:
        location = "crates/fasticamera-app/src/runtime.rs:1:1"
        log_line = f"F Fasticamera: panic at {location} (thread main): x\n"
        verdict = smoke_wait.evaluate_stream([log_line, result_line(panic_payload(location))], "panic")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("intentional_panic.rs", verdict.message)

    def test_panic_scenario_in_release_fails(self) -> None:
        payload = {"scenario": "panic", "status": "failed", "error": "сценарий \"panic\" есть только в отладочной сборке"}
        verdict = smoke_wait.evaluate_stream([result_line(payload)], "panic")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("отладочной", verdict.message)

    def test_unknown_scenario_is_usage_error(self) -> None:
        verdict = smoke_wait.evaluate_stream([], "nope")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_USAGE)


class SinceEpochArgumentTest(unittest.TestCase):
    def test_seconds_with_milliseconds_are_accepted(self) -> None:
        self.assertEqual(smoke_wait.parse_since_epoch("1792999217.756"), "1792999217.756")

    def test_whole_seconds_are_rejected(self) -> None:
        with self.assertRaises(smoke_wait.argparse.ArgumentTypeError):
            smoke_wait.parse_since_epoch("1792999217")

    def test_other_precisions_are_rejected(self) -> None:
        for text in ("1792999217.75", "1792999217.756579809", "", "abc.123"):
            with self.subTest(text=text), self.assertRaises(smoke_wait.argparse.ArgumentTypeError):
                smoke_wait.parse_since_epoch(text)


class LineReaderTest(unittest.TestCase):
    """Чтение живого потока: как adb logcat, процесс пишет пачку строк и не закрывает вывод."""

    def start_writer(self, *lines: str) -> subprocess.Popen[str]:
        program = (
            "import sys, time\n"
            f"sys.stdout.write({''.join(lines)!r})\n"
            "sys.stdout.flush()\n"
            "time.sleep(30)\n"
        )
        process = subprocess.Popen([sys.executable, "-c", program], stdout=subprocess.PIPE, text=True)
        self.addCleanup(process.wait)
        self.addCleanup(process.kill)
        return process

    def test_all_buffered_lines_arrive_before_deadline(self) -> None:
        # Регрессия сессии 01: ожидание через select видело только первую строку пачки,
        # остальные оставались в буфере Python, и проверка ждала до таймаута.
        process = self.start_writer("--------- beginning of main\n", "I Fasticamera: x\n", result_line(hello_payload()))
        started = time.monotonic()
        verdict = smoke_wait.evaluate_stream(smoke_wait._lines_until_deadline(process, started + 10), "hello")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        self.assertLess(time.monotonic() - started, 5)

    def test_deadline_stops_waiting_on_silent_stream(self) -> None:
        process = self.start_writer("I Fasticamera: без результата\n")
        started = time.monotonic()
        lines = list(smoke_wait._lines_until_deadline(process, started + 0.5))
        self.assertEqual(lines, ["I Fasticamera: без результата\n"])
        self.assertLess(time.monotonic() - started, 5)


class DeviceSmokeScriptTest(unittest.TestCase):
    """Запуск настоящего device-smoke.sh с подменённым adb — без телефона."""

    def setUp(self) -> None:
        temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(temp_dir.cleanup)
        self.fake_bin = Path(temp_dir.name)

    def write_fake_adb(self, devices_output: str) -> None:
        fake_adb = self.fake_bin / "adb"
        fake_adb.write_text(
            "#!/usr/bin/env bash\n"
            'if [[ "$1" == "devices" ]]; then\n'
            f"  printf '%b' {json.dumps(devices_output)}\n"
            "  exit 0\n"
            "fi\n"
            "echo \"фейковый adb: неожиданный вызов $*\" >&2\n"
            "exit 99\n",
            encoding="utf-8",
        )
        fake_adb.chmod(0o755)

    def run_smoke(self, *arguments: str) -> subprocess.CompletedProcess[str]:
        environment = {key: value for key, value in os.environ.items() if key != "ANDROID_SERIAL"}
        environment["PATH"] = f"{self.fake_bin}{os.pathsep}{environment['PATH']}"
        return subprocess.run(
            ["bash", str(SCRIPTS_DIR / "device-smoke.sh"), *arguments],
            capture_output=True,
            text=True,
            env=environment,
            timeout=30,
        )

    def test_no_device_is_reported_with_distinct_exit_code(self) -> None:
        self.write_fake_adb("List of devices attached\n\n")
        completed = self.run_smoke("--no-build", "hello")
        self.assertEqual(completed.returncode, 2, completed.stderr)
        self.assertIn("устройство не найдено", completed.stderr)

    def test_other_phone_model_is_not_used(self) -> None:
        self.write_fake_adb("List of devices attached\nemulator-5554 device product:sdk model:Pixel_9 device:emu\n")
        completed = self.run_smoke("--no-build", "hello")
        self.assertEqual(completed.returncode, 2, completed.stderr)

    def test_two_project_phones_require_explicit_serial(self) -> None:
        self.write_fake_adb(
            "List of devices attached\n"
            "aaa device product:CPH2447 model:CPH2447 device:OP594DL1\n"
            "bbb device product:CPH2447 model:CPH2447 device:OP594DL1\n"
        )
        completed = self.run_smoke("--no-build", "hello")
        self.assertEqual(completed.returncode, 2, completed.stderr)
        self.assertIn("ANDROID_SERIAL", completed.stderr)

    def test_missing_scenario_is_usage_error(self) -> None:
        self.write_fake_adb("List of devices attached\n")
        completed = self.run_smoke("--no-build")
        self.assertEqual(completed.returncode, 64, completed.stderr)

    def test_unknown_variant_is_usage_error(self) -> None:
        self.write_fake_adb("List of devices attached\n")
        completed = self.run_smoke("--variant", "beta", "hello")
        self.assertEqual(completed.returncode, 64, completed.stderr)


if __name__ == "__main__":
    unittest.main()
