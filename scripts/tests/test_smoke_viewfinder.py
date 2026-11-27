"""Тесты smoke-сценариев видоискателя сессии 07: проверки отчётов (``smoke_checks_viewfinder``) через
сквозной ``evaluate_stream``, шаги драйвера (скриншоты ``viewfinder``, касание ради 120 Гц у стенда V4),
нейтральная карта по скриншотам и ``device-smoke.sh`` с ``--stages`` / ``--neutral-card`` на фейковом adb."""

from __future__ import annotations

import json
import os
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_driver  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import smoke_neutral_card  # noqa: E402
import smoke_screen  # noqa: E402
import smoke_wait  # noqa: E402
import test_smoke_device_script as device_script  # noqa: E402
from test_smoke_screen import parsed, screencap_bytes  # noqa: E402


def stage_line(scenario: str, stage: str, **extra: object) -> str:
    payload = {"scenario": scenario, "stage": stage, **extra}
    return f"  1792957061.123  4242  4242 I Fasticamera: FASTICAMERA_STAGE {json.dumps(payload)}\n"


def result_line(payload: dict) -> str:
    return f"  1792957062.000  4242  4242 I Fasticamera: FASTICAMERA_RESULT {json.dumps(payload)}\n"


def passed(scenario: str, report: dict) -> dict:
    return {"scenario": scenario, "status": "passed", "report": report}


def white_balance_report(
    origin: str = "phone",
    request_state: str = "applied",
    cold_start_pending: bool = False,
    hidden_frames: int = 3,
) -> dict:
    """Раздел ``white_balance`` сводки: белая 3200 К, её происхождение и исход последней заявки источнику."""
    return {
        "white": {"kelvin": 3200.0, "duv": 0.0},
        "origin": origin,
        "request": {"state": request_state, "id": 1, "source": "phone"},
        "cold_start_pending": cold_start_pending,
        "hidden_frames": hidden_frames,
        "max_hidden_frames": 20,
        "hal_delta_mired": 5.0,
    }


def look_report(measurement_look: str = "product", tone_base: str = "agx", fingerprint: str = "97b8e8777d5d7e27") -> dict:
    """Отчёт о виде и основе (14o), как его пишет `look_report.rs`."""
    return {
        "tone_base": tone_base,
        "recipe_fingerprint": fingerprint,
        "layer_neutral": True,
        "wheels_active": False,
        "tone_active": False,
        "measurement_look": measurement_look,
    }


def compact_look(tone_base: str = "agx", fingerprint: str = "97b8e8777d5d7e27") -> dict:
    """Сжатый вид окна строки вида в строке результата (`look_rows_summary`)."""
    return {
        "tone_base": tone_base,
        "recipe_fingerprint": fingerprint,
        "wheels_active": False,
        "tone_active": False,
    }


def stage_budget_look_fields(look_rows: list[str] | None = None) -> dict:
    """Поля 14o в отчёте `viewfinder-stage-budget`: вид на входе, число построений размытия и виды окон строк вида."""
    fields: dict = {"look": look_report(), "backdrop_pyramid_rebuilds": 0}
    if look_rows:
        fields["look_rows"] = [
            {
                "row": row,
                "on": compact_look(fingerprint="1111111111111111"),
                "off": compact_look(fingerprint="2222222222222222"),
            }
            for row in look_rows
        ]
    return fields


