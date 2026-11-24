"""Тесты журнала тепла телефона: разбор вывода, команда, запись при обрывах adb, сводка по окнам."""

from __future__ import annotations

import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import device_thermal_log as log  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import thermal_log_parse as parse  # noqa: E402
import thermal_log_summary as summary  # noqa: E402

FIXTURE = Path(__file__).resolve().parent / "fixtures" / "thermal" / "sample_normal.txt"
SERIAL = "SECRET-SERIAL-123"
ZONES = ("gpuss-0", "gpuss-1", "gpuss-2", "gpuss-3", "shell_front", "battery", "shell_back")


def normal_output() -> str:
    return FIXTURE.read_text(encoding="utf-8")


class ParseSampleTest(unittest.TestCase):
    def test_normal_output_parsed_to_numbers(self) -> None:
        sample = parse.parse_sample(normal_output(), ZONES)
        self.assertAlmostEqual(sample.phone_epoch, 1792957000.123456789, places=3)
        self.assertEqual(sample.zone_temps_c["gpuss-0"], 37.4)
        self.assertEqual(sample.zone_temps_c["gpuss-1"], 38.1)
        self.assertEqual(sample.zone_temps_c["shell_front"], 33.5)
        self.assertAlmostEqual(sample.gpu_busy_fraction, 3164 / 1549719)
        self.assertEqual(sample.thermal_status, 1)
        self.assertEqual(sample.battery_temp_c, 35.3)
        self.assertEqual(sample.battery_level_percent, 81)
        self.assertIs(sample.external_power, True)

    def test_permission_denied_and_absent_zone_become_null_and_counted(self) -> None:
        sample = parse.parse_sample(normal_output(), ZONES)
        self.assertIsNone(sample.zone_temps_c["gpuss-3"])  # Permission denied
        self.assertIsNone(sample.zone_temps_c["shell_back"])  # строки нет вовсе
        self.assertEqual(sample.read_failures, 2)  # ровно gpuss-3 и shell_back; остальное прочитано

    def test_gpubusy_zero_zero_is_no_data_not_zero_and_not_failure(self) -> None:
        self.assertEqual(parse.parse_gpu_busy("0 0"), (None, True))
        self.assertEqual(parse.parse_gpu_busy("   3164 1549719"), (3164 / 1549719, True))
        self.assertEqual(parse.parse_gpu_busy("cat: x: Permission denied"), (None, False))
        self.assertEqual(parse.parse_gpu_busy(None), (None, False))
        output = normal_output().replace("@@gpubusy    3164 1549719", "@@gpubusy       0       0")
        sample = parse.parse_sample(output, ZONES)
        self.assertIsNone(sample.gpu_busy_fraction)
        self.assertEqual(sample.read_failures, parse.parse_sample(normal_output(), ZONES).read_failures)

    def test_truncated_output_rejected_entirely(self) -> None:
        cut = normal_output().split("@@battery")[0] + "@@battery\n  level: 8"
        with self.assertRaises(parse.SampleParseError):
            parse.parse_sample(cut, ZONES)

    def test_garbage_lines_do_not_break_other_fields(self) -> None:
        output = normal_output().replace("@@thermal\n", "@@thermal\n\x00\x01 мусор ###\n").replace(
            "  level: 81", "  level: восемь"
        )
        sample = parse.parse_sample(output, ZONES)
        self.assertIsNone(sample.battery_level_percent)
        self.assertEqual(sample.thermal_status, 1)
        self.assertEqual(sample.zone_temps_c["gpuss-0"], 37.4)

    def test_missing_time_rejected(self) -> None:
        with self.assertRaises(parse.SampleParseError):
            parse.parse_sample("@@zone gpuss-0 37400\n@@end\n", ZONES)

    def test_no_external_power_lines_gives_none(self) -> None:
        output = normal_output().replace("true", "false")
        self.assertIs(parse.parse_sample(output, ZONES).external_power, False)
        stripped = "\n".join(line for line in normal_output().splitlines() if "powered" not in line)
        self.assertIsNone(parse.parse_sample(stripped, ZONES).external_power)


