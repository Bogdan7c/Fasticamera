"""Тесты smoke-сценариев камеры (сессия 04): ожидания по отчётам, шаг запуска системной камеры в драйвере
и сверка каталога с ``dumpsys media.camera`` — каждое правило прохода и отказа."""

from __future__ import annotations

import copy
import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import smoke_checks  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import smoke_driver  # noqa: E402
import smoke_dumpsys  # noqa: E402
import smoke_wait  # noqa: E402
from test_smoke_driver import ADB, SERIAL, FakeAdb, fixture, make_actions, result_line, stage_line  # noqa: E402

DUMPSYS_COMMAND = [*ADB, "shell", "dumpsys", "media.camera"]
TRUST_COMMAND = [*ADB, "shell", "dumpsys", "trust"]
SYSTEM_CAMERA_COMMAND = [
    *ADB, "shell", "am", "start", "-W", "-a", "android.media.action.STILL_IMAGE_CAMERA", "-p", "com.oplus.camera",
]
SYSTEM_CAMERA_STARTED = (
    "Starting: Intent { act=android.media.action.STILL_IMAGE_CAMERA pkg=com.oplus.camera }\n"
    "Status: ok\nLaunchState: COLD\nActivity: com.oplus.camera/.component.CameraImageActivity\nTotalTime: 412\n"
)
CHOOSER_STARTED = (
    "Starting: Intent { act=android.media.action.STILL_IMAGE_CAMERA }\n"
    "Status: ok\nActivity: android/com.android.internal.app.ResolverActivity\n"
)


def raw(format_name: str, width: int, height: int) -> dict:
    return {"format": format_name, "width": width, "height": height}


def catalog_camera(camera_id: str, facing: str, raw_streams: list[dict]) -> dict:
    return {
        "id": camera_id,
        "facing": facing,
        "topology": {"kind": "physical"},
        "role": None,
        "hardware_level": "level3",
        "raw_streams": raw_streams,
        "widest_field_of_view_degrees": 82.0,
    }


def matching_cameras() -> list[dict]:
    """Каталог, совпадающий с синтетической фикстурой ``media_camera_synthetic.txt`` (видимые 0, 1, 2)."""
    return [
        catalog_camera("0", "back", [raw("raw10", 4096, 3072), raw("raw16", 4096, 3072), raw("raw10", 4000, 3000)]),
        catalog_camera(
            "1",
            "front",
            [raw("raw10", 2304, 1728), raw("raw16", 2304, 1728), raw("raw10", 2304, 1296), raw("raw16", 2304, 1296)],
        ),
        catalog_camera("2", "back", [raw("raw10", 4096, 3072), raw("raw16", 4096, 3072)]),
    ]


def catalog_payload(**overrides: object) -> dict:
    report: dict = {
        "state": "closed",
        "cameras": matching_cameras(),
        "roles": [
            {"role": "wide", "camera": "2"},
            {"role": "ultra_wide", "camera": "3"},
            {"role": "front", "camera": "1"},
        ],
        "backend": {"callback_panics": 0, "failures_recorded": 0, "skipped_cameras": []},
        "engine_counters": {},
        "event_count": 2,
    }
    report.update(overrides)
    return {"scenario": "camera-catalog", "status": "passed", "report": report}


def step_payload(**overrides: object) -> dict:
    """Сводка шага ``camera-stream`` в строке результата (пустые поля `failure`/`unavailable` не пишутся)."""
    step: dict = {
        "index": 0,
        "target": "2/raw10/16x9@60",
        "lever": "public_ae_target_range",
        "time_to_streaming_ms": 800,
        "measured_fps": 59.97,
        "results_completed": 300,
        "intervals": 299,
        "gaps": 0,
        "frames_missed_in_gaps": 0,
        "min_interval_ns": 16_000_000,
        "max_interval_ns": 17_500_000,
        "last_reported_frame_duration_ns": 33_333_333,
        "missing_keys": [],
        "capture_failures": 0,
        "reader": {"frames_acquired": 300, "images_returned": 299, "acquire_errors": 0},
        "state_at_end": "streaming",
    }
    step.update(overrides)
    return step


def stream_payload(*steps: dict) -> dict:
    return {
        "scenario": "camera-stream",
        "status": "passed",
        "report": {
            "measure_ms": 5000,
            "reader_in_flight_limit": 6,
            "steps": list(steps) or [step_payload()],
            "backend": {"callback_panics": 0},
        },
    }


