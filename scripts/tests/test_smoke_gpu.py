"""Тесты smoke-сценариев сессии 03 (render, render-lifecycle, ahb-import, gpu-fence):
шаги драйвера на стадиях, команды adb и ожидания по отчёту — каждое правило прохода и отказа."""

from __future__ import annotations

import copy
import json
import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import smoke_checks  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import smoke_driver  # noqa: E402
import smoke_wait  # noqa: E402
from test_smoke_driver import ADB, PACKAGE, FakeAdb, FakeClock, make_actions, result_line, stage_line  # noqa: E402

ENABLED = {"mode": "enabled", "errors": 0, "warnings": 0}


def render_payload(**overrides: object) -> dict:
    report = {
        "state": "Presenting",
        "frames_presented": 1200,
        "acquire_timeouts": 0,
        "recreations": 0,
        "backend_errors": 0,
        "last_backend_error": None,
        "pacing": {
            "histogram": [0, 1200, 0, 0, 0, 0, 0, 0],
            "intervals": 1200,
            "misses": 0,
            "miss_ratio": 0.0,
            "max_miss_ratio": 0.005,
            "observed_span_s": 10.0,
            "timing_gaps": 0,
            "frames_covered": 1201,
            "coverage_ratio": 1200 / 1201,
            "min_coverage_ratio": 0.9,
            "rejected_out_of_order": 0,
        },
        "refresh": {
            "measured_period_ms": 8.333,
            "expected_period_ms": 8.333,
            "pacing_period_ms": 8.333,
            "tolerance_ratio": 0.05,
            "settled_after_ms": 400.0,
            "synthetic_touch_requested": True,
        },
        "display": {
            "format": "B8G8R8A8_UNORM",
            "color_space": "SRGB_NONLINEAR_KHR",
            "width_px": 1440,
            "height_px": 3216,
            "transform": "Identity",
            "image_count": 3,
            "recreation_count": 0,
        },
        "validation": dict(ENABLED),
        "frame_rate_request": {"result": "applied"},
    }
    report.update(overrides)
    return {"scenario": "render", "status": "passed", "report": report}


def lifecycle_payload(**overrides: object) -> dict:
    report = {
        "planned_cycles": 3,
        "idle_baseline_live_objects": {"Fence": 3, "Image": 5},
        "cycles": [
            {"cycle": 1, "kind": "home", "release_latency_ms": 12, "presenting_live_objects": 21, "idle_live_objects": {"Fence": 3, "Image": 5}},
            {"cycle": 2, "kind": "home", "release_latency_ms": 30, "presenting_live_objects": 20, "idle_live_objects": {"Fence": 3, "Image": 5}},
            {"cycle": 3, "kind": "screen", "release_latency_ms": 18, "presenting_live_objects": 21, "idle_live_objects": {"Fence": 3, "Image": 5}},
        ],
        "releases_confirmed": 3,
        "surface_release_timeouts": 0,
        "activity_recreated": False,
        "validation": dict(ENABLED),
        "fault": None,
        "backend_errors": 0,
        "detach_failures": 0,
        "release_latency": {"max_ms": 30.0, "median_ms": 18.0},
    }
    report.update(overrides)
    return {"scenario": "render-lifecycle", "status": "passed", "report": report}


def ahb_entry(outcome: str = "matched", **summary_overrides: object) -> dict:
    summary = {
        "matched": True,
        "second_import_hit": True,
        "live_objects_restored": True,
        "validation_errors": 0,
        "validation": "enabled",
        "details": "ok",
    }
    summary.update(summary_overrides)
    return {"format": "raw16", "outcome": outcome, "details": "ok", "summary": None if outcome == "unsupported" else summary}


def ahb_payload(raw16: dict | None = None, raw10: dict | None = None) -> dict:
    return {
        "scenario": "ahb-import",
        "status": "passed",
        "report": {"width": 4096, "height": 3072, "raw16": raw16 or ahb_entry(), "raw10": raw10 or ahb_entry()},
    }