class CommandTest(unittest.TestCase):
    def test_sample_command_is_single_shell_line_reading_only_needed_paths(self) -> None:
        command = parse.build_sample_command({"gpuss-0": "/sys/class/thermal/thermal_zone12", "battery": "/sys/class/thermal/thermal_zone3"})
        self.assertNotIn("\n", command)
        self.assertIn("date +%s.%N", command)
        self.assertIn("/sys/class/thermal/thermal_zone12/temp", command)
        self.assertIn("/sys/class/kgsl/kgsl-3d0/gpubusy", command)
        self.assertIn("dumpsys thermalservice", command)
        self.assertIn("dumpsys battery", command)
        self.assertNotIn("/type", command)
        self.assertTrue(command.endswith('echo "@@end"'))

    def test_zone_listing_skips_unreadable_and_keeps_first_duplicate(self) -> None:
        listing = (
            "/sys/class/thermal/thermal_zone0 cpu-0\n"
            "/sys/class/thermal/thermal_zone1 cat: /sys/class/thermal/thermal_zone1/type: Permission denied\n"
            "/sys/class/thermal/thermal_zone2 gpuss-0\n"
            "/sys/class/thermal/thermal_zone9 gpuss-0\n"
            "мусор\n"
        )
        self.assertEqual(
            parse.parse_zone_listing(listing),
            {"cpu-0": "/sys/class/thermal/thermal_zone0", "gpuss-0": "/sys/class/thermal/thermal_zone2"},
        )


class FakeAdb:
    """Подставной adb: отвечает по очереди заранее заданными результатами и запоминает команды."""

    def __init__(self, results: list[log.AdbResult]) -> None:
        self.results = list(results)
        self.commands: list[list[str]] = []

    def __call__(self, command: list[str]) -> log.AdbResult:
        self.commands.append(command)
        return self.results.pop(0)


def ok(stdout: str) -> log.AdbResult:
    return log.AdbResult(0, stdout, "")


def offline() -> log.AdbResult:
    return log.AdbResult(1, "", f"error: device '{SERIAL}' not found")


def output_at(epoch: str) -> str:
    return normal_output().replace("1792957000.123456789", epoch)


def make_recorder(adb: FakeAdb, clock: list[float]) -> tuple[log.ThermalRecorder, io.StringIO]:
    buffer = io.StringIO()
    recorder = log.ThermalRecorder(adb, SERIAL, {"gpuss-0": "/sys/class/thermal/thermal_zone2"}, buffer, monotonic=lambda: clock[0])
    return recorder, buffer