def viewfinder_report(**overrides: object) -> dict:
    report: dict = {
        "sensor_fps": 59.95,
        "isp_fps": 59.9,
        "cpu_copies": 0,
        "gpu_time": {"isp_dispatch": {"samples": 600, "p50_ms": 1.8, "p95_ms": 2.3, "max_ms": 3.0}},
        "look": {"painter": "isp", "all_stages": True, "display_color_space": "display_p3", "picture": look_report()},
        # Прореживание статистики: 3600 обработанных кадров, каждый 4-й (900); 3 кадра скрыты на холодном старте.
        # Потребитель статистики — наш AE: расписание в разделе frame_statistics (``None`` — AE нет, публикаций нет).
        "statistics_published": 900,
        "frame_statistics": {"every_n_frames": 4},
        "frames_processed": 3600,
        "frames_hidden_cold_start": 3,
        "frames_presented": 3597,
        "bracket_frames_skipped": 0,
        # Белая взята у телефона, заявка открытия закрыта исходом applied (старый оценщик продукт не судит).
        "white_balance": {
            "white": {"kelvin": 3200.0, "duv": 0.0},
            "origin": "phone",
            "request": {"state": "applied", "id": 1, "source": "phone"},
            "cold_start_pending": False,
            "hidden_frames": 3,
            "max_hidden_frames": 20,
            "hal_delta_mired": 5.0,
        },
        "three_a_companion": {"state": "active", "width": 640, "height": 480},
        "latency": {"samples": 3000, "p50_ms": 41.0, "p95_ms": 47.0},
        "validation": {"mode": "enabled", "errors": 0},
        "window_ms": 60000,
        "pacing": pacing_report(),
        "state_at_end": "streaming",
    }
    report.update(overrides)
    return report


def pacing_report(**overrides: object) -> dict:
    """Минута при 60,03 Гц камеры против 60,00 Гц экрана: два переполнения буфера (дрейф), потерь больше нет."""
    pacing: dict = {
        "frames_published": 3602,
        "delivered": 3600,
        "dropped": 2,
        "superseded": 0,
        "jitter_overflow": 2,
        "repeats": 0,
        "shown_once_ratio": 0.9994,
    }
    pacing.update(overrides)
    return pacing


def profile(name: str, **overrides: object) -> dict:
    knobs: dict = {"name": name, "viewfinder_pacing": "jitter_buffer", "depth_reduction": "skip_one_vsync"}
    knobs.update(overrides)
    return knobs


def profile_summary(**overrides: object) -> dict:
    summary: dict = {
        "windows": 3,
        "latency_p50_ms": 41.0,
        "processed_fps": 59.0,
        "dropped_per_s": 0.03,
        "repeats_per_s": 0.0,
        "shown_once_ratio": 0.9994,
    }
    summary.update(overrides)
    return summary


def verdict_of(scenario: str, report: dict, stages: list[str] | None = None) -> smoke_wait.SmokeVerdict:
    return smoke_wait.evaluate_stream([*(stages or []), result_line(passed(scenario, report))], scenario)