def fence_payload(**overrides: object) -> dict:
    report = {
        "busy_iterations": 500000,
        "expected_value": 7,
        "work_a_value": 7,
        "work_b_value": 7,
        "b_started_after_a_finished": True,
        "work_a_duration_ns": 14000000,
        "work_b_duration_ns": 20000,
        "gap_between_a_end_and_b_begin_ns": 35000,
        "release_fence_pending_at_export": True,
        "exported_fence_was_already_signaled": False,
        "already_signaled_wait_semaphore_created": False,
        "already_signaled_work_completed": True,
        "already_signaled_result_value": 7,
        "validation_errors": 0,
        "validation": "enabled",
        "error": None,
    }
    report.update(overrides)
    return {"scenario": "gpu-fence", "status": "passed", "report": report}


def with_nested(payload: dict, path: list[str], value: object) -> dict:
    """Копия payload с заменённым значением по пути внутри ``report``."""
    changed = copy.deepcopy(payload)
    node = changed["report"]
    for key in path[:-1]:
        node = node[key]
    node[path[-1]] = value
    return changed


class RenderLifecycleDriverTest(unittest.TestCase):
    """Стадии ``render-lifecycle`` → команды adb в нужном порядке."""

    def drive(self, stage: str) -> tuple[FakeAdb, FakeClock]:
        adb, clock = FakeAdb(), FakeClock()
        driver = smoke_driver.StageDriver("render-lifecycle", make_actions(adb, clock))
        driver.handle_stage({"scenario": "render-lifecycle", "stage": stage, "cycle": 1})
        return adb, clock

    def test_presenting_home_presses_home(self) -> None:
        adb, _ = self.drive("presenting_home")
        self.assertEqual(adb.commands, [[*ADB, "shell", "input", "keyevent", "KEYCODE_HOME"]])

    def test_backgrounded_home_pauses_then_returns_to_app(self) -> None:
        adb, clock = self.drive("backgrounded_home")
        self.assertEqual(clock.sleeps, [smoke_driver.BACKGROUND_PAUSE_SECONDS])
        self.assertEqual(len(adb.commands), 1)
        self.assertIn(f"{PACKAGE}/dev.fasticamera.MainActivity", adb.commands[0])

    def test_presenting_screen_presses_power_once_when_awake(self) -> None:
        adb, clock = self.drive("presenting_screen")
        self.assertEqual(adb.keycodes(), ["KEYCODE_POWER"])
        self.assertEqual(adb.wakefulness, "Dozing", "экран реально погас")
        self.assertEqual(clock.sleeps[-1], smoke_driver.SCREEN_TRANSITION_PAUSE_SECONDS)

    def test_backgrounded_screen_waits_then_wakes_device(self) -> None:
        adb = FakeAdb(wakefulness="Dozing")
        clock = FakeClock()
        driver = smoke_driver.StageDriver("render-lifecycle", make_actions(adb, clock))
        driver.handle_stage({"scenario": "render-lifecycle", "stage": "backgrounded_screen", "cycle": 1})
        self.assertEqual(adb.keycodes(), ["KEYCODE_WAKEUP"])
        self.assertEqual(adb.wakefulness, "Awake")
        self.assertEqual(
            clock.sleeps,
            [smoke_driver.BACKGROUND_PAUSE_SECONDS, smoke_driver.SCREEN_TRANSITION_PAUSE_SECONDS],
        )

    def test_adb_failure_on_screen_off_becomes_driver_error(self) -> None:
        adb = FakeAdb(failing_prefix=[*ADB, "shell", "input", "keyevent", "KEYCODE_POWER"])
        driver = smoke_driver.StageDriver("render-lifecycle", make_actions(adb))
        with self.assertRaisesRegex(smoke_driver.DriverError, "KEYCODE_POWER.*device offline"):
            driver.handle_stage({"scenario": "render-lifecycle", "stage": "presenting_screen"})

    def test_stages_of_render_and_other_scenarios_do_nothing(self) -> None:
        for scenario, stage in [("render", "refresh_settled"), ("ahb-import", "started"), ("gpu-fence", "started")]:
            adb = FakeAdb()
            driver = smoke_driver.StageDriver(scenario, make_actions(adb))
            driver.handle_stage({"scenario": scenario, "stage": stage})
            self.assertEqual(adb.commands, [], scenario)

    def test_full_cycle_stream_runs_every_command_in_order(self) -> None:
        adb = FakeAdb()
        driver = smoke_driver.StageDriver("render-lifecycle", make_actions(adb))
        lines = [
            stage_line("render-lifecycle", "presenting_home", cycle=1),
            stage_line("render-lifecycle", "backgrounded_home", cycle=1),
            stage_line("render-lifecycle", "presenting_screen", cycle=2),
            stage_line("render-lifecycle", "backgrounded_screen", cycle=2),
            result_line(lifecycle_payload()),
        ]
        verdict = smoke_wait.evaluate_stream(lines, "render-lifecycle", driver.handle_stage)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        keycodes = [command[-1] for command in adb.commands if "keyevent" in command]
        self.assertEqual(keycodes, ["KEYCODE_HOME", "KEYCODE_POWER", "KEYCODE_WAKEUP"])
        self.assertEqual(adb.wakefulness, "Awake", "после цикла экран снова включён")


