"""Тесты smoke-сценария ``capture`` (сессия 14): перепроверка сводки (``smoke_checks_capture``), проверка файлов на ПК и
уборка (``smoke_checks_capture_files``) на подставном adb, сквозной ``device-smoke.sh capture`` со скриптом-заглушкой в PATH.

Телефон не нужен: формат вывода ``content query`` и семантика ``content delete`` проверятся на устройстве в 14c, здесь
закреплены правила скрипта — что он проверяет, что удаляет и чего не удаляет никогда."""

from __future__ import annotations

import copy
import json
import os
import stat
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_checks_capture  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import smoke_checks_capture_files as files  # noqa: E402
import smoke_driver  # noqa: E402
import smoke_wait  # noqa: E402
from smoke_dumpsys import CheckOutcome  # noqa: E402

SERIAL = "FAKE123"
PACKAGE = "dev.fasticamera.debug"
IMAGES = "content://media/external_primary/images/media/"


def result_line(payload: dict) -> str:
    return f"  1792957062.000  4242  4242 I Fasticamera: FASTICAMERA_RESULT {json.dumps(payload)}\n"


def compact_shot(number: int, step: str, outcome: str = "saved", **overrides: object) -> dict:
    shot = {"n": number, "step": step, "outcome": outcome, "fmt": "heif", "aspect": "4:3", "zoom_milli": 1000, "secured_ms": 40, "saved_ms": 900}
    shot.update(overrides)
    return shot


def capture_report(**overrides: object) -> dict:
    """Здоровая сводка ``capture``: 12 снимков (1 + 1 с DNG + 10), 13 адресов, серия без отказов."""
    shots = [compact_shot(1, "single_heif"), compact_shot(2, "single_heif_dng", "saved_with_raw")]
    shots += [compact_shot(number, "burst") for number in range(3, 13)]
    uris = [f"{IMAGES}{1000 + index}" for index in range(13)]
    report: dict = {
        "steps_planned": 3,
        "steps_done": 3,
        "unfinished": 0,
        "mode": {"mode": "real"},
        "mode_end": {"mode": "real"},
        "shots_total": 12,
        "shots_saved": 12,
        "shots": shots,
        "burst": {
            "presses": 10, "submitted": 10, "accepted": 10, "rejected_by_service": 0, "refused_by_screen": 0,
            "saved": 10, "failed": 0, "unfinished": 0, "press_gap_ms": [100, 104],
            "viewfinder": {"pool_exhausted": 0, "jitter_overflow": 0, "underruns": 0, "late_frames": 3,
                           "black_frames_while_picture_expected": 0, "holds_started": 0, "live_frames_presented": 300},
        },
        "phase_entries_lost": 0,
        "frames": {"exact": 12, "subst": 0, "evicted": 0, "unknown": 0, "share": 1.0, "p2f_max_ms": 92.4, "p2f_p50_ms": 81.0},
        "render_still": {"begins_accepted": 12},
        "full_frame": {"width_px": 4096, "height_px": 3072},
        "uris": uris,
    }
    report.update(overrides)
    return report


def passed(report: dict) -> dict:
    return {"scenario": "capture", "status": "passed", "report": report}


def verdict_of(report: dict, **kwargs: object) -> smoke_wait.SmokeVerdict:
    return smoke_wait.evaluate_stream([result_line(passed(report))], "capture", **kwargs)