def evict_payload(**overrides: object) -> dict:
    report: dict = {
        "failure": None,
        "camera": "2",
        "unavailable": {"reason": "evicted", "at_ms": 6000},
        "recovered": {"reason": "evicted", "attempts": 1, "at_ms": 9500, "recovery_ms": 3500},
        "final_state": "streaming",
        "counters": {"recovery_attempts": 1, "recoveries": 1},
    }
    report.update(overrides)
    return {"scenario": "camera-evict", "status": "passed", "report": report}


class DumpsysParseTest(unittest.TestCase):
    def setUp(self) -> None:
        self.cameras = smoke_dumpsys.parse_dumpsys_camera(fixture("media_camera_synthetic.txt"))

    def test_every_hal_device_section_becomes_a_camera_keyed_by_id(self) -> None:
        self.assertEqual(sorted(self.cameras), ["0", "1", "2", "4"])

    def test_facing_comes_from_lens_facing_and_is_lowercase(self) -> None:
        self.assertEqual({key: camera.facing for key, camera in self.cameras.items()}, {"0": "back", "1": "front", "2": "back", "4": "back"})

    def test_only_system_camera_capability_hides_a_camera(self) -> None:
        self.assertEqual({key for key, camera in self.cameras.items() if camera.is_system_camera}, {"4"})
        self.assertIn("LOGICAL_MULTI_CAMERA", self.cameras["0"].capabilities)

    def test_only_raw_output_configurations_are_collected(self) -> None:
        # Не RAW (35/36/33), INPUT и ключ MaximumResolution (37 8192x6144) в набор не входят.
        self.assertEqual(
            self.cameras["0"].raw_outputs,
            {("raw10", 4096, 3072), ("raw16", 4096, 3072), ("raw10", 4000, 3000)},
        )
        self.assertEqual(
            self.cameras["1"].raw_outputs,
            {("raw10", 2304, 1728), ("raw16", 2304, 1728), ("raw10", 2304, 1296), ("raw16", 2304, 1296)},
        )

    def test_later_sections_do_not_leak_into_the_last_camera(self) -> None:
        # После HAL-устройства 4 идут dumpState и Vendor tags с lens.facing [FRONT]: камера 4 остаётся back.
        self.assertEqual(self.cameras["4"].facing, "back")
        self.assertEqual(self.cameras["4"].raw_outputs, {("raw10", 3280, 2464), ("raw16", 3280, 2464)})

    def test_text_without_hal_sections_gives_no_cameras(self) -> None:
        self.assertEqual(smoke_dumpsys.parse_dumpsys_camera("== Service global info: ==\nNumber of camera devices: 0\n"), {})


class CompareCatalogTest(unittest.TestCase):
    def setUp(self) -> None:
        self.dumpsys = smoke_dumpsys.parse_dumpsys_camera(fixture("media_camera_synthetic.txt"))

    def problems(self, cameras: list[dict]) -> list[str]:
        return smoke_dumpsys.compare_catalog(cameras, self.dumpsys)

    def test_matching_catalog_has_no_problems_and_hidden_camera_absent_is_correct(self) -> None:
        self.assertEqual(self.problems(matching_cameras()), [])

    def test_visible_camera_missing_from_catalog_is_reported(self) -> None:
        problems = self.problems(matching_cameras()[:2])
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("камера 2", problems[0])
        self.assertIn("отсутствует в каталоге", problems[0])

    def test_hidden_camera_in_catalog_violates_invariant_six(self) -> None:
        cameras = [*matching_cameras(), catalog_camera("4", "back", [raw("raw10", 3280, 2464), raw("raw16", 3280, 2464)])]
        problems = self.problems(cameras)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("камера 4", problems[0])
        self.assertIn("инвариант 6", problems[0])

    def test_camera_dumpsys_does_not_know_is_reported(self) -> None:
        problems = self.problems([*matching_cameras(), catalog_camera("9", "back", [])])
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("камера 9", problems[0])
        self.assertIn("dumpsys о ней не знает", problems[0])

    def test_facing_mismatch_is_reported_with_both_values(self) -> None:
        cameras = matching_cameras()
        cameras[1]["facing"] = "back"
        problems = self.problems(cameras)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("камера 1", problems[0])
        self.assertIn("'back'", problems[0])
        self.assertIn("'front'", problems[0])

    def test_raw_size_mismatch_is_reported(self) -> None:
        cameras = matching_cameras()
        cameras[2]["raw_streams"][0] = raw("raw10", 4000, 3000)
        problems = self.problems(cameras)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("камера 2", problems[0])
        self.assertIn("raw10 4000x3000", problems[0])
        self.assertIn("raw10 4096x3072", problems[0])

    def test_missing_raw_format_and_extra_raw_stream_are_both_mismatches(self) -> None:
        missing = matching_cameras()
        missing[0]["raw_streams"].pop()
        self.assertEqual(len(self.problems(missing)), 1)
        extra = matching_cameras()
        extra[2]["raw_streams"].append(raw("raw16", 1000, 1000))
        self.assertEqual(len(self.problems(extra)), 1)

    def test_every_divergence_is_counted_separately(self) -> None:
        cameras = matching_cameras()[1:]  # нет камеры 0
        cameras[0]["facing"] = "back"
        cameras[1]["raw_streams"] = []
        self.assertEqual(len(self.problems(cameras)), 3)

    def test_empty_dumpsys_is_a_problem_not_a_silent_pass(self) -> None:
        problems = smoke_dumpsys.compare_catalog(matching_cameras(), {})
        self.assertEqual(len(problems), 1)
        self.assertIn("формат вывода", problems[0])