class ScreenPowerTest(unittest.TestCase):
    """POWER — переключатель, поэтому его нажатие зависит от состояния питания."""

    def test_screen_off_when_not_awake_raises_and_presses_nothing(self) -> None:
        for state in ("Dozing", "Asleep"):
            adb = FakeAdb(wakefulness=state)
            with self.assertRaisesRegex(smoke_driver.DriverError, f"mWakefulness={state}"):
                make_actions(adb).screen_off()
            self.assertEqual(adb.keycodes(), [], f"{state}: нажатие включило бы экран")
            self.assertEqual(adb.wakefulness, state)

    def test_screen_off_with_ignored_power_key_fails_after_the_bounded_wait(self) -> None:
        adb = FakeAdb(power_key_works=False)
        clock = FakeClock()
        with self.assertRaisesRegex(smoke_driver.DriverError, "не стало не Awake"):
            make_actions(adb, clock).screen_off()
        self.assertEqual(adb.keycodes(), ["KEYCODE_POWER"], "POWER нажат один раз, повторов нет")
        self.assertGreaterEqual(clock.now, smoke_driver.WAKEFULNESS_TIMEOUT_SECONDS)
        self.assertLess(clock.now, smoke_driver.WAKEFULNESS_TIMEOUT_SECONDS + 1.0, "ожидание ограничено")

    def test_screen_on_is_idempotent_and_waits_for_awake(self) -> None:
        adb = FakeAdb(wakefulness="Awake")
        make_actions(adb).screen_on()
        self.assertEqual(adb.keycodes(), ["KEYCODE_WAKEUP"])
        self.assertEqual(adb.wakefulness, "Awake")

    def test_screen_on_that_never_wakes_fails(self) -> None:
        adb = FakeAdb(wakefulness="Dozing", power_key_works=False)
        with self.assertRaisesRegex(smoke_driver.DriverError, "не стало Awake.*Dozing"):
            make_actions(adb).screen_on()

    def test_unparseable_power_state_is_an_error_not_a_guess(self) -> None:
        class SilentAdb(FakeAdb):
            def __call__(self, command: list[str]) -> smoke_driver.AdbResult:
                if command[-2:] == ["dumpsys", "power"]:
                    self.commands.append(command)
                    return smoke_driver.AdbResult(0, "nothing useful", "")
                return super().__call__(command)

        adb = SilentAdb()
        with self.assertRaisesRegex(smoke_driver.DriverError, "mWakefulness"):
            make_actions(adb).screen_off()
        self.assertEqual(adb.keycodes(), [], "не зная состояния, POWER не жмём")

    def test_parse_wakefulness(self) -> None:
        self.assertEqual(smoke_driver.parse_wakefulness("x\n  mWakefulness=Dozing\n y"), "Dozing")
        self.assertEqual(smoke_driver.parse_wakefulness("mWakefulness=Asleep"), "Asleep")
        with self.assertRaises(smoke_driver.DriverError):
            smoke_driver.parse_wakefulness("")


class WmSizeAdb(FakeAdb):
    """Фейковый adb, который на ``wm size`` отдаёт заготовленный вывод."""

    def __init__(self, wm_size_output: str) -> None:
        super().__init__()
        self._wm_size_output = wm_size_output

    def __call__(self, command: list[str]) -> smoke_driver.AdbResult:
        result = super().__call__(command)
        if command == [*ADB, "shell", "wm", "size"]:
            return smoke_driver.AdbResult(0, self._wm_size_output, "")
        return result