class RecorderTest(unittest.TestCase):
    def test_adb_drop_writes_gap_then_recovers_with_same_period(self) -> None:
        adb = FakeAdb([ok(output_at("100.0")), offline(), ok(output_at("110.0"))])
        clock = [0.0]
        recorder, buffer = make_recorder(adb, clock)
        ticks: list[float] = []

        def fake_wait(timeout: float) -> bool:
            ticks.append(timeout)
            clock[0] += timeout
            return False

        written = recorder.run(period_seconds=5.0, duration_seconds=10.0, stop_requested=fake_wait)
        records = [json.loads(line) for line in buffer.getvalue().splitlines()]
        self.assertEqual(written, 3)
        self.assertEqual([r["kind"] for r in records], ["sample", "gap", "sample"])
        self.assertEqual(records[1]["t_phone_estimate"], 105.0)  # 100 + 5 с по часам ПК
        self.assertIn("adb код 1", records[1]["reason"])
        self.assertEqual(ticks, [5.0, 5.0])  # тот же период, без накопления дрейфа
        self.assertEqual(records[2]["t_phone"], 110.0)

    def test_gap_reason_never_contains_serial(self) -> None:
        adb = FakeAdb([offline()])
        recorder, buffer = make_recorder(adb, [0.0])
        recorder.take_sample()
        self.assertNotIn(SERIAL, buffer.getvalue())
        self.assertIn("<serial>", buffer.getvalue())

    def test_truncated_output_becomes_gap_not_crash(self) -> None:
        adb = FakeAdb([ok("@@time 5.0\n@@zone gpuss-0 374")])
        recorder, _ = make_recorder(adb, [0.0])
        record = recorder.take_sample()
        self.assertEqual(record["kind"], "gap")
        self.assertIsNone(record["t_phone_estimate"])  # замеров ещё не было

    def test_stop_request_ends_loop(self) -> None:
        adb = FakeAdb([ok(output_at("1.0")), ok(output_at("2.0"))])
        recorder, buffer = make_recorder(adb, [0.0])
        answers = iter([False, True])
        written = recorder.run(5.0, None, lambda _timeout: next(answers))
        self.assertEqual(written, 2)
        self.assertEqual(len(buffer.getvalue().splitlines()), 2)

    def test_one_adb_call_per_sample(self) -> None:
        adb = FakeAdb([ok(output_at("1.0"))])
        recorder, _ = make_recorder(adb, [0.0])
        recorder.take_sample()
        self.assertEqual(len(adb.commands), 1)
        self.assertEqual(adb.commands[0][:4], ["adb", "-s", SERIAL, "shell"])


class HeaderAndDiscoveryTest(unittest.TestCase):
    def test_header_has_version_model_zones_and_no_serial(self) -> None:
        header = log.header_record(5.0, "CPH2447", ["gpuss-0", "battery"])
        text = json.dumps(header)
        self.assertEqual(header["format_version"], 1)
        self.assertEqual(header["device_model"], "CPH2447")
        self.assertEqual(header["period_s"], 5.0)
        self.assertIn("shell_back", header["zones_missing"])
        self.assertNotIn(SERIAL, text)

    def test_full_header_written_by_recorder_has_no_serial(self) -> None:
        recorder, buffer = make_recorder(FakeAdb([]), [0.0])
        recorder.write_header(5.0, "CPH2447", ["gpuss-0"])
        self.assertNotIn(SERIAL, buffer.getvalue())

    def test_discover_zones_keeps_only_tracked(self) -> None:
        listing = "/sys/class/thermal/thermal_zone0 cpu-0\n/sys/class/thermal/thermal_zone2 gpuss-0\n"
        self.assertEqual(log.discover_zones(FakeAdb([ok(listing)]), SERIAL), {"gpuss-0": "/sys/class/thermal/thermal_zone2"})

    def test_discover_zones_errors(self) -> None:
        with self.assertRaises(log.ThermalLogError):
            log.discover_zones(FakeAdb([offline()]), SERIAL)
        with self.assertRaises(log.ThermalLogError):
            log.discover_zones(FakeAdb([ok("/sys/class/thermal/thermal_zone0 cpu-0\n")]), SERIAL)

    def test_find_serial_by_model_and_env(self) -> None:
        devices = "List of devices attached\nAAA device usb:1 model:CPH2447 device:x\nBBB device model:Pixel\n"
        self.assertEqual(log.find_device_serial(FakeAdb([ok(devices)]), {}), "AAA")
        self.assertEqual(log.find_device_serial(FakeAdb([]), {"ANDROID_SERIAL": "ZZZ"}), "ZZZ")
        with self.assertRaises(log.ThermalLogError):
            log.find_device_serial(FakeAdb([ok("List of devices attached\n")]), {})


def sample(epoch: float, gpuss: list[float], battery: float | None = 30.0, busy: float | None = 0.5) -> dict:
    temps = {f"gpuss-{index}": value for index, value in enumerate(gpuss)}
    temps["battery"] = battery
    return {
        "kind": "sample", "t_phone": epoch, "temps_c": temps, "gpu_busy_fraction": busy, "thermal_status": 0,
        "battery_temp_c": 35.0, "battery_level_percent": 80, "external_power": False, "read_failures": 0,
    }