class DumpsysPostCheckTest(unittest.TestCase):
    def check(self, result: dict, dumpsys_text: str = "", returncode: int = 0) -> tuple[list[str], list[list[str]]]:
        commands: list[list[str]] = []

        def run_adb(command: list[str]) -> smoke_driver.AdbResult:
            commands.append(command)
            return smoke_driver.AdbResult(returncode, dumpsys_text, "device offline" if returncode else "")

        return smoke_dumpsys.dumpsys_post_check(SERIAL, run_adb)(result).problems, commands

    def test_runs_dumpsys_on_the_given_serial_and_passes_when_catalog_matches(self) -> None:
        problems, commands = self.check(catalog_payload(), fixture("media_camera_synthetic.txt"))
        self.assertEqual(problems, [])
        self.assertEqual(commands, [DUMPSYS_COMMAND])

    def test_mismatch_comes_back_as_problems(self) -> None:
        result = catalog_payload()
        result["report"]["cameras"][1]["facing"] = "back"
        problems, _ = self.check(result, fixture("media_camera_synthetic.txt"))
        self.assertEqual(len(problems), 1, problems)

    def test_adb_failure_is_a_problem_with_the_cause(self) -> None:
        problems, _ = self.check(catalog_payload(), returncode=1)
        self.assertEqual(len(problems), 1)
        self.assertIn("device offline", problems[0])

    def test_report_without_cameras_is_a_problem_and_dumpsys_is_not_read(self) -> None:
        problems, commands = self.check({"scenario": "camera-catalog", "status": "passed", "report": {}})
        self.assertEqual(len(problems), 1)
        self.assertEqual(commands, [])


class DescribeMatchTest(unittest.TestCase):
    def setUp(self) -> None:
        self.dumpsys = smoke_dumpsys.parse_dumpsys_camera(fixture("media_camera_synthetic.txt"))

    def test_evidence_line_names_ids_hidden_cameras_and_raw_configuration_counts(self) -> None:
        line = smoke_dumpsys.describe_match(matching_cameras(), self.dumpsys)
        self.assertIn("видимые ID [0, 1, 2]", line)
        self.assertIn("скрытые [4] (SYSTEM_CAMERA) в каталоге отсутствуют", line)
        # Камера 0: 3 конфигурации, 1: 4, 2: 2 — всего 9; скрытая 4 не считается.
        self.assertIn("RAW-конфигураций совпало 9 (0: 3, 1: 4, 2: 2)", line)

    def test_phone_without_hidden_cameras_says_so(self) -> None:
        visible_only = {key: camera for key, camera in self.dumpsys.items() if key != "4"}
        self.assertIn("скрытых камер нет", smoke_dumpsys.describe_match(matching_cameras(), visible_only))

    def test_successful_post_check_carries_the_line_and_a_failed_one_does_not(self) -> None:
        def run_adb(command: list[str]) -> smoke_driver.AdbResult:
            return smoke_driver.AdbResult(0, fixture("media_camera_synthetic.txt"), "")

        check = smoke_dumpsys.dumpsys_post_check(SERIAL, run_adb)
        good = check(catalog_payload())
        self.assertEqual(good.problems, [])
        self.assertEqual(len(good.notes), 1)
        self.assertIn("RAW-конфигураций совпало 9", good.notes[0])
        broken = catalog_payload()
        broken["report"]["cameras"][1]["facing"] = "back"
        bad = check(broken)
        self.assertEqual(len(bad.problems), 1)
        self.assertEqual(bad.notes, [], "при расхождении «всё совпало» не пишется")

    def test_main_prints_the_evidence_line_after_a_passing_catalog_verdict(self) -> None:
        import contextlib
        import io

        original_logcat, original_post = smoke_wait.read_logcat_lines, smoke_wait.post_check_for
        smoke_wait.read_logcat_lines = lambda *_args: iter([result_line(catalog_payload())])  # type: ignore[assignment]
        smoke_wait.post_check_for = lambda scenario, serial, _artifacts_dir=None: smoke_dumpsys.dumpsys_post_check(  # type: ignore[assignment]
            serial, lambda command: smoke_driver.AdbResult(0, fixture("media_camera_synthetic.txt"), "")
        )
        output = io.StringIO()
        try:
            with contextlib.redirect_stdout(output):
                code = smoke_wait.main(
                    ["--serial", SERIAL, "--since-epoch", "1.000", "--package", "dev.fasticamera.debug",
                     "--scenario", "camera-catalog", "--timeout", "5"]
                )
        finally:
            smoke_wait.read_logcat_lines, smoke_wait.post_check_for = original_logcat, original_post  # type: ignore[assignment]
        self.assertEqual(code, smoke_wait.EXIT_OK)
        printed = output.getvalue()
        self.assertIn("сценарий camera-catalog прошёл", printed)
        self.assertIn("сверка с dumpsys: видимые ID [0, 1, 2] совпали", printed)