SLOWER = {"observed_slower_than_expected": True}
NOT_SLOWER = {"observed_slower_than_expected": False}


class RenderTapStepTest(unittest.TestCase):
    """Стадия ``awaiting_refresh`` сценария ``render`` касается центра экрана, только если экран медленнее цели."""

    @staticmethod
    def awaiting(**fields: object) -> dict:
        return {"scenario": "render", "stage": "awaiting_refresh", **fields}

    def test_slower_display_taps_screen_center_from_wm_size(self) -> None:
        adb = WmSizeAdb("Physical size: 1440x3216\n")
        driver = smoke_driver.StageDriver("render", make_actions(adb))
        driver.handle_stage(self.awaiting(**SLOWER))
        self.assertEqual(
            adb.commands,
            [[*ADB, "shell", "wm", "size"], [*ADB, "shell", "input", "tap", "720", "1608"]],
        )

    def test_display_not_slower_or_flag_missing_does_not_tap(self) -> None:
        for fields in (NOT_SLOWER, {}, {"observed_slower_than_expected": "true"}):
            adb = WmSizeAdb("Physical size: 1440x3216\n")
            smoke_driver.StageDriver("render", make_actions(adb)).handle_stage(self.awaiting(**fields))
            self.assertEqual(adb.commands, [], fields)

    def test_repeated_stage_taps_every_time_it_is_slower(self) -> None:
        adb = WmSizeAdb("Physical size: 1440x3216\n")
        driver = smoke_driver.StageDriver("render", make_actions(adb))
        lines = [
            stage_line("render", "awaiting_refresh", attempt=1, **SLOWER),
            stage_line("render", "awaiting_refresh", attempt=2, **SLOWER),
            stage_line("render", "awaiting_refresh", attempt=3, **NOT_SLOWER),
            stage_line("render", "refresh_settled"),
            result_line(render_payload()),
        ]
        verdict = smoke_wait.evaluate_stream(lines, "render", driver.handle_stage)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        taps = [command for command in adb.commands if command[-3:-1] == ["input", "tap"] or "tap" in command]
        self.assertEqual(len(taps), 2, adb.commands)

    def test_override_size_wins_over_physical(self) -> None:
        adb = WmSizeAdb("Physical size: 1440x3216\nOverride size: 1080x2412\n")
        make_actions(adb).tap_screen()
        self.assertEqual(adb.commands[-1], [*ADB, "shell", "input", "tap", "540", "1206"])

    def test_unparsable_wm_size_is_error_without_tap(self) -> None:
        adb = WmSizeAdb("что-то не то\n")
        with self.assertRaisesRegex(smoke_driver.DriverError, "wm size"):
            make_actions(adb).tap_screen()
        self.assertFalse(any("tap" in command for command in adb.commands))

    def test_wm_size_failure_is_driver_error(self) -> None:
        adb = FakeAdb(failing_prefix=[*ADB, "shell", "wm", "size"])
        with self.assertRaisesRegex(smoke_driver.DriverError, "device offline"):
            make_actions(adb).tap_screen()

    def test_other_render_stages_and_lifecycle_stages_do_not_tap(self) -> None:
        for scenario, stage in [("render", "refresh_settled"), ("render", "presenting"), ("render-lifecycle", "awaiting_refresh")]:
            adb = WmSizeAdb("Physical size: 1440x3216\n")
            smoke_driver.StageDriver(scenario, make_actions(adb)).handle_stage(
                {"scenario": scenario, "stage": stage, **SLOWER}
            )
            self.assertEqual(adb.commands, [], (scenario, stage))