class ViewfinderCheckTest(unittest.TestCase):
    def test_healthy_viewfinder_passes(self) -> None:
        self.assertEqual(verdict_of("viewfinder", viewfinder_report()).exit_code, smoke_wait.EXIT_OK)

    def test_srgb_is_an_honest_answer_but_unknown_space_fails(self) -> None:
        srgb = viewfinder_report(look={"painter": "isp", "display_color_space": "srgb", "picture": look_report()})
        self.assertEqual(verdict_of("viewfinder", srgb).exit_code, smoke_wait.EXIT_OK)
        unknown = viewfinder_report(look={"painter": "isp", "display_color_space": "unknown", "picture": look_report()})
        verdict = verdict_of("viewfinder", unknown)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("пространство экрана", verdict.message)

    def test_extra_statistics_of_the_ae_cold_start_are_not_a_failure(self) -> None:
        # Наш AE на холодном старте считает статистику на каждом кадре (до 30 кадров) и публикует её в ленту:
        # публикаций больше, чем frames_processed / period. Проверка нижняя, верхней границы нет.
        for published in (900 + 30, 3600):
            with self.subTest(published=published):
                verdict = verdict_of("viewfinder", viewfinder_report(statistics_published=published))
                self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)

    def test_without_our_ae_zero_publications_are_not_a_failure(self) -> None:
        # Вендорский AE (или AE выключен): потребителей статистики нет, рендер её не считает. Проверка не применяется.
        report = viewfinder_report(statistics_published=0, frame_statistics=None)
        verdict = verdict_of("viewfinder", report)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)

    def test_the_statistics_section_is_required_even_without_a_consumer(self) -> None:
        report = {k: v for k, v in viewfinder_report().items() if k != "frame_statistics"}
        verdict = verdict_of("viewfinder", report)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("frame_statistics", verdict.message)

    def test_each_rule_is_rechecked(self) -> None:
        cases = {
            "ISP 40": viewfinder_report(isp_fps=40.0),
            "cpu_copies": viewfinder_report(cpu_copies=1),
            "прохода ISP": viewfinder_report(gpu_time={}),
            "рисовальщик": viewfinder_report(look={"painter": "debug_mosaic", "display_color_space": "srgb"}),
            "статистика кадра не публиковалась": viewfinder_report(statistics_published=0),
            "опубликована 100 раз": viewfinder_report(statistics_published=100),
            "раздела white_balance": {k: v for k, v in viewfinder_report().items() if k != "white_balance"},
            "белая не взята у телефона": viewfinder_report(
                white_balance=white_balance_report(origin="fallback", request_state="unavailable")
            ),
            "origin='manual'": viewfinder_report(white_balance=white_balance_report(origin="manual")),
            "заявка белого не закрыта исходом applied": viewfinder_report(
                white_balance=white_balance_report(request_state="timed_out")
            ),
            "холодный старт скрыл 21 кадров, предел 20": viewfinder_report(
                white_balance=white_balance_report(hidden_frames=21)
            ),
            "нет числа скрытых кадров": viewfinder_report(
                white_balance={k: v for k, v in white_balance_report().items() if k != "hidden_frames"}
            ),
            "холодный старт не кончился": viewfinder_report(
                white_balance=white_balance_report(cold_start_pending=True)
            ),
            "кадры теряются": viewfinder_report(frames_presented=3000),
            "нет счётчиков учёта": viewfinder_report(frames_hidden_cold_start=None),
            "вспомогательный поток 3A": viewfinder_report(
                three_a_companion={"state": "unavailable", "reason": {"kind": "rejected_by_hal"}}
            ),
            "задержка": viewfinder_report(latency={"samples": 0}),
            "validation": viewfinder_report(validation={"mode": "enabled", "errors": 2}),
            "состояние камеры": viewfinder_report(state_at_end="error"),
            "раздела pacing": {k: v for k, v in viewfinder_report().items() if k != "pacing"},
            "pacing.dropped=5": viewfinder_report(pacing=pacing_report(dropped=5)),
            "pacing.jitter_overflow=": viewfinder_report(pacing=pacing_report(jitter_overflow="много")),
            "раз в минуту": viewfinder_report(pacing=pacing_report(dropped=30, jitter_overflow=30)),
            "ровно один раз": viewfinder_report(pacing=pacing_report(shown_once_ratio=0.5)),
            "shown_once_ratio=None": viewfinder_report(pacing=pacing_report(shown_once_ratio=None)),
        }
        for needle, report in cases.items():
            with self.subTest(needle):
                verdict = verdict_of("viewfinder", report)
                self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED, verdict.message)
                self.assertIn(needle, verdict.message)

    def test_hidden_cold_start_frames_are_accounted_not_counted_as_lost(self) -> None:
        # 3 скрытых кадра: показано на 3 меньше обработанных — это норма, а не потеря.
        report = viewfinder_report(frames_presented=3597, frames_hidden_cold_start=3)
        self.assertEqual(verdict_of("viewfinder", report).exit_code, smoke_wait.EXIT_OK)

    def test_drift_level_overflow_and_superseded_are_reported_apart_and_pass(self) -> None:
        pacing = pacing_report(dropped=8, superseded=4, jitter_overflow=4, shown_once_ratio=0.9978)
        self.assertEqual(verdict_of("viewfinder", viewfinder_report(pacing=pacing)).exit_code, smoke_wait.EXIT_OK)

    def test_release_validation_errors_do_not_count(self) -> None:
        report = viewfinder_report(validation={"mode": "disabled", "errors": 0})
        self.assertEqual(verdict_of("viewfinder", report).exit_code, smoke_wait.EXIT_OK)

    def test_rust_failure_is_a_failure(self) -> None:
        verdict = smoke_wait.evaluate_stream(
            [result_line({"scenario": "viewfinder", "status": "failed", "error": "ISP пропускает кадры"})], "viewfinder"
        )
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("ISP пропускает кадры", verdict.message)