class CatalogScenarioVerdictTest(unittest.TestCase):
    """Сквозное: строка результата → проверка отчёта → сверка с dumpsys → вердикт."""

    def verdict(self, result: dict, dumpsys_text: str) -> smoke_wait.SmokeVerdict:
        adb = FakeAdb()
        original_call = adb.__call__

        def run_adb(command: list[str]) -> smoke_driver.AdbResult:
            if command == DUMPSYS_COMMAND:
                return smoke_driver.AdbResult(0, dumpsys_text, "")
            return original_call(command)

        post_check = smoke_dumpsys.dumpsys_post_check(SERIAL, run_adb)
        return smoke_wait.evaluate_stream([result_line(result)], "camera-catalog", None, None, post_check)

    def test_catalog_that_matches_dumpsys_passes(self) -> None:
        verdict = self.verdict(catalog_payload(), fixture("media_camera_synthetic.txt"))
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)

    def test_catalog_that_differs_from_dumpsys_fails_the_scenario_with_the_difference(self) -> None:
        result = catalog_payload()
        result["report"]["cameras"][2]["raw_streams"] = [raw("raw10", 4096, 3072)]
        verdict = self.verdict(result, fixture("media_camera_synthetic.txt"))
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("камера 2", verdict.message)
        self.assertIn("RAW-потоки", verdict.message)

    def test_post_check_is_not_consulted_when_the_report_itself_is_bad(self) -> None:
        called: list[dict] = []
        result = catalog_payload(cameras=[])
        verdict = smoke_wait.evaluate_stream([result_line(result)], "camera-catalog", None, None, lambda r: called.append(r) or smoke_dumpsys.CheckOutcome([]))
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertEqual(called, [])

    def test_only_camera_catalog_gets_the_dumpsys_post_check(self) -> None:
        self.assertIsNotNone(smoke_wait.post_check_for("camera-catalog", SERIAL))
        for scenario in ("camera-stream", "camera-evict", "hello", "gpu-fence"):
            self.assertIsNone(smoke_wait.post_check_for(scenario, SERIAL), scenario)


class CameraCatalogCheckTest(unittest.TestCase):
    def test_healthy_catalog_passes(self) -> None:
        self.assertEqual(smoke_checks.check_camera_catalog(catalog_payload()), [])

    def test_failed_scenario_reports_status_and_error(self) -> None:
        problems = smoke_checks.check_camera_catalog({"scenario": "camera-catalog", "status": "failed", "error": "каталог пуст"})
        self.assertEqual(len(problems), 1)
        self.assertIn("каталог пуст", problems[0])

    def test_empty_catalog_fails(self) -> None:
        self.assertEqual(smoke_checks.check_camera_catalog(catalog_payload(cameras=[])), ["каталог пуст или отсутствует (report.cameras)"])

    def test_hidden_cameras_four_and_five_must_not_be_in_the_catalog(self) -> None:
        for hidden in ("4", "5"):
            cameras = [*matching_cameras(), catalog_camera(hidden, "back", [raw("raw10", 3280, 2464)])]
            problems = smoke_checks.check_camera_catalog(catalog_payload(cameras=cameras))
            self.assertTrue(any(f"скрытая камера {hidden}" in text for text in problems), problems)

    def test_camera_without_raw_streams_or_required_field_fails(self) -> None:
        cameras = matching_cameras()
        cameras[2]["raw_streams"] = []
        del cameras[1]["hardware_level"]
        problems = smoke_checks.check_camera_catalog(catalog_payload(cameras=cameras))
        self.assertTrue(any("нет ни одного RAW-потока" in text and "'2'" in text for text in problems), problems)
        self.assertTrue(any("нет поля hardware_level" in text for text in problems), problems)

    def test_missing_role_fails_naming_the_role(self) -> None:
        roles = [{"role": "wide", "camera": "2"}, {"role": "front", "camera": "1"}]
        problems = smoke_checks.check_camera_catalog(catalog_payload(roles=roles))
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("ultra_wide", problems[0])

    def test_backend_panics_and_missing_backend_snapshot_fail(self) -> None:
        panicked = smoke_checks.check_camera_catalog(catalog_payload(backend={"callback_panics": 2}))
        self.assertTrue(any("паники колбэков" in text for text in panicked), panicked)
        absent = smoke_checks.check_camera_catalog(catalog_payload(backend=None))
        self.assertTrue(any("report.backend" in text for text in absent), absent)


