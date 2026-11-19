"""Тесты smoke-сценария ``ae-trace`` (сессия 08c): проверки сводки (``smoke_checks_ae_trace``) через сквозной
``evaluate_stream``, человекочитаемая таблица и диспетчеризация в ``smoke_wait``."""

from __future__ import annotations

import contextlib
import copy
import io
import json
import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_checks_ae_trace  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import smoke_wait  # noqa: E402

STEP_COLUMNS = smoke_checks_ae_trace.STEP_COLUMNS


def result_line(payload: dict) -> str:
    return f"  1792957062.000  4242  4242 I Fasticamera: FASTICAMERA_RESULT {json.dumps(payload)}\n"


def measured_row(cue: int, step_ev: float = 1.0, **overrides: object) -> list:
    """Строка настоящей ступеньки в порядке ``step_cols`` (форма — ``ae_trace_report.rs``)."""
    row = {
        "cue": cue,
        "kind": "measured",
        "step_ev": step_ev,
        "onset_ms": 500,
        "sensor_settle_ms": 900,
        "screen_settle_ms": 700,
        "overshoot_ev": 0.02,
        "sensor_overshoot_ev": 0.05,
        "max_abs_gain_ev": 0.3,
        "gain_at_end_ev": 0.0,
        "proposals": 12,
        "generations": 9,
    }
    row.update(overrides)
    return [row[column] for column in STEP_COLUMNS]


def missed_row(cue: int) -> list:
    return [cue, "no_step", 0.05, None, None, None, None, None, None, None, None, None]


def flicker(share: float | None = 1.0) -> dict:
    return {
        "readings": {"absent": 0, "none": 0, "mains_50hz": 200, "mains_60hz": 0},
        "long_frames": 40,
        "multiple_frames": 40,
        "multiple_share": share,
        "max_exposure_ms": 16.6,
    }


def ae_trace_report() -> dict:
    """Здоровая сводка: по две настоящие ступеньки в фото (быстрее) и видео (медленнее), статика ровная."""
    return {
        "real_steps": 4,
        "samples": 1800,
        "dropped_samples": 0,
        "ticks_without_ae": 0,
        "time_to_streaming_ms": 900,
        "time_to_live_ms": 1200,
        "haptics": {"queued": 14, "failed": {"count": 0, "first": []}},
        "gain_limit_ev": 0.5,
        "restore_failure": None,
        "step_cols": STEP_COLUMNS,
        "max_abs_gain_ev": 0.31,
        "phases": [
            {
                "phase": "photo",
                "profile": "photo",
                "profile_confirmed": True,
                "samples": 700,
                "max_abs_gain_ev": 0.31,
                "flicker": flicker(),
                "steps": [measured_row(1, 1.0), measured_row(2, -1.0, screen_settle_ms=650)],
                "median_screen_settle_ms": 675.0,
                "worst_overshoot_ev": 0.02,
            },
            {
                "phase": "video",
                "profile": "video",
                "profile_confirmed": True,
                "samples": 700,
                "max_abs_gain_ev": 0.28,
                "flicker": flicker(),
                "steps": [
                    measured_row(1, 1.0, screen_settle_ms=3200, sensor_settle_ms=3400),
                    measured_row(2, -1.0, screen_settle_ms=3000, sensor_settle_ms=3300),
                    missed_row(3),
                ],
                "median_screen_settle_ms": 3100.0,
                "worst_overshoot_ev": 0.01,
            },
            {
                "phase": "static",
                "profile": "photo",
                "profile_confirmed": True,
                "samples": 400,
                "max_abs_gain_ev": 0.0,
                "flicker": flicker(None),
                "quiet": {
                    "analyzed_ms": 16000,
                    "target_range_ev": 0.02,
                    "sensor_range_ev": 0.04,
                    "screen_range_ev": 0.03,
                    "proposals": 2,
                    "proposals_per_s": 0.1,
                    "generations": 1,
                    "generations_per_s": 0.1,
                },
            },
        ],
    }


def edited(mutate) -> dict:
    report = copy.deepcopy(ae_trace_report())
    mutate(report)
    return report


def passed(report: dict) -> dict:
    return {"scenario": "ae-trace", "status": "passed", "report": report}


def check(result: dict) -> tuple[list[str], str, str]:
    """Проблемы проверки и то, что она напечатала в stdout и stderr."""
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        problems = smoke_checks_ae_trace.check_ae_trace(result)
    return problems, out.getvalue(), err.getvalue()