class CaptureSummaryChecksTest(unittest.TestCase):
    def test_a_healthy_summary_passes_and_carries_the_numbers_in_notes(self) -> None:
        verdict = verdict_of(capture_report())
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        joined = " ".join(verdict.notes)
        self.assertIn("«кадр захвачен» через 40 мс", joined)
        self.assertIn("отказал экран 0", joined)
        self.assertIn("точно показанный кадр взят у 12 снимков", joined)
        self.assertIn("доля точных 1.0", joined)
        self.assertIn("медиана 81.0 мс, максимум 92.4 мс", joined)

    def problems(self, **overrides: object) -> list[str]:
        return smoke_checks_capture.check_capture(passed(capture_report(**overrides)))

    def test_frame_choice_deviations_are_named(self) -> None:
        healthy = {"exact": 12, "subst": 0, "evicted": 0, "unknown": 0, "share": 1.0, "p2f_max_ms": 92.4, "p2f_p50_ms": 81.0}
        cases = {
            "unknown": (dict(healthy, exact=11, unknown=1), "выбор кадра неизвестен у снимков: 1"),
            "evicted": (dict(healthy, exact=11, subst=1, evicted=1), "вытеснен из кольца у 1 снимков"),
            "not_measured": (dict(healthy, p2f_max_ms=None), "задержка «нажатие → кадр» не измерена"),
            "not_counted": (dict(healthy, exact=10), "выбор кадра учтён у 10 снимков из 12"),
        }
        for name, (frames, needle) in cases.items():
            with self.subTest(name):
                problems = self.problems(frames=frames)
                self.assertTrue(any(needle in problem for problem in problems), problems)
        without = capture_report()
        del without["frames"]
        self.assertTrue(any("нет учёта выбора кадра" in problem for problem in smoke_checks_capture.check_capture(passed(without))))
        garbage = self.problems(frames={"exact": "12"})
        self.assertTrue(any("нет чисел" in problem for problem in garbage), garbage)

    def test_a_substituting_but_otherwise_healthy_run_is_not_failed_by_the_script_and_the_note_names_the_replacements(self) -> None:
        # Порог доли точных судит Rust; скрипт замены сам не проваливает, но называет их в NOTE.
        frames = {"exact": 11, "subst": 1, "evicted": 0, "unknown": 0, "share": 0.917, "p2f_max_ms": 140.0, "p2f_p50_ms": 82.0}
        verdict = verdict_of(capture_report(frames=frames))
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        joined = " ".join(verdict.notes)
        self.assertIn("замен 1", joined)
        self.assertIn("доля точных 0.917", joined)

    def test_every_deviation_is_named(self) -> None:
        cases = {
            "steps_done": (dict(steps_done=2), "выполнено шагов 2 из 3"),
            "unfinished": (dict(unfinished=1), "не выполнено шагов"),
            "fake_start": (dict(mode={"mode": "fake_fallback", "reason": "x"}), "при старте не настоящий"),
            "fake_end": (dict(mode_end={"mode": "fake_for_scenario", "scenario": "screen"}), "в конце не настоящий"),
            "lost": (dict(phase_entries_lost=2), "потеряно записей 2"),
        }
        for name, (overrides, needle) in cases.items():
            with self.subTest(name):
                self.assertTrue(any(needle in problem for problem in self.problems(**overrides)), self.problems(**overrides))

    def test_shots_and_addresses_must_agree(self) -> None:
        report = capture_report()
        report["shots"][4] = compact_shot(5, "burst", "failed")
        problems = smoke_checks_capture.check_capture(passed(report))
        self.assertTrue(any("снимок 5 не сохранён: 'failed'" in problem for problem in problems), problems)
        self.assertTrue(any("адресов файлов 13, а по снимкам должно быть 12" in problem for problem in problems), problems)
        duplicate = capture_report(uris=[f"{IMAGES}1"] * 13)
        self.assertTrue(any("повторяются" in problem for problem in smoke_checks_capture.check_capture(passed(duplicate))))
        no_raw = capture_report()
        no_raw["shots"][1] = compact_shot(2, "single_heif_dng", "saved")
        no_raw["uris"] = no_raw["uris"][:12]
        self.assertTrue(any("снимков с DNG 0" in problem for problem in smoke_checks_capture.check_capture(passed(no_raw))))

    def test_burst_accounting_must_close(self) -> None:
        for field, value, needle in (
            ("refused_by_screen", 2, "нажатий 10, а попыток постановки 10 + отказов экрана 2"),
            ("saved", 9, "принято 10, а итогов 9"),
            ("failed", 1, "сорвано 1"),
            ("rejected_by_service", 1, "попыток 10, а принято 10 + отвергнуто 1"),
        ):
            with self.subTest(field):
                report = capture_report()
                report["burst"][field] = value
                problems = smoke_checks_capture.check_capture(passed(report))
                self.assertTrue(any(needle in problem for problem in problems), problems)

    def test_screen_refusals_that_add_up_are_a_note_not_a_failure(self) -> None:
        report = capture_report()
        report["burst"].update(submitted=6, accepted=6, saved=6, refused_by_screen=4)
        for index in range(7, 13):
            report["shots"].pop()
        report["uris"] = report["uris"][:7]
        verdict = verdict_of(report)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
        self.assertTrue(any("NOTE экран отказал 4" in note for note in verdict.notes), verdict.notes)

    def test_viewfinder_must_not_drop_frames_during_the_burst(self) -> None:
        for key in ("pool_exhausted", "black_frames_while_picture_expected", "holds_started"):
            with self.subTest(key):
                report = capture_report()
                report["burst"]["viewfinder"][key] = 1
                self.assertTrue(smoke_checks_capture.check_capture(passed(report)))

    def test_a_failed_scenario_is_a_failure_with_its_reason(self) -> None:
        payload = {"scenario": "capture", "status": "failed", "error": "сервис снимка не настоящий: запасной фейк"}
        verdict = smoke_wait.evaluate_stream([result_line(payload)], "capture")
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("запасной фейк", verdict.message)

    def test_capture_is_registered_without_screenshots_driver_steps_or_neutral_card(self) -> None:
        self.assertIn("capture", smoke_wait.CHECKS)
        self.assertIn("capture", smoke_wait.NOTES)
        self.assertNotIn("capture", smoke_wait.SCREENSHOT_SCENARIOS)
        self.assertNotIn("capture", smoke_wait.NEUTRAL_CARD_SCENARIOS)
        self.assertNotIn("capture", smoke_driver.SCENARIO_STEPS, "нажатия вкладывает сам сценарий")