class LatencyBudgetEnduranceHeadroomCheckTest(unittest.TestCase):
    def test_latency_needs_all_windows_two_profiles_and_a_verdict(self) -> None:
        summary = profile_summary()
        good = {
            "windows": 6,
            "expected_windows": 6,
            "profiles": [profile("baseline", viewfinder_pacing="latest_wins", depth_reduction="disabled"), profile("tuned")],
            "comparison": {"baseline": summary, "tuned": summary, "verdict": {"verdict": "no_clear_difference"}},
        }
        self.assertEqual(verdict_of("viewfinder-latency", good).exit_code, smoke_wait.EXIT_OK)
        short = {**good, "windows": 4}
        self.assertIn("4", verdict_of("viewfinder-latency", short).message)
        no_verdict = {**good, "comparison": {**good["comparison"], "verdict": {}}}
        self.assertEqual(verdict_of("viewfinder-latency", no_verdict).exit_code, smoke_wait.EXIT_SCENARIO_FAILED)

    def test_latency_outcome_of_the_ab_is_data_not_a_failure(self) -> None:
        summary = profile_summary()
        for outcome in ("tuned_better", "tuned_worse", "mixed", "not_comparable"):
            with self.subTest(outcome):
                report = {
                    "windows": 6,
                    "expected_windows": 6,
                    "profiles": [profile("a"), profile("b")],
                    "comparison": {"baseline": summary, "tuned": summary, "verdict": {"verdict": outcome}},
                }
                self.assertEqual(verdict_of("viewfinder-latency", report).exit_code, smoke_wait.EXIT_OK)

    def test_latency_rejects_same_names_and_empty_profile_summary(self) -> None:
        summary = profile_summary()
        base = {
            "windows": 6,
            "expected_windows": 6,
            "profiles": [profile("x"), profile("x")],
            "comparison": {"baseline": summary, "tuned": summary, "verdict": {"verdict": "mixed"}},
        }
        self.assertIn("разными именами", verdict_of("viewfinder-latency", base).message)
        empty = {**base, "profiles": [profile("a"), profile("b")]}
        empty["comparison"] = {**base["comparison"], "tuned": {"windows": 0}}
        message = verdict_of("viewfinder-latency", empty).message
        self.assertIn("comparison.tuned.windows", message)
        self.assertIn("comparison.tuned.latency_p50_ms", message)

    def test_latency_profiles_must_carry_valid_pacing_and_depth_reduction_and_pacing_numbers(self) -> None:
        good = {
            "windows": 6,
            "expected_windows": 6,
            "profiles": [profile("a"), profile("b")],
            "comparison": {
                "baseline": profile_summary(),
                "tuned": profile_summary(),
                "verdict": {"verdict": "mixed"},
            },
        }
        self.assertEqual(verdict_of("viewfinder-latency", good).exit_code, smoke_wait.EXIT_OK)
        cases = {
            "profiles[1].viewfinder_pacing": {**good, "profiles": [profile("a"), profile("b", viewfinder_pacing="triple")]},
            "profiles[0].depth_reduction": {**good, "profiles": [profile("a", depth_reduction=None), profile("b")]},
            "comparison.tuned.shown_once_ratio": {
                **good,
                "comparison": {**good["comparison"], "tuned": profile_summary(shown_once_ratio=None)},
            },
            "comparison.baseline.repeats_per_s": {
                **good,
                "comparison": {**good["comparison"], "baseline": {"windows": 3, "latency_p50_ms": 41.0}},
            },
        }
        for needle, report in cases.items():
            with self.subTest(needle):
                verdict = verdict_of("viewfinder-latency", report)
                self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED, verdict.message)
                self.assertIn(needle, verdict.message)

    def test_viewfinder_adpf_rules(self) -> None:
        ok = viewfinder_report(adpf={"mode": "enabled", "session": "active", "reports_sent": 3000})
        self.assertEqual(verdict_of("viewfinder", ok).exit_code, smoke_wait.EXIT_OK)
        unsupported = viewfinder_report(adpf={"mode": "enabled", "session": "unsupported"})
        self.assertEqual(verdict_of("viewfinder", unsupported).exit_code, smoke_wait.EXIT_OK)
        failed = viewfinder_report(adpf={"mode": "enabled", "session": "failed", "last_error": "нет сервиса"})
        verdict = verdict_of("viewfinder", failed)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("ADPF включён", verdict.message)
        unknown = viewfinder_report(adpf={"mode": "enabled", "session": "weird"})
        self.assertIn("состояние сессии ADPF", verdict_of("viewfinder", unknown).message)

    def test_stage_budget_table_has_a_row_per_stage_and_the_baseline(self) -> None:
        good = {
            **stage_budget_look_fields(),
            "surface": "bare_viewfinder",
            "requested_rows": ["tone", "whole_viewfinder"],
            "table": [
                "tone | 0.10 / 0.12 | 8.0 ± 2.0 | measured",
                "whole_viewfinder | 1.70 / 1.90 | 60.0 ± 5.0 | measured",
                "camera_without_isp | 0.70 / 1.00 | 480.0 (абс.) | baseline",
            ],
        }
        self.assertEqual(verdict_of("viewfinder-stage-budget", good).exit_code, smoke_wait.EXIT_OK)
        missing_baseline = {**good, "table": good["table"][:2]}
        self.assertIn("камера без ISP", verdict_of("viewfinder-stage-budget", missing_baseline).message)
        broken = {**good, "table": ["tone | 0.10", *good["table"][1:]]}
        self.assertIn("4 столбцов", verdict_of("viewfinder-stage-budget", broken).message)
        fewer = {**good, "requested_rows": ["levels", "tone", "whole_viewfinder"]}
        self.assertIn("запрошено 3", verdict_of("viewfinder-stage-budget", fewer).message)

    def test_endurance_records_throttling_either_way(self) -> None:
        calm = {"throttling": {"found": False, "signs": []}, "endurance": {"totals": {}}, "state_at_end": "streaming"}
        # Пустые итоги — тоже «нет итогов».
        self.assertIn("итогов", verdict_of("viewfinder-endurance", calm).message)
        calm["endurance"] = {"totals": {"measured_minutes": 10}}
        self.assertEqual(verdict_of("viewfinder-endurance", calm).exit_code, smoke_wait.EXIT_OK)
        found = {**calm, "throttling": {"found": True, "signs": [{"sign": "sensor_fps_drop", "minute": 7}]}}
        self.assertEqual(verdict_of("viewfinder-endurance", found).exit_code, smoke_wait.EXIT_OK)
        unexplained = {**calm, "throttling": {"found": True, "signs": []}}
        self.assertIn("признаки", verdict_of("viewfinder-endurance", unexplained).message)

    def test_headroom_bench_needs_frames_timing_and_an_honest_refresh(self) -> None:
        good = {
            "bench": {
                "frames_recorded": 36_000,
                "total": {"p95_ms": 14.2},
                "passes": [{"pass": "temporal_denoise", "gpu_time": {"p95_ms": 4.0}}],
            },
            "display": {"refresh_hz": 60.0},
            "fits_frame_budget": True,
        }
        self.assertEqual(verdict_of("video-headroom-bench", good).exit_code, smoke_wait.EXIT_OK)
        no_refresh_key = {**good, "display": {}}
        self.assertIn("частоты экрана", verdict_of("video-headroom-bench", no_refresh_key).message)
        idle = {**good, "bench": {**good["bench"], "frames_recorded": 0}}
        self.assertIn("не записал", verdict_of("video-headroom-bench", idle).message)