class AeTraceCheckTest(unittest.TestCase):
    def test_scenario_is_registered_and_a_healthy_run_passes_end_to_end(self) -> None:
        self.assertIn("ae-trace", smoke_wait.CHECKS)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            verdict = smoke_wait.evaluate_stream([result_line(passed(ae_trace_report()))], "ae-trace")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        self.assertIn("ae-trace: итоги", out.getvalue())

    def test_healthy_run_has_no_problems_and_prints_the_table_to_stdout(self) -> None:
        problems, out, err = check(passed(ae_trace_report()))
        self.assertEqual(problems, [])
        self.assertEqual(err, "")
        # Фото и видео рядом: время установки экрана — медиана по настоящим ступенькам каждой фазы.
        photo_line = next(line for line in out.splitlines() if line.strip().startswith("photo"))
        video_line = next(line for line in out.splitlines() if line.strip().startswith("video"))
        self.assertIn("675", photo_line)
        self.assertIn("3100", video_line)
        self.assertIn("настоящих ступенек: 4", out)
        self.assertIn("мерцание", out)
        self.assertIn("mains_50hz=200", out)
        self.assertIn("статика (16000 мс)", out)

    def test_each_rule_names_its_problem(self) -> None:
        def set_row(phase: int, index: int, column: str, value: object):
            def apply(report: dict) -> None:
                report["phases"][phase]["steps"][index][STEP_COLUMNS.index(column)] = value

            return apply

        cases = {
            "ни одной настоящей ступеньки": edited(
                lambda r: (r["phases"][0].update(steps=[missed_row(1)]), r["phases"][1].update(steps=[missed_row(1)]),
                           r.update(real_steps=0))
            ),
            "real_steps=3": edited(lambda r: r.update(real_steps=3)),
            "перерегулирование": edited(set_row(0, 0, "overshoot_ev", 0.45)),
            "меньше 0.4": edited(set_row(0, 0, "step_ev", 0.3)),
            "подтяжка ISP": edited(lambda r: r.update(max_abs_gain_ev=0.7)),
            "не подтверждён": edited(lambda r: r["phases"][1].update(profile_confirmed=False)),
            "подсказки владельцу не дошли": edited(
                lambda r: r["haptics"].update(failed={"count": 2, "first": ["подсказка 1: Backpressure"]})
            ),
            "не возвращён": edited(lambda r: r.update(restore_failure="очередь")),
            "ряд переполнен": edited(lambda r: r.update(dropped_samples=4)),
            "колонки ступенек": edited(lambda r: r.update(step_cols=["cue"])),
            "фазы отчёта": edited(lambda r: r.update(phases=r["phases"][:2])),
            "не из 12 колонок": edited(lambda r: r["phases"][0]["steps"].append([1, "measured"])),
            "неизвестный исход": edited(set_row(0, 0, "kind", "weird")),
            "нет сводки «дыхания»": edited(lambda r: r["phases"][2].pop("quiet")),
            "в flicker.readings нет": edited(lambda r: r["phases"][0]["flicker"]["readings"].pop("none")),
        }
        for needle, report in cases.items():
            with self.subTest(needle=needle):
                problems, _, _ = check(passed(report))
                self.assertTrue(any(needle in problem for problem in problems), f"{needle!r} не найдено в {problems}")

    def test_missed_cues_are_reported_in_the_table_but_are_not_problems(self) -> None:
        problems, out, _ = check(passed(ae_trace_report()))
        self.assertEqual(problems, [])
        video_line = next(line for line in out.splitlines() if line.strip().startswith("video"))
        # 2 настоящих и 1 пропущенная подсказка.
        self.assertRegex(video_line, r"\s2\s+1\s")

    def test_failed_run_with_a_report_still_prints_the_table_to_stderr(self) -> None:
        result = {
            "scenario": "ae-trace",
            "status": "failed",
            "error": "подтяжка ISP 0.9 EV выше предела 0.52 EV",
            "report": edited(lambda r: r.update(max_abs_gain_ev=0.9)),
        }
        problems, out, err = check(result)
        self.assertTrue(any("status='failed'" in problem and "подтяжка ISP 0.9" in problem for problem in problems))
        self.assertEqual(out, "")
        self.assertIn("ae-trace: итоги", err)

    def test_run_aborted_before_analysis_reports_only_the_cause(self) -> None:
        result = {
            "scenario": "ae-trace",
            "status": "failed",
            "error": "экспозицию ведёт вендорский AE",
            "report": {"real_steps": 0, "step_cols": STEP_COLUMNS},
        }
        problems, out, err = check(result)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("вендорский AE", problems[0])
        self.assertEqual(out + err, "")

    def test_result_without_report_is_a_problem(self) -> None:
        problems, _, _ = check({"scenario": "ae-trace", "status": "passed"})
        self.assertIn("нет report в результате", problems)


if __name__ == "__main__":
    unittest.main()