class CameraStreamCheckTest(unittest.TestCase):
    def problems(self, **overrides: object) -> list[str]:
        return smoke_checks.check_camera_stream(stream_payload(step_payload(**overrides)))

    def test_healthy_run_passes(self) -> None:
        self.assertEqual(smoke_checks.check_camera_stream(stream_payload(step_payload(), step_payload(index=1))), [])

    def test_failed_scenario_is_a_failure_with_its_error(self) -> None:
        problems = smoke_checks.check_camera_stream({"scenario": "camera-stream", "status": "failed", "error": "шаг 3: частота"})
        self.assertIn("шаг 3: частота", problems[0])

    def test_no_steps_fails(self) -> None:
        payload = stream_payload()
        payload["report"]["steps"] = []
        self.assertEqual(smoke_checks.check_camera_stream(payload), ["нет шагов в отчёте (report.steps)"])

    def test_step_failure_is_surfaced_with_the_step_label(self) -> None:
        problems = self.problems(failure={"kind": "never_streaming"})
        self.assertTrue(any("never_streaming" in text and "2/raw10/16x9@60" in text for text in problems), problems)

    def test_frame_rate_below_ninety_percent_of_the_target_fails(self) -> None:
        low = self.problems(measured_fps=53.9)
        self.assertTrue(any("53.90" in text and "90%" in text for text in low), low)
        self.assertEqual(self.problems(measured_fps=54.0), [])
        unmeasured = self.problems(measured_fps=None)
        self.assertTrue(any("не измерена" in text for text in unmeasured), unmeasured)

    def test_target_rate_comes_from_the_label_of_each_step(self) -> None:
        thirty = step_payload(target="1/raw10/4x3@30", measured_fps=27.0)
        self.assertEqual(smoke_checks.check_camera_stream(stream_payload(thirty)), [])
        slow = step_payload(target="1/raw10/4x3@30", measured_fps=26.9)
        self.assertEqual(len(smoke_checks.check_camera_stream(stream_payload(slow))), 1)
        no_rate = self.problems(target="без частоты")
        self.assertTrue(any("нет частоты" in text for text in no_rate), no_rate)

    def test_degraded_summary_without_details_is_still_judged_by_rate_and_raw_counters(self) -> None:
        trimmed = step_payload()
        for key in ("results_completed", "intervals", "gaps", "min_interval_ns", "missing_keys", "capture_failures"):
            del trimmed[key]
        payload = stream_payload(trimmed)
        payload["report"]["degraded"] = True
        self.assertEqual(smoke_checks.check_camera_stream(payload), [])

    def test_reader_without_frames_and_wrong_final_state_fail(self) -> None:
        no_frames = self.problems(reader={"frames_acquired": 0})
        self.assertTrue(any("не получил кадров" in text for text in no_frames), no_frames)
        absent = self.problems(reader=None)
        self.assertTrue(any("не получил кадров" in text for text in absent), absent)
        stopped = self.problems(state_at_end="error")
        self.assertTrue(any("'error'" in text for text in stopped), stopped)

    def partial_step(self, **overrides: object) -> dict:
        step = step_payload(
            target="0/raw10/4x3@30", capture_results="not_sent_by_hal", raw_fps=29.9, results_completed=0, lever="hal_default"
        )
        del step["measured_fps"]
        step.update(overrides)
        return step

    def test_partial_only_step_is_judged_by_raw_rate_and_ignores_missing_sensor_rate(self) -> None:
        self.assertEqual(smoke_checks.check_camera_stream(stream_payload(self.partial_step())), [])

    def test_partial_only_step_fails_on_low_or_missing_raw_rate(self) -> None:
        low = smoke_checks.check_camera_stream(stream_payload(self.partial_step(raw_fps=26.9)))
        self.assertTrue(any("по RAW-кадрам" in text and "26.90" in text for text in low), low)
        missing = smoke_checks.check_camera_stream(stream_payload(self.partial_step(raw_fps=None)))
        self.assertTrue(any("не измерена" in text for text in missing), missing)

    def test_final_results_on_a_partial_only_step_are_a_changed_fact(self) -> None:
        problems = smoke_checks.check_camera_stream(stream_payload(self.partial_step(results_completed=7)))
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("факт изменился (прошивка?)", problems[0])

    def test_ordinary_step_still_needs_the_sensor_rate_even_if_raw_rate_is_present(self) -> None:
        step = step_payload(raw_fps=60.0)
        del step["measured_fps"]
        problems = smoke_checks.check_camera_stream(stream_payload(step))
        self.assertTrue(any("не измерена" in text for text in problems), problems)

    def test_backend_panics_fail_the_whole_run(self) -> None:
        payload = stream_payload()
        payload["report"]["backend"] = {"callback_panics": 1}
        problems = smoke_checks.check_camera_stream(payload)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("паники колбэков", problems[0])
        payload["report"]["backend"] = None
        self.assertTrue(any("report.backend" in text for text in smoke_checks.check_camera_stream(payload)))

    def test_second_step_failure_is_attributed_to_step_two(self) -> None:
        problems = smoke_checks.check_camera_stream(stream_payload(step_payload(), step_payload(index=1, measured_fps=10.0)))
        self.assertEqual(len(problems), 1, problems)
        self.assertTrue(problems[0].startswith("шаг 2"), problems)