class FakePhone:
    """Подставной adb: строки MediaStore по номеру, содержимое файлов, журнал вызовов, удалённое."""

    def __init__(self) -> None:
        self.rows: dict[str, str] = {}
        self.data: dict[str, bytes] = {}
        self.deleted: set[str] = set()
        self.calls: list[list[str]] = []
        self.delete_returncode = 0
        self.query_returncode = 0
        self.decode_answer: dict | None = {"status": "decoded", "width": 4096, "height": 3072, "color_space": "sRGB", "bitmap_config": "ARGB_8888"}

    @staticmethod
    def key(uri: str) -> str:
        return uri.rsplit("/", 1)[-1]

    def add(self, uri: str, mime: str = "image/heic", width: object = 4096, height: object = 3072,
            path: str = "DCIM/Camera/", pending: int = 0, data: bytes = b"HEIC",
            owner: str | None = PACKAGE, name: str | None = None) -> None:
        # Имя по шаблону приложения (``IMG_ГГГГММДД_ЧЧММСС_ммм.расширение``); номер строки — в миллисекундах.
        display_name = name or f"IMG_20261122_101530_{int(self.key(uri)) % 1000:03d}.HEIC"
        self.rows[self.key(uri)] = (
            f"Row: 0 _display_name={display_name}, mime_type={mime}, width={width}, height={height}, "
            f"relative_path={path}, is_pending={pending}, owner_package_name={owner if owner is not None else 'NULL'}"
        )
        self.data[self.key(uri)] = data

    def adb(self, command: list[str]) -> smoke_driver.AdbResult:
        self.calls.append(command)
        tail = command[3:]
        if tail[:3] == ["shell", "content", "query"]:
            if self.query_returncode:
                return smoke_driver.AdbResult(self.query_returncode, "", "ошибка adb")
            key = self.key(tail[4])
            if key in self.deleted or key not in self.rows:
                return smoke_driver.AdbResult(0, "No result found.\n", "")
            return smoke_driver.AdbResult(0, self.rows[key] + "\n", "")
        if tail[:3] == ["shell", "content", "delete"]:
            if not self.delete_returncode:
                self.deleted.add(self.key(tail[4]))
            return smoke_driver.AdbResult(self.delete_returncode, "", "отказ" if self.delete_returncode else "")
        if tail[:2] == ["shell", "am"] and tail[2] == "broadcast":
            if self.decode_answer is None:
                return smoke_driver.AdbResult(0, "Broadcast completed: result=0\n", "")
            return smoke_driver.AdbResult(0, f'Broadcast completed: result=0, data="{json.dumps(self.decode_answer)}"\n', "")
        raise AssertionError(f"неожиданная команда adb: {command}")

    def adb_bytes(self, command: list[str]) -> smoke_driver.AdbBytesResult:
        self.calls.append(command)
        key = self.key(command[-1])
        if key not in self.data:
            return smoke_driver.AdbBytesResult(1, b"", "нет файла")
        return smoke_driver.AdbBytesResult(0, self.data[key], "")

    def deletes(self) -> list[str]:
        return [call[-1] for call in self.calls if call[3:6] == ["shell", "content", "delete"]]


def tiff_bytes(tags: list[int], big_endian: bool = False) -> bytes:
    order = ">" if big_endian else "<"
    header = (b"MM" if big_endian else b"II") + struct.pack(f"{order}HI", 42, 8)
    entries = b"".join(struct.pack(f"{order}HHII", tag, 3, 1, 0) for tag in tags)
    return header + struct.pack(f"{order}H", len(tags)) + entries + struct.pack(f"{order}I", 0)


STUB_TOOLS = files.Tools(heif_info="/stub/heif-info", exiftool=None, rawpy_python=None)


def tool_runner(heif_output: str = "image: 4096x3072 (id=1), primary\n  tiles: 8x6, tile size: 512x512\n", code: int = 0):
    def run(command: list[str]) -> files.ToolResult:
        return files.ToolResult(code, heif_output, "")

    return run


class ContentQueryParsingTest(unittest.TestCase):
    def test_a_row_is_parsed_with_nulls(self) -> None:
        row = files.parse_content_query(
            "Row: 0 _display_name=IMG_1.HEIC, mime_type=image/heic, width=4096, height=NULL, relative_path=DCIM/Camera/, is_pending=0\n"
        )
        self.assertEqual(row, files.MediaRow("IMG_1.HEIC", "image/heic", 4096, None, "DCIM/Camera/", 0))

    def test_no_result_is_none(self) -> None:
        self.assertIsNone(files.parse_content_query("No result found.\n"))
        self.assertIsNone(files.parse_content_query(""))

    def test_tiff_tags_of_both_byte_orders_and_broken_files(self) -> None:
        self.assertEqual(files.tiff_ifd0_tags(tiff_bytes([254, 50706])), {254, 50706})
        self.assertEqual(files.tiff_ifd0_tags(tiff_bytes([50706], big_endian=True)), {50706})
        self.assertIsNone(files.tiff_ifd0_tags(b"not a tiff at all"))
        self.assertIsNone(files.tiff_ifd0_tags(tiff_bytes([1, 2, 3])[:-20]), "каталог обрывается")