class RenderCheckTest(unittest.TestCase):
    def test_good_report_passes(self) -> None:
        self.assertEqual(smoke_checks.check_render(render_payload()), [])

    def test_known_deviations_are_informative_but_must_be_well_formed(self) -> None:
        good = render_payload(validation={**ENABLED, "known_deviations": {"VUID-x": 5, "VUID-y": 0}})
        self.assertEqual(smoke_checks.check_render(good), [], "срабатывания известных отходов не провал")
        for broken in ({"VUID-x": -1}, {"VUID-x": "много"}, ["VUID-x"]):
            payload = render_payload(validation={**ENABLED, "known_deviations": broken})
            self.assertTrue(smoke_checks.check_render(payload), broken)

    def test_failed_status_reports_error(self) -> None:
        problems = smoke_checks.check_render({"scenario": "render", "status": "failed", "error": "пропуски кадров"})
        self.assertIn("пропуски кадров", problems[0])

    def test_miss_ratio_above_limit_fails(self) -> None:
        payload = with_nested(render_payload(), ["pacing", "miss_ratio"], 0.02)
        self.assertTrue(any("доля пропусков" in text for text in smoke_checks.check_render(payload)))

    def test_period_outside_tolerance_or_unknown_fails(self) -> None:
        slow = with_nested(render_payload(), ["refresh", "measured_period_ms"], 16.667)
        self.assertTrue(any("вне допуска" in text for text in smoke_checks.check_render(slow)))
        unknown = with_nested(render_payload(), ["refresh", "measured_period_ms"], None)
        self.assertTrue(any("неизвестен" in text for text in smoke_checks.check_render(unknown)))

    def test_timing_gaps_are_required_and_not_a_failure_by_themselves(self) -> None:
        with_gaps = with_nested(render_payload(), ["pacing", "timing_gaps"], 37)
        self.assertEqual(smoke_checks.check_render(with_gaps), [], "дыры измерения — не пропуски")
        missing = with_nested(render_payload(), ["pacing", "timing_gaps"], None)
        self.assertTrue(any("timing_gaps" in text for text in smoke_checks.check_render(missing)))
        negative = with_nested(render_payload(), ["pacing", "timing_gaps"], -1)
        self.assertTrue(any("timing_gaps" in text for text in smoke_checks.check_render(negative)))
        no_frames = with_nested(render_payload(), ["pacing", "frames_covered"], 0)
        self.assertTrue(any("frames_covered" in text for text in smoke_checks.check_render(no_frames)))

    def test_sparse_pacing_data_fails_even_when_miss_ratio_looks_clean(self) -> None:
        # Окно набрано, пропусков нет, но интервалов 100 из 1000 кадров: остальное — потерянные записи.
        sparse = with_nested(
            with_nested(render_payload(), ["pacing", "intervals"], 100), ["pacing", "frames_covered"], 1000
        )
        problems = smoke_checks.check_render(sparse)
        self.assertTrue(any("разрежены" in text for text in problems), problems)
        # Телефон: 601 из 602 — норма.
        phone_like = with_nested(
            with_nested(render_payload(), ["pacing", "intervals"], 601), ["pacing", "frames_covered"], 602
        )
        self.assertEqual(smoke_checks.check_render(phone_like), [])
        # Порог и счётчик отброшенных меток обязаны быть в отчёте.
        no_threshold = with_nested(render_payload(), ["pacing", "min_coverage_ratio"], None)
        self.assertTrue(any("min_coverage_ratio" in text for text in smoke_checks.check_render(no_threshold)))
        no_rejected = with_nested(render_payload(), ["pacing", "rejected_out_of_order"], None)
        self.assertTrue(any("rejected_out_of_order" in text for text in smoke_checks.check_render(no_rejected)))

    def test_no_frames_no_intervals_and_backend_errors_fail(self) -> None:
        self.assertTrue(any("кадров" in text for text in smoke_checks.check_render(render_payload(frames_presented=0))))
        no_intervals = with_nested(render_payload(), ["pacing", "intervals"], 0)
        self.assertTrue(any("интервалов" in text for text in smoke_checks.check_render(no_intervals)))
        self.assertTrue(any("бэкенда" in text for text in smoke_checks.check_render(render_payload(backend_errors=1))))

    def test_missing_display_fields_fail(self) -> None:
        payload = with_nested(render_payload(), ["display", "color_space"], "")
        self.assertTrue(any("color_space" in text for text in smoke_checks.check_render(payload)))
        self.assertTrue(any("report.display" in text for text in smoke_checks.check_render(render_payload(display=None))))

    def test_validation_errors_fail_only_when_layer_enabled(self) -> None:
        enabled = with_nested(render_payload(), ["validation", "errors"], 2)
        self.assertTrue(any("validation" in text for text in smoke_checks.check_render(enabled)))
        release = render_payload(validation={"mode": "disabled", "errors": 0, "warnings": 0})
        self.assertEqual(smoke_checks.check_render(release), [], "в release слой выключен и это не провал")
        broken = render_payload(validation={"mode": "maybe", "errors": 0, "warnings": 0})
        self.assertTrue(any("validation.mode" in text for text in smoke_checks.check_render(broken)))