class DetailLinesTest(unittest.TestCase):
    """Подробности отчёта идут строками ``detail_*``: вердикт их собирает, драйвер на них ничего не делает."""

    def detail(self, scenario: str, stage: str, part: int, parts: int, items: list) -> str:
        return stage_line(scenario, stage, part=part, parts=parts, items=items)

    def test_verdict_collects_only_detail_stages_of_the_scenario_in_order(self) -> None:
        lines = [
            stage_line("camera-stream", "started", steps=2),
            self.detail("camera-stream", "detail_steps", 1, 2, [{"index": 0}]),
            self.detail("camera-stream", "detail_steps", 2, 2, [{"index": 1}]),
            self.detail("camera-catalog", "detail_cameras", 1, 1, [{"id": "9"}]),
            result_line(stream_payload()),
        ]
        verdict = smoke_wait.evaluate_stream(lines, "camera-stream")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        self.assertEqual([(d["stage"], d["part"]) for d in verdict.details], [("detail_steps", 1), ("detail_steps", 2)])
        self.assertEqual(verdict.details[1]["items"], [{"index": 1}])

    def test_details_are_kept_when_the_scenario_fails(self) -> None:
        failed = stream_payload(step_payload(measured_fps=10.0))
        lines = [self.detail("camera-stream", "detail_steps", 1, 1, [{"index": 0}]), result_line(failed)]
        verdict = smoke_wait.evaluate_stream(lines, "camera-stream")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertEqual(len(verdict.details), 1)

    def test_detail_stage_does_not_run_driver_steps(self) -> None:
        adb = FakeAdb()
        driver = smoke_driver.StageDriver("camera-evict", make_actions(adb))
        lines = [self.detail("camera-evict", "detail_x", 1, 1, []), result_line(evict_payload())]
        smoke_wait.evaluate_stream(lines, "camera-evict", driver.handle_stage)
        self.assertEqual(adb.commands, [])

    def test_main_prints_details_after_the_verdict_with_a_greppable_marker(self) -> None:
        import contextlib
        import io

        lines = [self.detail("camera-stream", "detail_steps", 1, 1, [{"index": 0, "note": "русский"}]), result_line(stream_payload())]
        original = smoke_wait.read_logcat_lines
        smoke_wait.read_logcat_lines = lambda *_args: iter(lines)  # type: ignore[assignment]
        output = io.StringIO()
        try:
            with contextlib.redirect_stdout(output):
                code = smoke_wait.main(
                    ["--serial", SERIAL, "--since-epoch", "1.000", "--package", "dev.fasticamera.debug",
                     "--scenario", "camera-stream", "--timeout", "5"]
                )
        finally:
            smoke_wait.read_logcat_lines = original  # type: ignore[assignment]
        self.assertEqual(code, smoke_wait.EXIT_OK)
        printed = output.getvalue().splitlines()
        self.assertIn("сценарий camera-stream прошёл", printed[0])
        marker_lines = [line for line in printed[1:] if line.startswith(smoke_wait.DETAIL_LINE_MARKER)]
        self.assertEqual(len(marker_lines), 1)
        self.assertIn("русский", marker_lines[0], "кириллица не экранируется")