class ExpectationsTest(unittest.TestCase):
    def test_addresses_map_to_shots_in_order_with_dng_after_its_primary(self) -> None:
        expectations = files.expectations_of(capture_report())
        self.assertEqual(len(expectations), 13)
        self.assertEqual([(e.shot, e.kind) for e in expectations[:3]], [(1, "primary"), (2, "primary"), (2, "raw")])
        self.assertEqual(expectations[0].size, (4096, 3072))
        self.assertEqual(expectations[2].mimes, files.RAW_MIME)
        self.assertEqual(expectations[3].shot, 3)

    def test_an_unsaved_shot_takes_no_address(self) -> None:
        report = capture_report()
        report["shots"][0] = compact_shot(1, "single_heif", "failed")
        report["uris"] = report["uris"][1:]
        expectations = files.expectations_of(report)
        self.assertEqual(expectations[0].shot, 2)
        self.assertEqual(len(expectations), 12)

    def test_other_framing_is_not_compared_and_says_so(self) -> None:
        report = capture_report()
        report["shots"][0] = compact_shot(1, "single_heif", aspect="16:9")
        first = files.expectations_of(report)[0]
        self.assertIsNone(first.size)
        self.assertIn("не сверяется", first.size_note or "")


class RowChecksTest(unittest.TestCase):
    def expectation(self, **overrides: object) -> files.FileExpectation:
        values = dict(uri=f"{IMAGES}1", shot=1, kind="primary", mimes=("image/heic", "image/heif"), size=(4096, 3072))
        values.update(overrides)
        return files.FileExpectation(**values)  # type: ignore[arg-type]

    def row(self, **overrides: object) -> files.MediaRow:
        values = dict(display_name="IMG.HEIC", mime_type="image/heic", width=4096, height=3072, relative_path="DCIM/Camera/", is_pending=0)
        values.update(overrides)
        return files.MediaRow(**values)  # type: ignore[arg-type]

    def test_a_good_row_has_no_problems_and_a_rotated_size_is_accepted(self) -> None:
        self.assertEqual(files.row_problems(self.expectation(), self.row(), ""), [])
        self.assertEqual(files.row_problems(self.expectation(), self.row(width=3072, height=4096), ""), [])

    def test_each_wrong_value_is_named(self) -> None:
        cases = (
            (dict(is_pending=1), "is_pending=1"),
            (dict(relative_path="Pictures/"), "relative_path='Pictures/'"),
            (dict(mime_type="image/jpeg"), "MIME 'image/jpeg'"),
            (dict(width=2000), "размеры 2000x3072"),
        )
        for overrides, needle in cases:
            with self.subTest(overrides):
                problems = files.row_problems(self.expectation(), self.row(**overrides), "")
                self.assertEqual(len(problems), 1, problems)
                self.assertIn(needle, problems[0])

    def test_a_missing_row_is_a_problem_with_the_raw_answer(self) -> None:
        problems = files.row_problems(self.expectation(), None, "No result found.")
        self.assertIn("MediaStore не вернул строку", problems[0])