class SummaryTest(unittest.TestCase):
    RECORDS = [
        sample(100.0, [40.0, 50.0]),
        sample(110.0, [44.0, 60.0], busy=None),
        {"kind": "gap", "t_phone_estimate": 115.0, "reason": "x"},
        sample(120.0, [50.0, 70.0], battery=None),
        sample(200.0, [30.0, 32.0]),
    ]

    def test_window_stats_are_numbers(self) -> None:
        result = summary.summarize_window(self.RECORDS, summary.TimeWindow("до", 100.0, 130.0))
        self.assertEqual((result.sample_count, result.gap_count), (3, 1))
        gpuss0 = result.metrics["temp:gpuss-0"]
        self.assertEqual((gpuss0.count, gpuss0.minimum, gpuss0.maximum, gpuss0.first, gpuss0.last), (3, 40.0, 50.0, 40.0, 50.0))
        self.assertAlmostEqual(gpuss0.mean, 44.666666, places=4)
        self.assertEqual(result.metrics["temp:gpuss_max"].maximum, 70.0)
        self.assertEqual(result.metrics["temp:gpuss_max"].mean, (50.0 + 60.0 + 70.0) / 3)
        self.assertEqual(result.metrics["temp:gpuss_mean"].first, 45.0)
        self.assertEqual(result.metrics["temp:battery"].count, 2)  # None не считается нулём
        self.assertEqual(result.metrics["gpu_busy_fraction"].count, 2)

    def test_window_is_half_open_and_excludes_outside(self) -> None:
        result = summary.summarize_window(self.RECORDS, summary.TimeWindow("после", 120.0, 200.0))
        self.assertEqual(result.sample_count, 1)
        self.assertEqual(result.gap_count, 0)
        self.assertEqual(result.metrics["temp:gpuss-1"].maximum, 70.0)

    def test_empty_window_has_no_metrics(self) -> None:
        result = summary.summarize_window(self.RECORDS, summary.TimeWindow("пусто", 500.0, 600.0))
        self.assertEqual((result.sample_count, result.metrics), (0, {}))

    def test_markdown_table_contains_values_and_counts(self) -> None:
        text = summary.render_markdown(summary.summarize(self.RECORDS, [summary.TimeWindow("до", 100.0, 130.0)]))
        self.assertIn("Замеров: 3, пропусков: 1", text)
        self.assertIn("| gpuss_max | 3 | 50.00 | 60.00 | 70.00 | 50.00 | 70.00 |", text)

    def test_cli_roundtrip_with_windows_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            header = log.header_record(5.0, "CPH2447", ["gpuss-0"])
            (root / "t.jsonl").write_text("\n".join(json.dumps(r) for r in [header, *self.RECORDS]) + "\n", encoding="utf-8")
            (root / "w.json").write_text(json.dumps([{"name": "A", "from_epoch": 100, "to_epoch": 130}]), encoding="utf-8")
            stdout = io.StringIO()
            real_stdout, sys.stdout = sys.stdout, stdout
            try:
                code = log.main(["summarize", str(root / "t.jsonl"), "--windows-json", str(root / "w.json"), "--json"])
            finally:
                sys.stdout = real_stdout
            self.assertEqual(code, 0)
            payload = json.loads(stdout.getvalue())
            self.assertEqual(payload[0]["samples"], 3)
            self.assertEqual(payload[0]["metrics"]["temp:gpuss_max"]["max"], 70.0)

    def test_log_without_header_is_error(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "t.jsonl"
            path.write_text(json.dumps(self.RECORDS[0]) + "\n", encoding="utf-8")
            with self.assertRaises(summary.SummaryError):
                summary.read_log(path)


if __name__ == "__main__":
    unittest.main()
