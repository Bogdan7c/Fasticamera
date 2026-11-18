"""Тесты чтения logcat, которое переживает обрыв беспроводного adb (дефект сессии 07, стенд V4)."""

from __future__ import annotations

import json
import sys
import unittest
from collections.abc import Iterator
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_logcat  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import smoke_wait  # noqa: E402

SINCE = "1792957000.000"


def log_line(epoch: str, message: str) -> str:
    """Строка в формате ``logcat -v epoch``, как её печатает телефон."""
    return f"  {epoch}  4242  4243 I Fasticamera: {message}\n"


def hello_result(epoch: str) -> str:
    payload = {
        "scenario": "hello",
        "status": "passed",
        "report": {
            "build": {"crate_version": "0.1.0", "git_revision": "8459f12abcd0", "abi": "arm64-v8a", "debug_assertions": True},
            "abi": "arm64-v8a",
            "message": "hello",
        },
    }
    return log_line(epoch, f"FASTICAMERA_RESULT {json.dumps(payload)}")


class FakeClock:
    """Часы теста: время двигают только отрезки и ожидание устройства."""

    def __init__(self) -> None:
        self.now = 0.0

    def __call__(self) -> float:
        return self.now

    def sleep(self, seconds: float) -> None:
        self.now += seconds


class ScriptedLogcat:
    """Подставной logcat: каждый отрезок — заранее заданные строки; запоминает метки ``-T``."""

    def __init__(self, clock: FakeClock, segments: list[list[str]], seconds_per_segment: float = 5.0) -> None:
        self.clock = clock
        self.segments = list(segments)
        self.seconds_per_segment = seconds_per_segment
        self.cursors: list[str] = []

    def open_segment(self, cursor_epoch: str, _deadline: float) -> Iterator[str]:
        self.cursors.append(cursor_epoch)
        lines = self.segments.pop(0) if self.segments else []
        yield from lines
        # Отрезок закончился (обрыв): прошло время работы отрезка.
        self.clock.now += self.seconds_per_segment


class DeviceWaiterRecorder:
    def __init__(self, clock: FakeClock, comes_back: bool, wait_cost_seconds: float = 3.0) -> None:
        self.clock = clock
        self.comes_back = comes_back
        self.wait_cost_seconds = wait_cost_seconds
        self.requested_waits: list[float] = []

    def __call__(self, wait_seconds: float) -> bool:
        self.requested_waits.append(wait_seconds)
        self.clock.now += min(self.wait_cost_seconds, wait_seconds) if self.comes_back else wait_seconds
        return self.comes_back


def run_resumable(
    logcat: ScriptedLogcat,
    waiter: DeviceWaiterRecorder,
    clock: FakeClock,
    timeout: float = 100.0,
    since_epoch: str = SINCE,
) -> tuple[list[str], list[str]]:
    """Читает поток как настоящее ожидание: до строки результата или до конца потока."""
    reports: list[str] = []
    lines: list[str] = []
    for line in smoke_logcat.resumable_lines(
        open_segment=logcat.open_segment,
        wait_for_device=waiter,
        since_epoch=since_epoch,
        deadline=clock() + timeout,
        clock=clock,
        pause=clock.sleep,
        report=reports.append,
    ):
        lines.append(line)
        if smoke_wait.RESULT_MARKER in line:
            break
    return lines, reports