class FilePostCheckTest(unittest.TestCase):
    def run_check(self, phone: FakePhone, report: dict | None = None, package: str = PACKAGE,
                  tools: files.Tools = STUB_TOOLS, run_tool=None) -> tuple[CheckOutcome, list[Path]]:
        created: list[Path] = []

        def make_temp_dir() -> str:
            path = tempfile.mkdtemp(prefix="capture-test-")
            created.append(Path(path))
            return path

        check = files.capture_files_post_check(
            SERIAL, package, run_adb=phone.adb, run_adb_bytes=phone.adb_bytes,
            run_tool=run_tool or tool_runner(), tools=tools, make_temp_dir=make_temp_dir,
        )
        return check(passed(report or capture_report())), created

    def healthy_phone(self, report: dict | None = None) -> FakePhone:
        phone = FakePhone()
        for expectation in files.expectations_of(report or capture_report()):
            if expectation.kind == "raw":
                phone.add(expectation.uri, mime="image/x-adobe-dng", data=tiff_bytes([254, 50706]))
            else:
                phone.add(expectation.uri)
        return phone

    def test_healthy_files_pass_with_notes_and_the_temp_dir_is_removed(self) -> None:
        outcome, created = self.run_check(self.healthy_phone())
        self.assertEqual(outcome.problems, [])
        joined = " ".join(outcome.notes)
        self.assertIn("плитки 8x6 по 512x512", joined)
        self.assertIn("ImageDecoder: снимок 1 4096x3072, пространство цвета sRGB", joined)
        self.assertIn("exiftool или perl не найден", joined, "недоступный инструмент — заметка, не провал")
        self.assertIn("проверено файлов 13", joined)
        self.assertTrue(created and not created[0].exists(), "временный каталог убран, в репозиторий ничего не пишется")

    def test_a_pending_file_is_a_problem(self) -> None:
        phone = self.healthy_phone()
        phone.add(f"{IMAGES}1000", pending=1)
        outcome, _ = self.run_check(phone)
        self.assertTrue(any("is_pending=1" in problem for problem in outcome.problems), outcome.problems)

    def test_a_broken_heic_and_a_wrong_decoded_size_are_problems(self) -> None:
        outcome, _ = self.run_check(self.healthy_phone(), run_tool=tool_runner("image: 2048x1536 (id=1), primary\n"))
        self.assertTrue(any("HEIC" in problem and "2048x1536" in problem for problem in outcome.problems), outcome.problems)
        phone = self.healthy_phone()
        phone.decode_answer = {"status": "failed", "error": "ImageDecoder$DecodeException"}
        outcome, _ = self.run_check(phone)
        self.assertTrue(any("ImageDecoder не открыл" in problem for problem in outcome.problems), outcome.problems)
        phone = self.healthy_phone()
        phone.decode_answer = None
        outcome, _ = self.run_check(phone)
        self.assertTrue(any("приёмник" in problem and "не ответил" in problem for problem in outcome.problems), outcome.problems)

    def test_release_package_skips_the_system_decoder_with_a_note(self) -> None:
        phone = self.healthy_phone()
        outcome, _ = self.run_check(phone, package="dev.fasticamera")
        self.assertEqual(outcome.problems, [])
        self.assertIn("приёмник есть только в debug-сборке", " ".join(outcome.notes))
        self.assertFalse(any("broadcast" in call for call in phone.calls))

    def test_a_dng_without_dng_version_is_a_problem(self) -> None:
        phone = self.healthy_phone()
        raw = files.expectations_of(capture_report())[2]
        phone.add(raw.uri, mime="image/x-adobe-dng", data=tiff_bytes([254]))
        outcome, _ = self.run_check(phone)
        self.assertTrue(any("нет тега DNGVersion" in problem for problem in outcome.problems), outcome.problems)

    def test_exiftool_and_rawpy_errors_fail_and_their_absence_is_a_note(self) -> None:
        tools = files.Tools(heif_info="/stub/heif-info", exiftool=Path("/stub/exiftool"), rawpy_python=Path("/stub/python"))

        def run(command: list[str]) -> files.ToolResult:
            if command[0] == "/stub/heif-info":
                return files.ToolResult(0, "image: 4096x3072 (id=1), primary\n", "")
            if command[0] == "perl":
                return files.ToolResult(0, "Validate : 0 warnings, 2 errors\nError : Bad IFD\n", "")
            return files.ToolResult(1, "", "LibRawFileUnsupportedError")

        outcome, _ = self.run_check(self.healthy_phone(), tools=tools, run_tool=run)
        joined = " | ".join(outcome.problems)
        self.assertIn("exiftool -validate: ошибок 2", joined)
        self.assertIn("exiftool: ошибка", joined)
        self.assertIn("rawpy не открыл", joined)

    def test_a_missing_file_content_is_a_problem_and_unsafe_addresses_are_never_queried(self) -> None:
        phone = self.healthy_phone()
        del phone.data["1001"]
        outcome, _ = self.run_check(phone)
        self.assertTrue(any("не скачался" in problem for problem in outcome.problems), outcome.problems)
        report = capture_report(uris=["content://media/external/images/media/5; rm -rf /"] + capture_report()["uris"][1:])
        phone = self.healthy_phone()
        outcome, _ = self.run_check(phone, report=report)
        self.assertTrue(any("не похож на файл MediaStore" in problem for problem in outcome.problems), outcome.problems)
        self.assertFalse(any("rm -rf" in " ".join(call) for call in phone.calls))

    def test_a_report_without_addresses_is_a_problem(self) -> None:
        outcome, _ = self.run_check(FakePhone(), report=capture_report(uris=[]))
        self.assertTrue(any("нет адресов файлов" in problem or "проверять нечего" in problem for problem in outcome.problems), outcome.problems)


