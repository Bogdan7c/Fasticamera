"""Тесты smoke-сценариев ``raw-stream*``: шаг драйвера ``screencap``, таблица стадий, проверки по правилам,
сквозной вердикт ``evaluate_stream`` со скриншотами (телефон не нужен)."""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_checks  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import smoke_driver  # noqa: E402
import smoke_screen  # noqa: E402
import smoke_wait  # noqa: E402
from test_smoke_screen import HEIGHT, WIDTH, black, noisy, screencap_bytes  # noqa: E402

SERIAL = "FAKE123"
PACKAGE = "dev.fasticamera.debug"
SCREENCAP_COMMAND = ["adb", "-s", SERIAL, "exec-out", "screencap"]


def stage_line(scenario: str, stage: str, **extra: object) -> str:
    payload = {"scenario": scenario, "stage": stage, **extra}
    return f"  1792957061.123  4242  4242 I Fasticamera: FASTICAMERA_STAGE {json.dumps(payload)}\n"


def result_line(payload: dict) -> str:
    return f"  1792957062.000  4242  4242 I Fasticamera: FASTICAMERA_RESULT {json.dumps(payload)}\n"


class FakeScreen:
    """Фейковый ``adb exec-out screencap``: отдаёт заготовленные кадры по очереди (последний повторяется)."""

    def __init__(self, *frames: bytes, returncode: int = 0) -> None:
        self.frames = list(frames)
        self.commands: list[list[str]] = []
        self.returncode = returncode

    def __call__(self, command: list[str]) -> smoke_driver.AdbBytesResult:
        self.commands.append(command)
        if self.returncode != 0:
            return smoke_driver.AdbBytesResult(self.returncode, b"", "device offline")
        frame = self.frames.pop(0) if len(self.frames) > 1 else self.frames[0]
        return smoke_driver.AdbBytesResult(0, frame, "")


def frame(pixel_at, colorspace: bool = True) -> bytes:
    return screencap_bytes(pixel_at, header_with_colorspace=colorspace)


def actions_for(screen: FakeScreen, artifacts_dir: Path | None) -> smoke_driver.AdbDeviceActions:
    def unused_adb(command: list[str]) -> smoke_driver.AdbResult:
        raise AssertionError(f"обычный adb не должен вызываться: {command}")

    return smoke_driver.AdbDeviceActions(
        SERIAL, PACKAGE, run_adb=unused_adb, run_adb_bytes=screen, artifacts_dir=artifacts_dir, sleep=lambda _s: None
    )