class ResumableLinesTest(unittest.TestCase):
    def test_drop_mid_run_resumes_from_last_epoch_and_finds_result(self) -> None:
        clock = FakeClock()
        first = log_line("1792957001.100", "стадия 1")
        boundary = log_line("1792957002.200", "стадия 2")
        same_millisecond = log_line("1792957002.200", "соседняя запись в ту же миллисекунду")
        result = hello_result("1792957600.000")
        logcat = ScriptedLogcat(clock, [[first, boundary], [boundary, same_millisecond, result]])
        waiter = DeviceWaiterRecorder(clock, comes_back=True)

        lines, reports = run_resumable(logcat, waiter, clock)

        # Второй отрезок начат с последней увиденной метки, а не с начала прогона.
        self.assertEqual(logcat.cursors, [SINCE, "1792957002.200"])
        # Повтор на стыке отброшен, новая строка с той же меткой — нет.
        self.assertEqual(lines, [first, boundary, same_millisecond, result])
        self.assertEqual(len(waiter.requested_waits), 1)
        self.assertTrue(any("оборвался" in message for message in reports))
        verdict = smoke_wait.evaluate_stream(iter(lines), "hello")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)

    def test_device_never_returns_ends_without_result(self) -> None:
        clock = FakeClock()
        logcat = ScriptedLogcat(clock, [[log_line("1792957001.100", "стадия 1")]])
        waiter = DeviceWaiterRecorder(clock, comes_back=False)

        lines, reports = run_resumable(logcat, waiter, clock, timeout=60.0)

        # Ждали ровно остаток дедлайна (60 − 5 с работы отрезка), второй отрезок не открывали.
        self.assertEqual(waiter.requested_waits, [55.0])
        self.assertEqual(logcat.cursors, [SINCE])
        self.assertTrue(any("не вернулось" in message for message in reports))
        verdict = smoke_wait.evaluate_stream(iter(lines), "hello")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_NO_RESULT)

    def test_deadline_reached_does_not_wait_for_device(self) -> None:
        clock = FakeClock()
        logcat = ScriptedLogcat(clock, [[log_line("1792957001.100", "тишина")]], seconds_per_segment=100.0)
        waiter = DeviceWaiterRecorder(clock, comes_back=True)

        lines, _reports = run_resumable(logcat, waiter, clock, timeout=100.0)

        self.assertEqual(waiter.requested_waits, [])
        self.assertEqual(logcat.cursors, [SINCE])
        self.assertEqual(len(lines), 1)

    def test_out_of_order_line_inside_segment_is_kept_and_cursor_stays_newest(self) -> None:
        clock = FakeClock()
        newer = log_line("1792957005.000", "новее")
        older_late = log_line("1792957004.900", "записана с опозданием")
        logcat = ScriptedLogcat(clock, [[newer, older_late], []], seconds_per_segment=60.0)
        waiter = DeviceWaiterRecorder(clock, comes_back=True)

        lines, _reports = run_resumable(logcat, waiter, clock)

        self.assertEqual(lines, [newer, older_late])
        self.assertEqual(logcat.cursors[:2], [SINCE, "1792957005.000"])

    def test_epoch_compared_as_numbers_not_strings(self) -> None:
        # Строкой «999.999» > «1000.000»: курсор застрял бы на старой метке и повторил бы весь хвост.
        clock = FakeClock()
        logcat = ScriptedLogcat(
            clock, [[log_line("999.999", "до"), log_line("1000.000", "после")], []], seconds_per_segment=60.0
        )
        waiter = DeviceWaiterRecorder(clock, comes_back=True)

        _lines, _reports = run_resumable(logcat, waiter, clock, since_epoch="999.000")

        self.assertEqual(logcat.cursors[:2], ["999.000", "1000.000"])

    def test_lines_without_epoch_pass_through_and_do_not_move_cursor(self) -> None:
        clock = FakeClock()
        banner = "--------- beginning of main\n"
        logcat = ScriptedLogcat(clock, [[banner], []], seconds_per_segment=60.0)
        waiter = DeviceWaiterRecorder(clock, comes_back=True)

        lines, _reports = run_resumable(logcat, waiter, clock)

        self.assertEqual(lines, [banner])
        self.assertEqual(logcat.cursors[:2], [SINCE, SINCE])


class ReadLogcatLinesWiringTest(unittest.TestCase):
    """``smoke_wait.read_logcat_lines`` действительно идёт через переподключение (подмена подпроцессов)."""

    def test_read_logcat_lines_survives_drop(self) -> None:
        result = hello_result("1792957600.000")
        segments = [[log_line("1792957001.100", "до обрыва")], [result]]
        cursors: list[str] = []
        waited_serials: list[str] = []

        def fake_segment(serial: str, since_epoch: str, _deadline: float) -> Iterator[str]:
            self.assertEqual(serial, "phone")
            cursors.append(since_epoch)
            yield from segments.pop(0)

        def fake_wait(serial: str, _wait_seconds: float) -> bool:
            waited_serials.append(serial)
            return True

        original_segment, original_wait = smoke_wait._logcat_segment, smoke_wait._wait_for_device
        original_pause = smoke_logcat.RECONNECT_PAUSE_SECONDS
        smoke_wait._logcat_segment = fake_segment  # type: ignore[assignment]
        smoke_wait._wait_for_device = fake_wait  # type: ignore[assignment]
        smoke_logcat.RECONNECT_PAUSE_SECONDS = 0.0
        try:
            verdict = smoke_wait.evaluate_stream(smoke_wait.read_logcat_lines("phone", SINCE, 30.0), "hello")
        finally:
            smoke_wait._logcat_segment, smoke_wait._wait_for_device = original_segment, original_wait  # type: ignore[assignment]
            smoke_logcat.RECONNECT_PAUSE_SECONDS = original_pause

        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        self.assertEqual(cursors, [SINCE, "1792957001.100"])
        self.assertEqual(waited_serials, ["phone"])


class ResultEchoContractTest(unittest.TestCase):
    """Приложение повторяет строку итога (решение 58): ожидание берёт первую дошедшую копию."""

    def test_first_copy_lost_in_logcat_the_repeat_is_enough(self) -> None:
        # Первая копия выпала из logcat во всплеске записи; через секунду пришёл повтор.
        lines = [
            log_line("1792957001.000", 'FASTICAMERA_STAGE {"scenario":"hello","stage":"started"}'),
            log_line("1792957001.300", "что-то ещё"),
            hello_result("1792957002.300"),
        ]
        verdict = smoke_wait.evaluate_stream(iter(lines), "hello")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)

    def test_repeated_copies_are_harmless(self) -> None:
        # Все копии дошли: вердикт по первой, лишние строки не меняют итог.
        lines = [hello_result("1792957002.000"), hello_result("1792957003.000"), hello_result("1792957004.000")]
        verdict = smoke_wait.evaluate_stream(iter(lines), "hello")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)

    def test_without_any_copy_there_is_no_result(self) -> None:
        verdict = smoke_wait.evaluate_stream(iter([log_line("1792957001.000", "тишина")]), "hello")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_NO_RESULT)

if __name__ == "__main__":
    unittest.main()