class CleanupTest(unittest.TestCase):
    def phone_with_files(self) -> FakePhone:
        phone = FakePhone()
        for uri in capture_report()["uris"]:
            phone.add(uri)
        return phone

    def clean(self, phone: FakePhone, result: dict | None = None, keep: bool = False) -> CheckOutcome:
        return files.capture_cleanup(SERIAL, keep, PACKAGE, run_adb=phone.adb)(result or passed(capture_report()))

    def test_only_the_files_of_this_run_are_deleted_and_the_phone_is_left_clean(self) -> None:
        phone = self.phone_with_files()
        other = f"{IMAGES}77777"
        phone.add(other)
        outcome = self.clean(phone)
        self.assertEqual(outcome.problems, [])
        self.assertEqual(sorted(phone.deletes()), sorted(capture_report()["uris"]))
        self.assertNotIn(other, phone.deletes(), "чужой файл владельца не тронут")
        self.assertIn("удалено файлов 13 из 13", " ".join(outcome.notes))

    def test_keep_captures_deletes_nothing_and_lists_the_addresses(self) -> None:
        phone = self.phone_with_files()
        outcome = self.clean(phone, keep=True)
        self.assertEqual(phone.deletes(), [])
        self.assertIn("--keep-captures", outcome.notes[0])
        self.assertIn(f"{IMAGES}1000", outcome.notes[0])

    def test_a_file_outside_dcim_camera_is_not_deleted(self) -> None:
        phone = self.phone_with_files()
        phone.add(f"{IMAGES}1003", path="Pictures/Mine/")
        outcome = self.clean(phone)
        self.assertNotIn(f"{IMAGES}1003", phone.deletes())
        self.assertIn("НЕ удалён", " ".join(outcome.notes))
        self.assertEqual(len(phone.deletes()), 12)

    def test_a_file_of_another_owner_is_not_deleted(self) -> None:
        phone = self.phone_with_files()
        phone.add(f"{IMAGES}1003", owner="com.other.gallery")
        phone.add(f"{IMAGES}1004", owner=None)
        outcome = self.clean(phone)
        self.assertNotIn(f"{IMAGES}1003", phone.deletes())
        self.assertNotIn(f"{IMAGES}1004", phone.deletes(), "владелец неизвестен: не наш")
        self.assertIn("com.other.gallery", " ".join(outcome.notes))
        self.assertEqual(len(phone.deletes()), 11)

    def test_a_file_with_a_foreign_name_is_not_deleted(self) -> None:
        phone = self.phone_with_files()
        for number, name in ((1003, "Holiday.jpg"), (1004, "IMG_20261122_101530_12.HEIC"), (1005, "IMG_20261122_101530_123.PNG")):
            phone.add(f"{IMAGES}{number}", name=name)
        outcome = self.clean(phone)
        for number in (1003, 1004, 1005):
            self.assertNotIn(f"{IMAGES}{number}", phone.deletes())
        self.assertEqual(" ".join(outcome.notes).count("не по шаблону имён приложения"), 3)
        self.assertEqual(len(phone.deletes()), 10)

    def test_a_file_renamed_by_the_oneplus_gallery_is_still_ours_and_deleted(self) -> None:
        # 14c: галерея OnePlus переименовывает наш снимок «…_886.HEIC» в «…_0886.HEIC» (файл и владелец те же).
        phone = self.phone_with_files()
        renamed = capture_report()["uris"][0]
        phone.add(renamed, name="IMG_20261122_065359_0880.HEIC")
        outcome = self.clean(phone)
        self.assertIn(renamed, phone.deletes())
        self.assertNotIn("не по шаблону", " ".join(outcome.notes))
        # Пять цифр миллисекунд или ведущая не-ноль — по-прежнему чужое имя.
        phone = self.phone_with_files()
        for number, name in ((1003, "IMG_20261122_065359_00880.HEIC"), (1004, "IMG_20261122_065359_1880.HEIC")):
            phone.add(f"{IMAGES}{number}", name=name)
        outcome = self.clean(phone)
        for number in (1003, 1004):
            self.assertNotIn(f"{IMAGES}{number}", phone.deletes())

    def test_the_generic_file_address_form_is_never_queried_or_deleted(self) -> None:
        phone = self.phone_with_files()
        report = capture_report(uris=["content://media/external/file/1000", "content://media/external_primary/file/1001"])
        outcome = self.clean(phone, passed(report))
        self.assertEqual(phone.calls, [], "ни одного вызова adb")
        self.assertEqual(" ".join(outcome.notes).count("НЕ удалён"), 2)

    def test_an_unreadable_answer_is_not_counted_as_deleted(self) -> None:
        phone = self.phone_with_files()
        original = phone.adb

        def garbled(command: list[str]) -> smoke_driver.AdbResult:
            if command[3:6] == ["shell", "content", "query"]:
                phone.calls.append(command)
                return smoke_driver.AdbResult(0, "что-то непонятное\n", "")
            return original(command)

        outcome = files.capture_cleanup(SERIAL, False, PACKAGE, run_adb=garbled)(passed(capture_report()))
        self.assertEqual(phone.deletes(), [])
        self.assertEqual(" ".join(outcome.notes).count("ответ MediaStore не разобран"), 13)
        self.assertIn("удалено файлов 0 из 13", " ".join(outcome.notes))

    def test_an_unsafe_address_from_the_report_is_never_passed_to_adb(self) -> None:
        phone = self.phone_with_files()
        report = capture_report(uris=["content://media/external/images/media/1 --uri x", "file:///sdcard/DCIM/a.jpg", "content://other/1"])
        outcome = self.clean(phone, passed(report))
        self.assertEqual(phone.calls, [], "ни одного вызова adb")
        self.assertEqual(" ".join(outcome.notes).count("НЕ удалён"), 3)

    def test_a_failed_delete_and_a_dead_query_are_loud_notes_and_not_failures(self) -> None:
        phone = self.phone_with_files()
        phone.delete_returncode = 1
        outcome = self.clean(phone)
        self.assertEqual(outcome.problems, [])
        self.assertEqual(" ".join(outcome.notes).count("ВНИМАНИЕ, файл остался в галерее"), 13)
        dead = self.phone_with_files()
        dead.query_returncode = 1
        outcome = self.clean(dead)
        self.assertEqual(dead.deletes(), [], "вслепую не удаляем")
        self.assertIn("удалять вслепую нельзя", " ".join(outcome.notes))

    def test_a_result_without_a_report_cleans_nothing(self) -> None:
        phone = FakePhone()
        outcome = self.clean(phone, {"scenario": "capture", "status": "failed", "error": "x"})
        self.assertEqual((outcome.problems, outcome.notes, phone.calls), ([], [], []))