class ScreencapStepTest(unittest.TestCase):
    def test_command_is_raw_exec_out_screencap_without_png_flag(self) -> None:
        screen = FakeScreen(frame(noisy(1)))
        with tempfile.TemporaryDirectory() as directory:
            actions_for(screen, Path(directory)).screencap("viewfinder_a")
        self.assertEqual(screen.commands, [SCREENCAP_COMMAND])
        self.assertNotIn("-p", screen.commands[0])

    def test_raw_output_and_png_are_saved_under_the_label(self) -> None:
        data = frame(noisy(1), colorspace=False)
        with tempfile.TemporaryDirectory() as directory:
            actions_for(FakeScreen(data), Path(directory)).screencap("viewfinder_a")
            self.assertEqual(smoke_screen.capture_path(Path(directory), "viewfinder_a").read_bytes(), data)
            png = (Path(directory) / "viewfinder_a.png").read_bytes()
        self.assertTrue(png.startswith(b"\x89PNG"))

    def test_missing_artifacts_dir_is_a_driver_error_without_adb_call(self) -> None:
        screen = FakeScreen(frame(noisy(1)))
        with self.assertRaisesRegex(smoke_driver.DriverError, "каталог артефактов не задан"):
            actions_for(screen, None).screencap("viewfinder_a")
        self.assertEqual(screen.commands, [])

    def test_adb_failure_becomes_a_driver_error(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(smoke_driver.DriverError, "screencap: adb завершился с кодом 1"):
                actions_for(FakeScreen(b"", returncode=1), Path(directory)).screencap("viewfinder_a")

    def test_garbage_output_fails_at_once_and_saves_nothing(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(smoke_driver.DriverError, "не сохранён"):
                actions_for(FakeScreen(b"\x00" * 40), Path(directory)).screencap("viewfinder_a")
            self.assertEqual(list(Path(directory).iterdir()), [])


class RecordingActions:
    def __init__(self) -> None:
        self.calls: list[str] = []

    def screencap(self, label: str) -> None:
        self.calls.append(f"screencap({label})")

    def pause(self, seconds: float) -> None:
        self.calls.append(f"pause({seconds:g})")


class StageTableTest(unittest.TestCase):
    def test_viewfinder_live_takes_two_shots_with_a_pause_between(self) -> None:
        for scenario in ("raw-stream", "raw-stream-endurance"):
            actions = RecordingActions()
            driver = smoke_driver.StageDriver(scenario, actions)  # type: ignore[arg-type]
            driver.handle_stage({"scenario": scenario, "stage": "viewfinder_live"})
            self.assertEqual(actions.calls, ["screencap(viewfinder_a)", "pause(1.5)", "screencap(viewfinder_b)"], scenario)

    def test_other_stages_and_the_probe_scenario_take_no_shots(self) -> None:
        actions = RecordingActions()
        driver = smoke_driver.StageDriver("raw-stream", actions)  # type: ignore[arg-type]
        for stage in ("started", "streaming", "progress", "detail_stream"):
            driver.handle_stage({"scenario": "raw-stream", "stage": stage})
        self.assertEqual(actions.calls, [])
        self.assertNotIn("raw-stream-cpu-probe", smoke_driver.SCENARIO_STEPS)


def healthy_report(**overrides: object) -> dict:
    report: dict = {
        "kind": "raw-stream",
        "requested": {"role": "wide", "raw_format": "raw10", "readout": "four_by_three", "frame_rate": 60},
        "configured": {"camera": "2", "frame_rate": 60},
        "window_ms": 60_000,
        "sensor_fps": 59.9,
        "target_fps": 60,
        "cpu_copies_per_frame": 0.0,
        "paired_fraction": 1.0,
        "presented_since_live": 3_580,
        "pool_max_images": 6,
        "ownership": {"violations": 0, "frames_with_frame_number": 500},
        "latency": {"samples": 3000, "p50_ms": 38.0, "p95_ms": 45.0, "max_ms": 60.0},
        "state_at_end": "streaming",
        "render": {"release_export_failures": 0, "camera_frame_failures": 0, "leases_leaked_on_teardown": 0},
        "validation": {"mode": "enabled", "errors": 0},
        "stream": {"cpu_copies": 0, "dropped_pool_exhausted": 0, "leaked_leases": 0, "frames_leaked": 0},
        "reader": {"pool_exhausted": 0, "images_leaked": 0, "fence_merge_failures": 0, "acquire_errors": 0},
    }
    report.update(overrides)
    return {"scenario": "raw-stream", "status": "passed", "report": report}


class CheckRawStreamTest(unittest.TestCase):
    def problems(self, **overrides: object) -> list[str]:
        return smoke_checks.check_raw_stream(healthy_report(**overrides))

    def test_healthy_report_passes(self) -> None:
        self.assertEqual(self.problems(), [])

    def test_failed_status_is_reported_and_nothing_else_is_guessed(self) -> None:
        payload = healthy_report()
        payload["status"] = "failed"
        payload["error"] = "cpu_copies: 3"
        problems = smoke_checks.check_raw_stream(payload)
        self.assertEqual(len(problems), 1)
        self.assertIn("failed", problems[0])

    def test_fps_below_ninety_percent_fails_and_ninety_passes(self) -> None:
        self.assertTrue(any("меньше 90%" in text for text in self.problems(sensor_fps=53.9)))
        self.assertEqual(self.problems(sensor_fps=54.0), [])
        self.assertTrue(any("не измерена" in text for text in self.problems(sensor_fps=None)))

    def test_fps_is_judged_against_the_fallback_target_after_a_degradation(self) -> None:
        self.assertEqual(self.problems(sensor_fps=29.9, target_fps=30, stream_degraded={"fallback_fps": 30}), [])
        self.assertTrue(any("меньше" in text for text in self.problems(sensor_fps=29.9)))

    def test_any_cpu_copy_or_absent_figure_fails(self) -> None:
        self.assertTrue(any("cpu_copies_per_frame" in text for text in self.problems(cpu_copies_per_frame=0.01)))
        self.assertTrue(any("cpu_copies_per_frame" in text for text in self.problems(cpu_copies_per_frame=None)))

    def test_ownership_violations_fail(self) -> None:
        self.assertTrue(any("владения" in text for text in self.problems(ownership={"violations": 2})))

    def test_nothing_presented_fails(self) -> None:
        self.assertTrue(any("на экран" in text for text in self.problems(presented_since_live=0)))

    def test_engine_not_streaming_at_the_end_fails(self) -> None:
        self.assertTrue(any("состояние" in text for text in self.problems(state_at_end="recovering")))

    def test_validation_errors_fail_only_when_the_layer_is_enabled(self) -> None:
        self.assertTrue(any("validation" in text for text in self.problems(validation={"mode": "enabled", "errors": 1})))
        self.assertEqual(self.problems(validation={"mode": "disabled", "errors": 1}), [])

    def test_each_zero_counter_is_checked_by_name(self) -> None:
        cases = {
            "stream": ("leaked_leases", "stream.leaked_leases"),
            "reader": ("pool_exhausted", "reader.pool_exhausted"),
            "render": ("release_export_failures", "render.release_export_failures"),
        }
        for section, (name, needle) in cases.items():
            base = dict(healthy_report()["report"][section])
            base[name] = 2
            problems = self.problems(**{section: base})
            self.assertTrue(any(needle in text for text in problems), f"{needle}: {problems}")

    def test_trimmed_summary_without_big_counters_is_not_a_failure_by_itself(self) -> None:
        payload = healthy_report()
        for key in ("stream", "reader", "render", "validation"):
            del payload["report"][key]
        payload["report"]["degraded_report"] = True
        self.assertEqual(smoke_checks.check_raw_stream(payload), [])


class CheckCpuProbeTest(unittest.TestCase):
    def test_probe_passes_only_with_counted_copies(self) -> None:
        passing = {"scenario": "raw-stream-cpu-probe", "status": "passed", "report": {"cpu_copies_per_frame": 1.0}}
        self.assertEqual(smoke_checks.check_raw_stream_cpu_probe(passing), [])
        silent = {"scenario": "raw-stream-cpu-probe", "status": "passed", "report": {"cpu_copies_per_frame": 0.0}}
        self.assertTrue(any("не доказан" in text for text in smoke_checks.check_raw_stream_cpu_probe(silent)))
        absent = {"scenario": "raw-stream-cpu-probe", "status": "passed", "report": {}}
        self.assertEqual(len(smoke_checks.check_raw_stream_cpu_probe(absent)), 1)


class EvaluateStreamWithScreenshotsTest(unittest.TestCase):
    """Сквозной путь: стадия viewfinder_live -> настоящие шаги драйвера -> результат -> пост-проверка скриншотов."""

    def run_scenario(self, scenario: str, screen: FakeScreen, payload: dict | None = None) -> smoke_wait.SmokeVerdict:
        with tempfile.TemporaryDirectory() as directory:
            artifacts = Path(directory)
            driver = smoke_driver.StageDriver(scenario, actions_for(screen, artifacts))
            result = payload or healthy_report()
            result["scenario"] = scenario
            lines = [
                stage_line(scenario, "started"),
                stage_line(scenario, "viewfinder_live", after_ms=1100),
                stage_line(scenario, "progress", elapsed_ms=10_000),
                result_line(result),
            ]
            verdict = smoke_wait.evaluate_stream(
                iter(lines),
                scenario,
                driver.handle_stage,
                PACKAGE,
                smoke_wait.post_check_for(scenario, SERIAL, artifacts),
            )
            self.saved = sorted(path.name for path in artifacts.iterdir())
            return verdict

    def test_live_picture_passes_with_screenshot_numbers_and_both_files_saved(self) -> None:
        for scenario in ("raw-stream", "raw-stream-endurance"):
            verdict = self.run_scenario(scenario, FakeScreen(frame(noisy(1)), frame(noisy(2))))
            self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
            self.assertTrue(any("скриншоты: яркость" in note for note in verdict.notes), verdict.notes)
            self.assertEqual(
                self.saved,
                ["viewfinder_a.png", "viewfinder_a.screencap", "viewfinder_b.png", "viewfinder_b.screencap"],
            )

    def test_black_screen_fails_the_scenario_even_though_rust_said_passed(self) -> None:
        verdict = self.run_scenario("raw-stream", FakeScreen(frame(black), frame(black)))
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("экран чёрный", verdict.message)

    def test_frozen_picture_fails(self) -> None:
        verdict = self.run_scenario("raw-stream", FakeScreen(frame(noisy(7)), frame(noisy(7))))
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("не меняется", verdict.message)

    def test_screencap_failure_fails_with_the_stage_in_the_message(self) -> None:
        verdict = self.run_scenario("raw-stream", FakeScreen(b"", returncode=1))
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("viewfinder_live", verdict.message)

    def test_problem_found_by_the_rust_verdict_is_reported_before_screenshots(self) -> None:
        verdict = self.run_scenario(
            "raw-stream", FakeScreen(frame(noisy(1)), frame(noisy(2))), healthy_report(cpu_copies_per_frame=0.5)
        )
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("cpu_copies_per_frame", verdict.message)

    def test_cpu_probe_has_no_screenshot_step_and_no_post_check(self) -> None:
        self.assertIsNone(smoke_wait.post_check_for("raw-stream-cpu-probe", SERIAL, Path("/nonexistent")))
        payload = {"scenario": "raw-stream-cpu-probe", "status": "passed", "report": {"cpu_copies_per_frame": 1.0}}
        verdict = smoke_wait.evaluate_stream(iter([result_line(payload)]), "raw-stream-cpu-probe")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)

    def test_post_check_without_artifacts_dir_is_a_problem_not_a_silent_pass(self) -> None:
        check = smoke_wait.post_check_for("raw-stream", SERIAL, None)
        assert check is not None
        self.assertEqual(len(check({}).problems), 1)


if __name__ == "__main__":
    unittest.main()