class CameraEvictCheckTest(unittest.TestCase):
    def test_healthy_eviction_passes_for_every_accepted_reason(self) -> None:
        for reason in sorted(smoke_checks.ACCEPTED_EVICTION_REASONS):
            payload = evict_payload(unavailable={"reason": reason, "at_ms": 1})
            self.assertEqual(smoke_checks.check_camera_evict(payload), [], reason)

    def test_unexpected_reason_fails_naming_it(self) -> None:
        problems = smoke_checks.check_camera_evict(evict_payload(unavailable={"reason": "device_failure", "at_ms": 1}))
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("device_failure", problems[0])

    def test_no_unavailable_or_no_recovery_fails(self) -> None:
        none_seen = smoke_checks.check_camera_evict(evict_payload(unavailable=None))
        self.assertTrue(any("None" in text for text in none_seen), none_seen)
        no_recovery = smoke_checks.check_camera_evict(evict_payload(recovered=None))
        self.assertTrue(any("Recovered" in text for text in no_recovery), no_recovery)

    def test_wrong_final_state_zero_recoveries_and_recorded_failure_fail(self) -> None:
        state = smoke_checks.check_camera_evict(evict_payload(final_state="error"))
        self.assertTrue(any("final_state" in text for text in state), state)
        counter = smoke_checks.check_camera_evict(evict_payload(counters={"recoveries": 0}))
        self.assertTrue(any("recoveries" in text for text in counter), counter)
        failure = smoke_checks.check_camera_evict(evict_payload(failure={"kind": "never_recovered"}))
        self.assertTrue(any("never_recovered" in text for text in failure), failure)

    def test_failed_scenario_is_a_failure(self) -> None:
        problems = smoke_checks.check_camera_evict({"scenario": "camera-evict", "status": "failed", "error": "вытеснения не было"})
        self.assertIn("вытеснения не было", problems[0])


class CameraFakeAdb(FakeAdb):
    """FakeAdb, который на запуск системной камеры отвечает как телефон (``am start -W``)."""

    def __init__(self, launch_output: str = SYSTEM_CAMERA_STARTED, **kwargs) -> None:
        super().__init__(**kwargs)
        self._launch_output = launch_output

    def __call__(self, command: list[str]) -> smoke_driver.AdbResult:
        result = super().__call__(command)
        if command == SYSTEM_CAMERA_COMMAND and result.returncode == 0:
            return smoke_driver.AdbResult(0, self._launch_output, "")
        return result