class RecordingActions:
    """Фейк ``DeviceActions``: только записывает вызовы."""

    def __init__(self) -> None:
        self.calls: list[str] = []

    def __getattr__(self, name: str):
        return lambda *arguments: self.calls.append(name if not arguments else f"{name}:{arguments[0]}")


class DriverStepsTest(unittest.TestCase):
    def test_viewfinder_takes_two_shots_on_viewfinder_live(self) -> None:
        actions = RecordingActions()
        driver = smoke_driver.StageDriver("viewfinder", actions)
        driver.handle_stage({"scenario": "viewfinder", "stage": "viewfinder_live"})
        self.assertEqual(actions.calls, ["screencap:viewfinder_a", "pause:1.5", "screencap:viewfinder_b"])

    def test_headroom_bench_taps_once_only_when_asked(self) -> None:
        actions = RecordingActions()
        driver = smoke_driver.StageDriver("video-headroom-bench", actions)
        driver.handle_stage({"scenario": "video-headroom-bench", "stage": "bench_started", "tap_for_refresh": True})
        driver.handle_stage({"scenario": "video-headroom-bench", "stage": "minute"})
        self.assertEqual(actions.calls, ["tap_screen"])
        quiet = RecordingActions()
        smoke_driver.StageDriver("video-headroom-bench", quiet).handle_stage(
            {"scenario": "video-headroom-bench", "stage": "bench_started", "tap_for_refresh": False}
        )
        self.assertEqual(quiet.calls, [])

    def test_viewfinder_is_a_screenshot_scenario(self) -> None:
        self.assertIn("viewfinder", smoke_wait.SCREENSHOT_SCENARIOS)