class CleanupInTheVerdictFlowTest(unittest.TestCase):
    def test_cleanup_runs_after_a_failed_check_and_its_notes_reach_the_verdict(self) -> None:
        phone = CleanupTest().phone_with_files()
        report = capture_report()
        report["burst"]["failed"] = 1
        payload = {"scenario": "capture", "status": "failed", "error": "серия: сорвалось снимков 1", "report": report}
        verdict = smoke_wait.evaluate_stream(
            [result_line(payload)], "capture", cleanup=files.capture_cleanup(SERIAL, False, PACKAGE, run_adb=phone.adb)
        )
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertEqual(len(phone.deletes()), 13, "файлы провального прогона тоже убраны")
        self.assertIn("удалено файлов 13 из 13", " ".join(verdict.notes))

    def test_cleanup_runs_after_the_post_check_even_when_the_post_check_fails(self) -> None:
        order: list[str] = []

        def post(_result: dict) -> CheckOutcome:
            order.append("post")
            return CheckOutcome(["файл битый"])

        def cleanup(_result: dict) -> CheckOutcome:
            order.append("cleanup")
            return CheckOutcome([], ["убрано"])

        verdict = verdict_of(capture_report(), post_check=post, cleanup=cleanup)
        self.assertEqual(order, ["post", "cleanup"])
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("файл битый", verdict.message)
        self.assertIn("убрано", verdict.notes)

    def test_cleanup_is_not_called_for_other_scenarios_results(self) -> None:
        calls: list[dict] = []
        foreign = {"scenario": "zoom", "status": "passed", "report": {}}
        smoke_wait.evaluate_stream([result_line(foreign)], "capture", cleanup=lambda result: calls.append(result) or CheckOutcome([]))
        self.assertEqual(calls, [])

    def test_cleanup_and_post_check_are_wired_for_capture_only(self) -> None:
        self.assertIsNotNone(smoke_wait.cleanup_for("capture", SERIAL, False, PACKAGE))
        self.assertIsNone(smoke_wait.cleanup_for("zoom", SERIAL, False, PACKAGE))
        self.assertIsNotNone(smoke_wait.capture_post_check_for("capture", SERIAL, PACKAGE))
        self.assertIsNone(smoke_wait.capture_post_check_for("zoom", SERIAL, PACKAGE))
        arguments = smoke_wait.parse_arguments(["--serial", SERIAL, "--since-epoch", "1.000", "--package", PACKAGE, "--scenario", "capture", "--timeout", "5", "--keep-captures"])
        self.assertTrue(arguments.keep_captures)


FAKE_ADB = r"""#!/usr/bin/env bash
printf '%s\n' "$*" >> "$FAKE_ADB_LOG"
if [[ "${1:-}" == "-s" ]]; then shift 2; fi
case "${1:-}" in
    get-state) echo device ;;
    install) echo Success ;;
    logcat) cat "$FAKE_LOGCAT_FILE" ;;
    exec-out)
        id="${5##*/}"
        cat "$FAKE_FILES/$id" ;;
    shell)
        case "${2:-}" in
            date) echo 1000.123456789 ;;
            content)
                id="${5##*/}"
                case "${3:-}" in
                    query)
                        if [[ -e "$FAKE_STATE/deleted_$id" || ! -e "$FAKE_ROWS/$id" ]]; then echo "No result found."; else cat "$FAKE_ROWS/$id"; fi ;;
                    delete) : > "$FAKE_STATE/deleted_$id" ;;
                esac ;;
            am)
                if [[ "${3:-}" == "broadcast" ]]; then
                    printf 'Broadcast completed: result=0, data="%s"\n' "$(cat "$FAKE_DECODE_FILE")"
                fi ;;
        esac ;;
esac
exit 0
"""

HEIF_INFO_STUB = """#!/usr/bin/env bash
echo "image: 4096x3072 (id=49), primary"
echo "  tiles: 8x6, tile size: 512x512"
"""


