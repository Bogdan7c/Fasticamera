"""Сессия 16a, P4b: `gpu-saturation-bench` — проверка результата (успех, работа без цифры, пропавшая запрошенная работа,
шумный рендер, камера не закрыта) и флаги `device-smoke.sh` (без телефона)."""

from __future__ import annotations

import copy
import subprocess
import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_checks_saturation  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import smoke_wait  # noqa: E402
import test_smoke_device_script as device_script  # noqa: E402
from test_smoke_viewfinder import passed, result_line  # noqa: E402

SCENARIO = "gpu-saturation-bench"
REQUESTED = ["viewfinder", "compositor", "still"]
QUIET_RENDER = {
    "state": "observed",
    "before": {},
    "after": {},
    "delta": {"frames_presented": 600, "isp_frames": 0, "recomposed_frames": 0, "ui_frames_drawn": 0},
}


def report(**overrides: object) -> dict:
    """Здоровый отчёт: три работы с цифрами, камера закрыта, рендер молчал."""
    base: dict = {
        "requested": list(REQUESTED),
        "table": ["viewfinder|397|2.52|ok", "compositor|2550|0.392|ok", "still|26.7|37.5|ok"],
        "workloads": 3,
        "measured": 3,
        "cancelled": False,
        "validation": "disabled",
        "sensor": {"source": "engine", "camera": "2", "lens_shading_calibrated": True},
        "look": {"preset": "As Eye", "tone_base": "agx"},
        "camera": "closed_by_scenario",
        "warmup_ms": 3000,
        "measure_ms": 10000,
        "frames_in_flight": 2,
        "render_tolerance_frames": 5,
        "render": copy.deepcopy(QUIET_RENDER),
    }
    base.update(overrides)
    return base


def check(**overrides: object) -> list[str]:
    return smoke_checks_saturation.check_gpu_saturation_bench(passed(SCENARIO, report(**overrides)))


class SaturationResultCheckTest(unittest.TestCase):
    def test_healthy_result_passes(self) -> None:
        self.assertEqual(check(), [])

    def test_scenario_is_registered_with_checks_and_notes(self) -> None:
        self.assertIn(SCENARIO, smoke_wait.CHECKS)
        self.assertIn(SCENARIO, smoke_wait.NOTES)

    def test_failed_status_is_reported_with_its_error(self) -> None:
        problems = smoke_checks_saturation.check_gpu_saturation_bench(
            {"scenario": SCENARIO, "status": "failed", "error": "still: нехватка памяти"}
        )
        self.assertEqual(len(problems), 1)
        self.assertIn("нехватка памяти", problems[0])

    def test_a_work_without_a_number_fails_with_its_verdict(self) -> None:
        problems = check(table=["viewfinder|397|2.52|ok", "compositor|-|-|no_frames", "still|26.7|37.5|ok"], measured=2)
        self.assertTrue(any("compositor" in text and "no_frames" in text for text in problems), problems)
        self.assertTrue(any("2 работ из 3" in text for text in problems), problems)

    def test_a_requested_work_missing_from_the_table_fails(self) -> None:
        problems = check(table=["viewfinder|397|2.52|ok", "still|26.7|37.5|ok"], measured=3)
        self.assertEqual([text for text in problems if "нет в таблице" in text], ["запрошенной работы compositor нет в таблице"])

    def test_extra_duplicate_and_malformed_rows_fail(self) -> None:
        problems = check(table=[*report()["table"], "viewfinder|1|1000|ok", "display_photo|5|200|ok", "мусор"])
        joined = "\n".join(problems)
        self.assertIn("viewfinder в таблице дважды", joined)
        self.assertIn("display_photo, которую не просили", joined)
        self.assertIn("не в формате", joined)

    def test_inconsistent_or_nonpositive_numbers_fail(self) -> None:
        self.assertTrue(any("не согласованы" in text for text in check(table=["viewfinder|397|9.99|ok", "compositor|2550|0.392|ok", "still|26.7|37.5|ok"])))
        self.assertTrue(any("больше нуля" in text for text in check(table=["viewfinder|0|0|ok", "compositor|2550|0.392|ok", "still|26.7|37.5|ok"])))
        self.assertTrue(any("не числа" in text for text in check(table=["viewfinder|-|-|ok", "compositor|2550|0.392|ok", "still|26.7|37.5|ok"])))

    def test_cancelled_run_and_open_camera_fail(self) -> None:
        self.assertTrue(any("отменён" in text for text in check(cancelled=True)))
        self.assertTrue(any("камера" in text for text in check(camera="streaming")))
        self.assertEqual(check(camera="engine_unavailable"), [])

    def test_render_work_above_tolerance_fails_and_within_tolerance_passes(self) -> None:
        noisy = copy.deepcopy(QUIET_RENDER)
        noisy["delta"]["isp_frames"] = 6
        self.assertTrue(any("6 кадров продуктовой работы" in text for text in check(render=noisy)))
        noisy["delta"]["isp_frames"] = 5
        self.assertEqual(check(render=noisy), [])
        unavailable = {"state": "render_unavailable", "reason": "нет vsync"}
        self.assertEqual(check(render=unavailable), [])

    def test_missing_requested_list_is_a_problem_not_a_pass(self) -> None:
        result = passed(SCENARIO, {k: v for k, v in report().items() if k != "requested"})
        problems = smoke_checks_saturation.check_gpu_saturation_bench(result)
        self.assertTrue(any("нет списка запрошенных" in text for text in problems), problems)

    def test_notes_name_the_sensor_source_validation_and_render(self) -> None:
        notes = smoke_checks_saturation.saturation_notes(
            passed(SCENARIO, report(validation="enabled", sensor={"source": "fallback", "reason": "движок камеры недоступен"}))
        )
        text = "\n".join(notes)
        self.assertIn("ЗАПАСНОЙ", text)
        self.assertIn("validation включён", text)
        self.assertIn("As Eye", text)

    def test_wait_loop_gives_exit_code_by_the_check(self) -> None:
        good = smoke_wait.evaluate_stream([result_line(passed(SCENARIO, report()))], SCENARIO)
        self.assertEqual(good.exit_code, 0, good.message)
        broken = smoke_wait.evaluate_stream(
            [result_line(passed(SCENARIO, report(table=["viewfinder|397|2.52|ok"], measured=1)))], SCENARIO
        )
        self.assertEqual(broken.exit_code, 1, broken.message)
        self.assertIn("compositor", broken.message)


