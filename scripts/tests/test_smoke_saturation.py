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
        self.assertTrue(any("нет числа запрошенных" in text for text in problems), problems)

    def new_format(self, **overrides: object) -> dict:
        """Формат 14o: имён в сводке нет, есть число запрошенных и пропавшие."""
        fields = {k: v for k, v in report().items() if k != "requested"}
        fields.update(requested_count=len(REQUESTED), requested_missing=[])
        fields.update(overrides)
        return passed(SCENARIO, fields)

    def test_new_format_without_names_passes_when_every_requested_work_has_a_row(self) -> None:
        self.assertEqual(smoke_checks_saturation.check_gpu_saturation_bench(self.new_format()), [])

    def test_new_format_names_a_work_that_got_no_table_row(self) -> None:
        result = self.new_format(
            requested_count=4, requested_missing=["headroom_v4"], measured=3
        )
        problems = smoke_checks_saturation.check_gpu_saturation_bench(result)
        self.assertTrue(any("headroom_v4 нет в таблице" in text for text in problems), problems)

    def test_new_format_fails_when_a_row_vanished_without_being_named(self) -> None:
        result = self.new_format(requested_count=4)
        problems = smoke_checks_saturation.check_gpu_saturation_bench(result)
        self.assertTrue(any("строка потерялась" in text for text in problems), problems)

    def test_new_format_with_a_broken_missing_list_is_a_problem(self) -> None:
        problems = smoke_checks_saturation.check_gpu_saturation_bench(self.new_format(requested_missing="x"))
        self.assertTrue(any("requested_missing не список" in text for text in problems), problems)

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


PARITY_REQUESTED = ["look_parity_agx_bands", "look_parity_agx_tone"]


def parity_report(**overrides: object) -> dict:
    """Здоровый отчёт прогона группы паритета: две работы с цифрой цены и две строки паритета (допуск держится)."""
    base = report(
        requested=list(PARITY_REQUESTED),
        table=["look_parity_agx_bands|397|2.52|ok", "look_parity_agx_tone|400|2.50|ok"],
        workloads=2,
        measured=2,
        parity_expected=2,
        parity=["look_parity_agx_bands|9.50e-6|5.00e-4|1e-5|pass", "look_parity_agx_tone|0.00e0|4.90e-4|1e-5|pass"],
    )
    base.update(overrides)
    return base


def check_parity(**overrides: object) -> list[str]:
    return smoke_checks_saturation.check_gpu_saturation_bench(passed(SCENARIO, parity_report(**overrides)))


class SaturationParityResultTest(unittest.TestCase):
    """14o: паритет GPU с CPU-эталоном на устройстве — провал, если ошибка выше допуска или числа нет."""

    def test_parity_inside_the_allowance_passes(self) -> None:
        self.assertEqual(check_parity(), [])

    def test_excess_above_the_allowance_fails_naming_the_work(self) -> None:
        problems = check_parity(
            parity=["look_parity_agx_bands|1.35e-5|5.02e-4|1e-5|fail", "look_parity_agx_tone|0.00e0|4.90e-4|1e-5|pass"]
        )
        self.assertTrue(any("look_parity_agx_bands" in text and "больше допуска" in text for text in problems), problems)
        # Вердикт «pass» при числе выше допуска — тоже провал: скрипт не верит вердикту на слово.
        lying = check_parity(
            parity=["look_parity_agx_bands|1.35e-5|5.02e-4|1e-5|pass", "look_parity_agx_tone|0.00e0|4.90e-4|1e-5|pass"]
        )
        self.assertTrue(any("больше допуска" in text for text in lying), lying)

    def test_excess_exactly_at_the_allowance_passes(self) -> None:
        self.assertEqual(
            check_parity(
                parity=["look_parity_agx_bands|1.00e-5|5.00e-4|1e-5|pass", "look_parity_agx_tone|0.00e0|4.90e-4|1e-5|pass"]
            ),
            [],
        )

    def test_a_work_without_a_number_fails(self) -> None:
        problems = check_parity(
            parity=["look_parity_agx_bands|-|-|-|unavailable", "look_parity_agx_tone|0.00e0|4.90e-4|1e-5|pass"]
        )
        self.assertTrue(any("look_parity_agx_bands" in text and "не измерен" in text for text in problems), problems)

    def test_a_missing_row_fails_because_the_expected_count_comes_from_the_requested_list(self) -> None:
        problems = check_parity(parity=["look_parity_agx_bands|9.50e-6|5.00e-4|1e-5|pass"])
        self.assertTrue(any("1 из 2" in text for text in problems), problems)

    def test_missing_parity_fields_fail_when_parity_works_were_requested(self) -> None:
        stripped = {k: v for k, v in parity_report().items() if k not in ("parity", "parity_expected")}
        problems = smoke_checks_saturation.check_gpu_saturation_bench(passed(SCENARIO, stripped))
        self.assertTrue(any("parity_expected" in text for text in problems), problems)
        no_rows = {k: v for k, v in parity_report().items() if k != "parity"}
        problems = smoke_checks_saturation.check_gpu_saturation_bench(passed(SCENARIO, no_rows))
        self.assertTrue(any("report.parity" in text for text in problems), problems)

    def test_foreign_duplicate_and_malformed_rows_fail(self) -> None:
        problems = check_parity(
            parity=[
                "look_parity_agx_bands|9.50e-6|5.00e-4|1e-5|pass",
                "look_parity_agx_bands|9.50e-6|5.00e-4|1e-5|pass",
                "look_parity_other|1.00e-6|5.00e-4|1e-5|pass",
                "мусор",
            ]
        )
        joined = "\n".join(problems)
        self.assertIn("дважды", joined)
        self.assertIn("которую не просили", joined)
        self.assertIn("не в формате", joined)

    def test_runs_without_parity_works_need_no_parity_fields(self) -> None:
        self.assertEqual(check(), [])

    def test_notes_name_the_worst_parity_excess(self) -> None:
        notes = smoke_checks_saturation.saturation_notes(passed(SCENARIO, parity_report()))
        self.assertTrue(any("9.50e-06" in note and "look_parity_agx_bands" in note for note in notes), notes)