class DeviceSmokeCaptureTest(unittest.TestCase):
    """``device-smoke.sh capture`` на подставном adb с состоянием (строки MediaStore, файлы, удалённое)."""

    def setUp(self) -> None:
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.work = Path(temp.name)
        self.bin = self.work / "bin"
        for directory in ("bin", "rows", "files", "state", "home"):
            (self.work / directory).mkdir()
        for name, text in (("adb", FAKE_ADB), ("heif-info", HEIF_INFO_STUB)):
            path = self.bin / name
            path.write_text(text, encoding="utf-8")
            path.chmod(path.stat().st_mode | stat.S_IEXEC)
        self.log = self.work / "adb.log"
        self.log.write_text("", encoding="utf-8")
        self.apk = self.work / "app.apk"
        self.apk.write_bytes(b"")
        self.logcat = self.work / "logcat.txt"
        self.decode = self.work / "decode.json"
        self.decode.write_text(json.dumps({"status": "decoded", "width": 4096, "height": 3072, "color_space": "sRGB", "bitmap_config": "ARGB_8888"}), encoding="utf-8")
        self.report = capture_report()
        for expectation in files.expectations_of(self.report):
            key = expectation.uri.rsplit("/", 1)[-1]
            mime = "image/x-adobe-dng" if expectation.kind == "raw" else "image/heic"
            (self.work / "rows" / key).write_text(
                f"Row: 0 _display_name=IMG_20261122_101530_{int(key) % 1000:03d}.HEIC, mime_type={mime}, width=4096, height=3072, relative_path=DCIM/Camera/, is_pending=0, owner_package_name={PACKAGE}\n",
                encoding="utf-8",
            )
            (self.work / "files" / key).write_bytes(tiff_bytes([254, 50706]) if expectation.kind == "raw" else b"HEIC")

    def run_smoke(self, payload: dict, *extra: str) -> subprocess.CompletedProcess[str]:
        self.logcat.write_text(result_line(payload), encoding="utf-8")
        environment = dict(os.environ)
        environment.update(
            PATH=f"{self.bin}{os.pathsep}{environment['PATH']}",
            HOME=str(self.work / "home"),  # без ~/.cache/fasticamera: exiftool и rawpy «не найдены», проверка hermetic
            ANDROID_SERIAL=SERIAL,
            FASTICAMERA_APK_PATH=str(self.apk),
            FAKE_ADB_LOG=str(self.log),
            FAKE_LOGCAT_FILE=str(self.logcat),
            FAKE_ROWS=str(self.work / "rows"),
            FAKE_FILES=str(self.work / "files"),
            FAKE_STATE=str(self.work / "state"),
            FAKE_DECODE_FILE=str(self.decode),
        )
        return subprocess.run(
            ["bash", str(SCRIPTS_DIR / "device-smoke.sh"), "--no-build", "--timeout", "20", *extra, "capture"],
            capture_output=True, text=True, env=environment, timeout=60,
        )

    def deletes(self) -> list[str]:
        return [line.split("--uri ")[-1] for line in self.log.read_text(encoding="utf-8").splitlines() if "shell content delete" in line]

    def test_a_healthy_run_is_verified_and_its_files_are_deleted(self) -> None:
        completed = self.run_smoke(passed(self.report))
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(sorted(self.deletes()), sorted(self.report["uris"]))
        self.assertIn("удалено файлов 13 из 13", completed.stdout)
        self.assertIn("плитки 8x6", completed.stdout)

    def test_keep_captures_leaves_the_files_and_prints_their_addresses(self) -> None:
        completed = self.run_smoke(passed(self.report), "--keep-captures")
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(self.deletes(), [])
        self.assertIn(self.report["uris"][0], completed.stdout)

    def test_a_pending_file_fails_the_run_but_the_files_are_still_removed(self) -> None:
        key = self.report["uris"][0].rsplit("/", 1)[-1]
        row = self.work / "rows" / key
        row.write_text(row.read_text(encoding="utf-8").replace("is_pending=0", "is_pending=1"), encoding="utf-8")
        completed = self.run_smoke(passed(self.report))
        self.assertEqual(completed.returncode, 1, completed.stdout)
        self.assertIn("is_pending=1", completed.stderr)
        self.assertEqual(len(self.deletes()), 13)

    def test_a_failed_scenario_with_files_still_cleans_them(self) -> None:
        payload = {"scenario": "capture", "status": "failed", "error": "серия: сорвалось снимков 1", "report": self.report}
        completed = self.run_smoke(payload)
        self.assertEqual(completed.returncode, 1, completed.stdout)
        self.assertEqual(len(self.deletes()), 13)

    def test_keep_captures_is_only_for_capture(self) -> None:
        completed = subprocess.run(
            ["bash", str(SCRIPTS_DIR / "device-smoke.sh"), "--no-build", "--keep-captures", "zoom"],
            capture_output=True, text=True, timeout=30, env={**os.environ, "ANDROID_SERIAL": SERIAL},
        )
        self.assertEqual(completed.returncode, 64, completed.stderr)
        self.assertIn("только для capture", completed.stderr)

    def test_the_usage_header_documents_capture_and_the_flag(self) -> None:
        header = (SCRIPTS_DIR / "device-smoke.sh").read_text(encoding="utf-8").split("set -euo pipefail", 1)[0]
        self.assertIn("capture (150;", header)
        self.assertIn("--keep-captures", header)
        completed = subprocess.run(["bash", str(SCRIPTS_DIR / "device-smoke.sh"), "--help"], capture_output=True, text=True, timeout=30)
        self.assertIn("keep-captures", completed.stderr)


if __name__ == "__main__":
    unittest.main()