class DeviceSmokeSaturationFlagsTest(unittest.TestCase):
    """`device-smoke.sh` на фейковом adb (тот же, что у `test_smoke_device_script`)."""

    setUp = device_script.DeviceSmokeScenariosTest.setUp
    run_smoke = device_script.DeviceSmokeScenariosTest.run_smoke
    adb_calls = device_script.DeviceSmokeScenariosTest.adb_calls

    OK = passed(SCENARIO, report())

    def start_call(self) -> str:
        return next(call for call in self.adb_calls() if "am start" in call)

    def test_flags_reach_the_intent_as_extras(self) -> None:
        completed = self.run_smoke(
            SCENARIO,
            [result_line(self.OK)],
            extra_arguments=(
                "--saturation-workloads", "viewfinder,compositor,still",
                "--saturation-measure-ms", "5000",
                "--saturation-warmup-ms", "1500",
            ),
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        start = self.start_call()
        self.assertIn("-e saturation_workloads viewfinder,compositor,still", start)
        self.assertIn("-e saturation_measure_ms 5000", start)
        self.assertIn("-e saturation_warmup_ms 1500", start)

    def test_without_the_flags_no_saturation_extra_is_sent(self) -> None:
        completed = self.run_smoke(SCENARIO, [result_line(self.OK)])
        self.assertEqual(completed.returncode, 0, completed.stderr)
        start = self.start_call()
        for extra in ("saturation_workloads", "saturation_measure_ms", "saturation_warmup_ms"):
            self.assertNotIn(extra, start)

    def test_broken_values_and_the_wrong_scenario_are_usage_errors(self) -> None:
        for arguments, scenario in (
            (("--saturation-workloads", "Viewfinder"), SCENARIO),
            (("--saturation-workloads", "viewfinder;ls"), SCENARIO),
            (("--saturation-workloads", "viewfinder,,still"), SCENARIO),
            (("--saturation-measure-ms", "0"), SCENARIO),
            (("--saturation-measure-ms", "пять"), SCENARIO),
            (("--saturation-warmup-ms", "-1"), SCENARIO),
            (("--saturation-workloads", "viewfinder"), "viewfinder"),
            (("--saturation-measure-ms", "1000"), "hello"),
        ):
            with self.subTest(arguments=arguments, scenario=scenario):
                completed = self.run_smoke(scenario, [], extra_arguments=arguments)
                self.assertEqual(completed.returncode, 64, completed.stderr)

    def test_a_result_with_a_missing_work_makes_the_script_fail(self) -> None:
        broken = passed(SCENARIO, report(table=["viewfinder|397|2.52|ok", "still|26.7|37.5|ok"]))
        completed = self.run_smoke(SCENARIO, [result_line(broken)])
        self.assertEqual(completed.returncode, 1, completed.stderr)

    def test_help_documents_the_saturation_flags(self) -> None:
        completed = subprocess.run(
            ["bash", str(SCRIPTS_DIR / "device-smoke.sh"), "--help"], capture_output=True, text=True, timeout=30
        )
        for flag in ("--saturation-workloads", "--saturation-measure-ms", "--saturation-warmup-ms", "gpu-saturation-bench"):
            self.assertIn(flag, completed.stderr)


if __name__ == "__main__":
    unittest.main()