THRESHOLDS = smoke_neutral_card.NeutralCardThresholds(
    region_fraction=0.2, max_relative_chroma=0.08, min_mean_level=20.0,
    display_middle_gray=0.20, max_middle_gray_error_ev=0.2, p3_luminance_coefficients=(0.2290, 0.6917, 0.0793),
    flatness=smoke_neutral_card.FlatnessThresholds(
        screen_width_pt=440.0, frame_top_pt=186.0, frame_bottom_pt=772.7,
        corner_inset_fraction=0.05, corner_region_fraction=0.08, max_corner_ev=0.25, max_mean_ev=0.15,
    ),
)


def card(rgb: tuple[int, int, int]):
    """Экран: серая «карта» ``rgb`` в центре, рядом с центром — насыщенно-красная полоса (в область центра не
    входит); углы кадра и всё вне полосы — цвета карты, чтобы проверка ровности поля не мешала этим тестам."""
    return lambda x, y: (200, 30, 30) if 30 <= y < 64 and not 15 <= x < 25 else rgb


class NeutralCardTest(unittest.TestCase):
    def test_gray_center_passes_and_reports_numbers(self) -> None:
        analysis = smoke_neutral_card.analyze_neutral_card(parsed(card((120, 122, 118))), THRESHOLDS, "viewfinder_a")
        self.assertEqual(analysis.problems, [])
        self.assertIn("R=120.0 G=122.0 B=118.0", analysis.notes[0])

    def test_tinted_center_fails(self) -> None:
        analysis = smoke_neutral_card.analyze_neutral_card(parsed(card((140, 120, 100))), THRESHOLDS, "viewfinder_a")
        self.assertEqual(len(analysis.problems), 1)
        self.assertIn("не серая", analysis.problems[0])

    def test_dark_center_is_not_judged_for_chroma(self) -> None:
        analysis = smoke_neutral_card.analyze_neutral_card(parsed(card((10, 5, 2))), THRESHOLDS, "viewfinder_a")
        self.assertEqual(len(analysis.problems), 1)
        self.assertIn("тёмный", analysis.problems[0])

    def test_middle_gray_card_passes_and_wrong_levels_fail_with_the_ev_error(self) -> None:
        middle = round(smoke_neutral_card.srgb_linear_to_encoded(0.20) * 255)  # 124: ближайшее к средний серый
        ok = smoke_neutral_card.analyze_neutral_card(parsed(card((middle,) * 3)), THRESHOLDS, "viewfinder_a")
        self.assertEqual(ok.problems, [])
        self.assertIn("ошибка +0.01 EV", ok.notes[0])
        self.assertIn("~123.6/255", ok.notes[0])
        bright = smoke_neutral_card.analyze_neutral_card(parsed(card((160,) * 3)), THRESHOLDS, "viewfinder_a")
        self.assertEqual(len(bright.problems), 1, bright.problems)
        self.assertIn("светлее", bright.problems[0])
        dim = smoke_neutral_card.analyze_neutral_card(parsed(card((90,) * 3)), THRESHOLDS, "viewfinder_a")
        self.assertEqual(len(dim.problems), 1, dim.problems)
        self.assertIn("темнее", dim.problems[0])

    def test_ev_error_follows_the_formula(self) -> None:
        self.assertAlmostEqual(smoke_neutral_card.middle_gray_error_ev(0.40, 0.20), 1.0)
        self.assertAlmostEqual(smoke_neutral_card.middle_gray_error_ev(0.10, 0.20), -1.0)
        white = smoke_neutral_card.display_luminance((255, 255, 255), THRESHOLDS.p3_luminance_coefficients)
        self.assertAlmostEqual(white, 1.0, places=3)

    def test_thresholds_live_in_the_screen_toml(self) -> None:
        loaded = smoke_neutral_card.load_thresholds()
        self.assertGreater(loaded.max_relative_chroma, 0)
        self.assertEqual(loaded.display_middle_gray, 0.20)
        self.assertEqual(loaded.max_middle_gray_error_ev, 0.2)
        with self.assertRaises(smoke_screen.ScreenError):
            smoke_neutral_card.parse_thresholds("[band]\ntop_fraction = 0.3\n")

    def test_post_check_reads_both_shots_from_the_artifacts_dir(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            for label in smoke_screen.VIEWFINDER_LABELS:
                smoke_screen.capture_path(Path(directory), label).write_bytes(
                    screencap_bytes(card((140, 120, 100)), header_with_colorspace=True)
                )
            analysis = smoke_neutral_card.neutral_card_post_check(Path(directory), THRESHOLDS)
        self.assertEqual(len(analysis.problems), 2, analysis.problems)
        missing = smoke_neutral_card.neutral_card_post_check(Path("/nonexistent-fasticamera"), THRESHOLDS)
        self.assertIn("нет скриншота", missing.problems[0])


class DeviceSmokeViewfinderTest(unittest.TestCase):
    """``device-smoke.sh`` на фейковом adb (тот же, что у ``test_smoke_device_script``)."""

    setUp = device_script.DeviceSmokeScenariosTest.setUp
    run_smoke = device_script.DeviceSmokeScenariosTest.run_smoke
    adb_calls = device_script.DeviceSmokeScenariosTest.adb_calls

    BUDGET_OK = passed(
        "viewfinder-stage-budget",
        {
            **stage_budget_look_fields(),
            "surface": "bare_viewfinder",
            "requested_rows": ["levels"],
            "table": ["levels | 0.05 / 0.06 | не различим (разброс 4.0) | indistinguishable"],
        },
    )

    def test_stages_reach_the_intent_as_an_extra(self) -> None:
        completed = self.run_smoke(
            "viewfinder-stage-budget",
            [result_line(self.BUDGET_OK)],
            extra_arguments=("--stages", "levels,whole_viewfinder"),
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        start = next(call for call in self.adb_calls() if "am start" in call)
        self.assertIn("-e scenario viewfinder-stage-budget -e stages levels,whole_viewfinder", start)

    def test_exposure_controller_reaches_the_intent_and_is_validated(self) -> None:
        completed = self.run_smoke(
            "viewfinder-stage-budget",
            [result_line(self.BUDGET_OK)],
            extra_arguments=("--exposure-controller", "vendor"),
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        start = next(call for call in self.adb_calls() if "am start" in call)
        self.assertIn("-e exposure_controller vendor", start)
        for bad in ("auto", "own;ls"):
            with self.subTest(value=bad):
                rejected = self.run_smoke("viewfinder", [], extra_arguments=("--exposure-controller", bad))
                self.assertEqual(rejected.returncode, 64, rejected.stderr)

    def test_smoke_runs_always_turn_the_owners_white_memory_off(self) -> None:
        """Прогон не читает белую владельца и не портит её: intent ВСЕГДА несёт ``white_memory none``, с любыми
        другими флагами и в обоих вариантах приложения."""
        for extra_arguments in ((), ("--variant", "release"), ("--exposure-controller", "vendor")):
            with self.subTest(extra_arguments=extra_arguments):
                self.adb_log.write_text("", encoding="utf-8")
                completed = self.run_smoke(
                    "viewfinder-stage-budget", [result_line(self.BUDGET_OK)], extra_arguments=extra_arguments
                )
                self.assertEqual(completed.returncode, 0, completed.stderr)
                start = next(call for call in self.adb_calls() if "am start" in call)
                self.assertIn("-e white_memory none", start)

    def test_deploy_for_the_owner_does_not_touch_the_white_memory(self) -> None:
        """``device-deploy.sh`` ставит приложение для жизни: память белой работает как в продукте, extra не шлётся."""
        self.logcat_file.write_text("", encoding="utf-8")
        environment = dict(os.environ)
        environment.update(
            PATH=f"{self.fake_bin}{os.pathsep}{environment['PATH']}",
            ANDROID_SERIAL=device_script.SERIAL,
            FASTICAMERA_APK_PATH=str(self.apk),
            FAKE_ADB_LOG=str(self.adb_log),
            FAKE_LOGCAT_FILE=str(self.logcat_file),
            FAKE_DUMP_FILE=str(self.dump_file),
            FAKE_FAILING_PM_COMMAND="",
        )
        completed = subprocess.run(
            ["bash", str(SCRIPTS_DIR / "device-deploy.sh"), "--no-build", "--log-seconds", "1"],
            capture_output=True, text=True, env=environment, timeout=60,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        starts = [call for call in self.adb_calls() if "am start" in call]
        self.assertEqual(len(starts), 1, starts)
        self.assertNotIn("white_memory", starts[0])

    def test_without_stages_no_extra_is_sent(self) -> None:
        completed = self.run_smoke("viewfinder-stage-budget", [result_line(self.BUDGET_OK)])
        self.assertEqual(completed.returncode, 0, completed.stderr)
        start = next(call for call in self.adb_calls() if "am start" in call)
        self.assertNotIn("-e stages", start)
        self.assertNotIn("exposure_controller", start)

    def test_ab_window_reaches_the_intent_as_an_extra_next_to_stages(self) -> None:
        completed = self.run_smoke(
            "viewfinder-stage-budget",
            [result_line(self.BUDGET_OK)],
            extra_arguments=("--stages", "levels", "--ab-window-ms", "15000"),
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        start = next(call for call in self.adb_calls() if "am start" in call)
        self.assertIn("-e stages levels -e ab_window_ms 15000", start)

    def test_endurance_length_reaches_the_intent_as_an_extra(self) -> None:
        completed = self.run_smoke(
            "viewfinder-endurance",
            [],
            extra_arguments=("--endurance-ms", "180000", "--exposure-controller", "own"),
        )
        self.assertNotEqual(completed.returncode, 64, completed.stderr)
        start = next(call for call in self.adb_calls() if "am start" in call)
        self.assertIn("-e endurance_ms 180000", start)

    def test_bad_stages_or_flags_for_the_wrong_scenario_are_usage_errors(self) -> None:
        for arguments, scenario in (
            (("--stages", "levels;rm -rf"), "viewfinder-stage-budget"),
            (("--stages", "levels"), "viewfinder"),
            (("--neutral-card",), "raw-stream"),
            (("--ab-window-ms", "15s"), "viewfinder-stage-budget"),
            (("--ab-window-ms", "0"), "viewfinder-stage-budget"),
            (("--ab-window-ms", "15000"), "viewfinder"),
            (("--endurance-ms", "3min"), "viewfinder-endurance"),
            (("--endurance-ms", "180000"), "viewfinder"),
        ):
            with self.subTest(arguments=arguments, scenario=scenario):
                completed = self.run_smoke(scenario, [], extra_arguments=arguments)
                self.assertEqual(completed.returncode, 64, completed.stderr)

    def test_neutral_card_flag_judges_the_center_of_both_shots(self) -> None:
        # Фейковый screencap: тусклый тёплый кадр 8×10 — центр «не серый», картинка не меняется.
        width, height = 8, 10
        pixels = bytes((150, 120, 90, 255)) * (width * height)
        self.dump_file.write_bytes(struct.pack("<IIII", width, height, 1, 0) + pixels)
        artifacts = self.work_dir / "shots"
        completed = self.run_smoke(
            "viewfinder",
            [stage_line("viewfinder", "viewfinder_live"), result_line(passed("viewfinder", viewfinder_report()))],
            extra_arguments=("--artifacts-dir", str(artifacts), "--neutral-card"),
        )
        self.assertEqual(completed.returncode, 1, completed.stderr)
        self.assertIn("карта не серая", completed.stderr)
        self.assertEqual(len([call for call in self.adb_calls() if call == "exec-out screencap"]), 2)


if __name__ == "__main__":
    unittest.main()