class RenderLifecycleCheckTest(unittest.TestCase):
    def test_good_report_passes(self) -> None:
        self.assertEqual(smoke_checks.check_render_lifecycle(lifecycle_payload()), [])

    def test_object_growth_fails_with_cycle_number(self) -> None:
        payload = copy.deepcopy(lifecycle_payload())
        payload["report"]["cycles"][1]["idle_live_objects"]["Fence"] = 4
        problems = smoke_checks.check_render_lifecycle(payload)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("цикле 2", problems[0])
        self.assertIn("Fence", problems[0])

    def test_fluctuating_presenting_counts_are_informative_only(self) -> None:
        payload = copy.deepcopy(lifecycle_payload())
        payload["report"]["cycles"][0]["presenting_live_objects"] = 99
        self.assertEqual(smoke_checks.check_render_lifecycle(payload), [])

    def test_unmeasured_idle_census_and_missing_baseline_fail(self) -> None:
        payload = copy.deepcopy(lifecycle_payload())
        payload["report"]["cycles"][2]["idle_live_objects"] = None
        self.assertTrue(any("не замерены" in text for text in smoke_checks.check_render_lifecycle(payload)))
        self.assertTrue(
            any(
                "idle_baseline_live_objects" in text
                for text in smoke_checks.check_render_lifecycle(lifecycle_payload(idle_baseline_live_objects=None))
            )
        )

    def test_release_count_and_cycle_count_must_match_plan(self) -> None:
        self.assertTrue(
            any("освобождений" in text for text in smoke_checks.check_render_lifecycle(lifecycle_payload(releases_confirmed=2)))
        )
        short = copy.deepcopy(lifecycle_payload())
        short["report"]["cycles"].pop()
        self.assertTrue(any("2 из 3" in text for text in smoke_checks.check_render_lifecycle(short)))

    def test_shell_and_backend_counters_fail(self) -> None:
        for field, value, fragment in [
            ("surface_release_timeouts", 1, "surface_release_timeouts"),
            ("activity_recreated", True, "activity_recreated"),
            ("fault", "ClockStopped: x", "неисправен"),
            ("backend_errors", 1, "бэкенда"),
            ("detach_failures", 1, "detach_failures"),
        ]:
            problems = smoke_checks.check_render_lifecycle(lifecycle_payload(**{field: value}))
            self.assertTrue(any(fragment in text for text in problems), f"{field}: {problems}")

    def test_validation_follows_mode(self) -> None:
        enabled = lifecycle_payload(validation={"mode": "enabled", "errors": 1, "warnings": 0})
        self.assertTrue(any("validation" in text for text in smoke_checks.check_render_lifecycle(enabled)))
        release = lifecycle_payload(validation={"mode": "disabled", "errors": 0, "warnings": 0})
        self.assertEqual(smoke_checks.check_render_lifecycle(release), [])