SCOPES_REQUESTED = ["scopes_waveform", "scopes_vectorscope_ycbcr"]


def scopes_report(**overrides: object) -> dict:
    """Здоровый отчёт с двумя работами приборов и их настоящими размерами картинки."""
    base = report(
        requested=list(SCOPES_REQUESTED),
        table=["scopes_waveform|300|3.33|ok", "scopes_vectorscope_ycbcr|250|4.00|ok"],
        workloads=2,
        measured=2,
        scope_pictures_expected=2,
        scope_pictures=["scopes_waveform|1011x827", "scopes_vectorscope_ycbcr|827x827"],
    )
    base.update(overrides)
    return base


class SaturationScopePictureSizeTest(unittest.TestCase):
    """14o, пункт 5: отчёт называет размер картинки каждого прибора; без размера прогон не принимается."""

    def check_scopes(self, **overrides: object) -> list[str]:
        return smoke_checks_saturation.check_gpu_saturation_bench(passed(SCENARIO, scopes_report(**overrides)))

    def test_sizes_for_every_scopes_work_pass(self) -> None:
        self.assertEqual(self.check_scopes(), [])

    def test_a_work_without_a_size_fails(self) -> None:
        problems = self.check_scopes(scope_pictures=["scopes_waveform|1011x827", "scopes_vectorscope_ycbcr|-"])
        self.assertTrue(any("scopes_vectorscope_ycbcr|-" in text for text in problems), problems)

    def test_zero_or_garbled_sizes_fail(self) -> None:
        for bad in ("scopes_waveform|0x827", "scopes_waveform|1011", "scopes_waveform|axb"):
            with self.subTest(bad=bad):
                problems = self.check_scopes(scope_pictures=[bad, "scopes_vectorscope_ycbcr|827x827"])
                self.assertTrue(any("не в формате" in text for text in problems), problems)

    def test_a_missing_row_fails_by_the_expected_count(self) -> None:
        problems = self.check_scopes(scope_pictures=["scopes_waveform|1011x827"])
        self.assertTrue(any("1 из 2" in text for text in problems), problems)

    def test_missing_field_fails_when_scopes_works_were_requested(self) -> None:
        stripped = {k: v for k, v in scopes_report().items() if k not in ("scope_pictures", "scope_pictures_expected")}
        problems = smoke_checks_saturation.check_gpu_saturation_bench(passed(SCENARIO, stripped))
        self.assertTrue(any("scope_pictures_expected" in text for text in problems), problems)

    def test_notes_name_the_picture_sizes(self) -> None:
        notes = smoke_checks_saturation.saturation_notes(passed(SCENARIO, scopes_report()))
        self.assertTrue(any("scopes_waveform 1011x827" in note for note in notes), notes)


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