class SystemCameraStepTest(unittest.TestCase):
    def test_launch_command_uses_explicit_serial_action_and_system_camera_package(self) -> None:
        adb = CameraFakeAdb()
        make_actions(adb).launch_system_camera()
        self.assertEqual(adb.commands, [TRUST_COMMAND, SYSTEM_CAMERA_COMMAND])

    def test_locked_phone_is_a_driver_error_before_launching_anything(self) -> None:
        # Факт 08b: наше приложение видно поверх блокировки, а системная камера — нет; вытеснения не было.
        adb = CameraFakeAdb()
        adb.device_locked = True
        with self.assertRaises(smoke_driver.DriverError) as raised:
            make_actions(adb).launch_system_camera()
        self.assertIn("заблокирован", str(raised.exception))
        self.assertNotIn(SYSTEM_CAMERA_COMMAND, adb.commands, "системную камеру при блокировке не запускаем")

    def test_device_locked_is_read_for_the_current_user_only(self) -> None:
        output = (
            ' User "Guest" (id=10, flags=0x0): trustState=UNTRUSTED, deviceLocked=0\n'
            ' User "Owner" (id=0, flags=0x4c13) (current): trustState=UNTRUSTED, deviceLocked=1\n'
        )
        self.assertTrue(smoke_driver.parse_device_locked(output))
        with self.assertRaises(smoke_driver.DriverError):
            smoke_driver.parse_device_locked("no trust info")

    def test_chooser_instead_of_system_camera_is_a_driver_error(self) -> None:
        # Факт smoke 04: без пакета интент открывал диалог выбора, и вытеснения не было.
        adb = CameraFakeAdb(launch_output=CHOOSER_STARTED)
        with self.assertRaises(smoke_driver.DriverError) as raised:
            make_actions(adb).launch_system_camera()
        self.assertIn("ResolverActivity", str(raised.exception))

    def test_launch_without_activity_report_is_a_driver_error(self) -> None:
        adb = CameraFakeAdb(launch_output="Status: ok\n")
        with self.assertRaises(smoke_driver.DriverError):
            make_actions(adb).launch_system_camera()

    def test_launched_activity_is_parsed_from_am_start_wait_output(self) -> None:
        self.assertEqual(
            smoke_driver.launched_activity(SYSTEM_CAMERA_STARTED), "com.oplus.camera/.component.CameraImageActivity"
        )
        self.assertIsNone(smoke_driver.launched_activity("Status: ok\nTotalTime: 3\n"))

    def test_adb_failure_becomes_driver_error(self) -> None:
        adb = FakeAdb(failing_prefix=[*ADB, "shell", "am", "start"])
        with self.assertRaises(smoke_driver.DriverError) as raised:
            make_actions(adb).launch_system_camera()
        self.assertIn("системной камеры", str(raised.exception))

    def test_am_start_error_text_with_zero_exit_code_is_failure(self) -> None:
        def run_adb(command: list[str]) -> smoke_driver.AdbResult:
            return smoke_driver.AdbResult(0, "Error: Activity not started, unable to resolve Intent\n", "")

        actions = smoke_driver.AdbDeviceActions(SERIAL, "dev.fasticamera.debug", run_adb=run_adb)
        with self.assertRaises(smoke_driver.DriverError):
            actions.launch_system_camera()

    def test_evict_streaming_stage_launches_system_camera_waits_and_returns_to_the_app(self) -> None:
        adb = CameraFakeAdb()
        driver = smoke_driver.StageDriver("camera-evict", make_actions(adb))
        driver.handle_stage({"scenario": "camera-evict", "stage": "streaming", "camera": "2"})
        self.assertEqual(adb.commands[0], TRUST_COMMAND, "сначала проверка блокировки (факт 08b)")
        self.assertEqual(adb.commands[1], SYSTEM_CAMERA_COMMAND)
        self.assertEqual(adb.commands[2][: len(ADB) + 2], [*ADB, "shell", "am"], "возврат приложения — через am start")
        self.assertIn("-n", adb.commands[2])
        self.assertIn(f"dev.fasticamera.debug/{smoke_driver.MAIN_ACTIVITY}", adb.commands[2])
        self.assertEqual(len(adb.commands), 3)

    def test_hold_between_launch_and_return_comes_from_the_table(self) -> None:
        steps = smoke_driver.SCENARIO_STEPS["camera-evict"]["streaming"]
        self.assertEqual(
            [step.kind for step in steps],
            [
                smoke_driver.StepKind.LAUNCH_SYSTEM_CAMERA,
                smoke_driver.StepKind.PAUSE,
                smoke_driver.StepKind.RETURN_TO_APP,
            ],
        )
        self.assertEqual(steps[1].seconds, smoke_driver.SYSTEM_CAMERA_HOLD_SECONDS)

    def test_other_evict_stages_and_other_scenarios_run_nothing(self) -> None:
        adb = FakeAdb()
        driver = smoke_driver.StageDriver("camera-evict", make_actions(adb))
        driver.handle_stage({"scenario": "camera-evict", "stage": "started"})
        driver.handle_stage({"scenario": "camera-stream", "stage": "streaming"})
        other = smoke_driver.StageDriver("camera-stream", make_actions(adb))
        other.handle_stage({"scenario": "camera-stream", "stage": "streaming"})
        self.assertEqual(adb.commands, [])

    def test_whole_evict_run_stage_then_result_in_logcat_order(self) -> None:
        adb = CameraFakeAdb()
        sleeps: list[float] = []
        actions = smoke_driver.AdbDeviceActions(SERIAL, "dev.fasticamera.debug", run_adb=adb, sleep=sleeps.append)
        driver = smoke_driver.StageDriver("camera-evict", actions)
        lines = [stage_line("camera-evict", "started"), stage_line("camera-evict", "streaming", camera="2"), result_line(evict_payload())]
        verdict = smoke_wait.evaluate_stream(lines, "camera-evict", driver.handle_stage)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        self.assertEqual(sleeps, [smoke_driver.SYSTEM_CAMERA_HOLD_SECONDS])
        self.assertEqual(adb.commands[:2], [TRUST_COMMAND, SYSTEM_CAMERA_COMMAND])


if __name__ == "__main__":
    unittest.main()