class AhbImportCheckTest(unittest.TestCase):
    def test_both_matched_passes(self) -> None:
        self.assertEqual(smoke_checks.check_ahb_import(ahb_payload()), [])

    def test_raw10_unsupported_passes_but_raw16_unsupported_fails(self) -> None:
        self.assertEqual(smoke_checks.check_ahb_import(ahb_payload(raw10=ahb_entry("unsupported"))), [])
        problems = smoke_checks.check_ahb_import(ahb_payload(raw16=ahb_entry("unsupported")))
        self.assertTrue(any("raw16" in text for text in problems), problems)

    def test_mismatch_and_error_outcomes_fail_for_both_formats(self) -> None:
        for outcome in ("mismatch", "error", "matched_with_validation_errors"):
            self.assertTrue(smoke_checks.check_ahb_import(ahb_payload(raw10=ahb_entry(outcome))), outcome)
            self.assertTrue(smoke_checks.check_ahb_import(ahb_payload(raw16=ahb_entry(outcome))), outcome)

    def test_matched_with_validation_errors_fails_and_prints_known_deviations(self) -> None:
        entry = ahb_entry("matched_with_validation_errors", validation_errors=2, known_deviations={"VUID-x": 3})
        problems = smoke_checks.check_ahb_import(ahb_payload(raw16=entry))
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("VUID-x", problems[0])
        self.assertIn("raw16", problems[0])
        # Для raw10 исход тоже провальный (unsupported там допустим, а этот — нет).
        self.assertTrue(smoke_checks.check_ahb_import(ahb_payload(raw10=entry)))

    def test_matched_with_broken_summary_fails(self) -> None:
        for field in ("matched", "second_import_hit", "live_objects_restored"):
            payload = ahb_payload(raw16=ahb_entry(**{field: False}))
            self.assertTrue(any(field in text for text in smoke_checks.check_ahb_import(payload)), field)

    def test_validation_follows_mode(self) -> None:
        enabled = ahb_payload(raw10=ahb_entry(validation_errors=1))
        self.assertTrue(any("validation" in text for text in smoke_checks.check_ahb_import(enabled)))
        release = ahb_payload(raw10=ahb_entry(validation="disabled", validation_errors=0))
        self.assertEqual(smoke_checks.check_ahb_import(release), [])


class GpuFenceCheckTest(unittest.TestCase):
    def test_good_report_passes(self) -> None:
        self.assertEqual(smoke_checks.check_gpu_fence(fence_payload()), [])

    def test_each_ordering_rule_fails(self) -> None:
        cases = [
            ({"work_b_value": 0xDEADBEEF}, "не равны"),
            ({"b_started_after_a_finished": False}, "b_started_after_a_finished"),
            ({"release_fence_pending_at_export": False}, "release_fence_pending_at_export"),
            ({"release_fence_pending_at_export": None}, "release_fence_pending_at_export"),
            ({"already_signaled_wait_semaphore_created": True}, "лишний"),
            ({"already_signaled_work_completed": False}, "не завершилась"),
            ({"already_signaled_result_value": 0}, "итог"),
            ({"validation_errors": 1}, "validation"),
        ]
        for overrides, fragment in cases:
            problems = smoke_checks.check_gpu_fence(fence_payload(**overrides))
            self.assertTrue(any(fragment in text for text in problems), f"{overrides}: {problems}")

    def test_release_build_ignores_validation_counter(self) -> None:
        self.assertEqual(smoke_checks.check_gpu_fence(fence_payload(validation="disabled", validation_errors=0)), [])


class RegistrationTest(unittest.TestCase):
    def test_all_gpu_scenarios_have_expectations_and_are_judged_end_to_end(self) -> None:
        payloads = {
            "render": render_payload(),
            "render-lifecycle": lifecycle_payload(),
            "ahb-import": ahb_payload(),
            "gpu-fence": fence_payload(),
        }
        for name, payload in payloads.items():
            self.assertIn(name, smoke_wait.CHECKS)
            verdict = smoke_wait.evaluate_stream([result_line(payload)], name)
            self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, f"{name}: {verdict.message}")
            broken = dict(payload, status="failed", error="нарушение")
            verdict = smoke_wait.evaluate_stream([result_line(broken)], name)
            self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED, name)

    def test_driver_table_knows_render_lifecycle_stages_only(self) -> None:
        self.assertEqual(
            set(smoke_driver.SCENARIO_STEPS["render-lifecycle"]),
            {"presenting_home", "backgrounded_home", "presenting_screen", "backgrounded_screen"},
        )
        self.assertEqual(set(smoke_driver.SCENARIO_STEPS["render"]), {"awaiting_refresh"}, "render: только условное касание")
        for scenario in ("ahb-import", "gpu-fence"):
            self.assertNotIn(scenario, smoke_driver.SCENARIO_STEPS)

    def test_result_json_survives_round_trip_through_logcat_line(self) -> None:
        payload = render_payload()
        verdict = smoke_wait.evaluate_stream([result_line(payload)], "render")
        self.assertIn(json.dumps(payload), verdict.message)


if __name__ == "__main__":
    unittest.main()
