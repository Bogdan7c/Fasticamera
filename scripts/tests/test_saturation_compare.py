"""Сессия 16a: сравнение двух сохранённых прогонов `gpu-saturation-bench` («база → новое»): таблица мс/кадр, изменение в %,
дайджест выхода совпал / различается / недоступен. Фикстуры повторяют то, что печатает `device-smoke.sh`
(`FASTICAMERA_DETAIL` стадии `detail_rows` и `FASTICAMERA_RESULT`)."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_checks_saturation as saturation  # noqa: E402  (путь к scripts/ добавлен строкой выше)

DIGEST_A = "0123456789abcdef"
DIGEST_B = "fedcba9876543210"


def computed(combined: str, parts: dict[str, str] | None = None, note: str | None = None) -> dict:
    return {
        "state": "computed",
        "combined": combined,
        "parts": [{"label": label, "digest": value} for label, value in (parts or {"image": combined}).items()],
        "frame_note": note,
    }


def item(name: str, cost_ms: float | None, digest: dict | None, state: str = "measured") -> dict:
    """Строка работы в том виде, в каком её сериализует `detail_item` (Rust)."""
    outcome: dict = {"state": state}
    if cost_ms is not None:
        outcome["measurement"] = {"frame_cost_ms": cost_ms, "frames_per_second": 1000.0 / cost_ms}
    return {"name": name, "index": 1, "total": 1, "outcome": outcome, "digest": digest}


def detail_text(*items: dict) -> str:
    stage = {"scenario": "gpu-saturation-bench", "stage": "detail_rows", "part": 1, "parts": 1, "items": list(items)}
    return f"FASTICAMERA_DETAIL {json.dumps(stage, ensure_ascii=False)}\n"


def result_text(table: list[str], digests: list[str] | None = None) -> str:
    report: dict = {"table": table, "requested": [row.split("|")[0] for row in table]}
    if digests is not None:
        report["digests"] = digests
    payload = {"scenario": "gpu-saturation-bench", "status": "passed", "report": report}
    return f"  1792957062.000  4242  4242 I Fasticamera: FASTICAMERA_RESULT {json.dumps(payload)}\n"


def compare(text_a: str, text_b: str) -> dict[str, saturation.RowComparison]:
    rows = saturation.compare_runs(saturation.parse_saturation_rows(text_a), saturation.parse_saturation_rows(text_b))
    return {row.name: row for row in rows}


class ParseTest(unittest.TestCase):
    def test_detail_rows_give_cost_digest_parts_and_frame_note_in_run_order(self) -> None:
        text = "шум\n" + detail_text(
            item("viewfinder", 6.67, computed(DIGEST_A, {"image": DIGEST_A}, "last_frame=awb_statistics")),
            item("still", 98.0, computed(DIGEST_B, {"image": DIGEST_A, "thumbnail": DIGEST_B})),
        )
        rows = saturation.parse_saturation_rows(text)
        self.assertEqual(list(rows), ["viewfinder", "still"])
        self.assertEqual(rows["viewfinder"].frame_cost_ms, 6.67)
        self.assertEqual(rows["viewfinder"].digest, DIGEST_A)
        self.assertEqual(rows["viewfinder"].frame_note, "last_frame=awb_statistics")
        self.assertEqual(rows["still"].digest_parts, {"image": DIGEST_A, "thumbnail": DIGEST_B})

    def test_result_line_table_and_digests_are_read_when_there_are_no_details(self) -> None:
        text = result_text(["viewfinder|150|6.67|ok", "still|10.2|98.0|ok"], [f"viewfinder|{DIGEST_A}", "still|-"])
        rows = saturation.parse_saturation_rows(text)
        self.assertEqual(rows["viewfinder"].frame_cost_ms, 6.67)
        self.assertEqual(rows["viewfinder"].digest, DIGEST_A)
        self.assertIsNone(rows["still"].digest)

    def test_old_result_without_digests_parses_and_garbage_is_ignored(self) -> None:
        rows = saturation.parse_saturation_rows("мусор\nFASTICAMERA_DETAIL {не json\n" + result_text(["viewfinder|150|6.67|ok"]))
        self.assertEqual(list(rows), ["viewfinder"])
        self.assertIsNone(rows["viewfinder"].digest)
        self.assertEqual(saturation.parse_saturation_rows("ничего полезного"), {})

    def test_details_win_over_the_result_table_for_the_same_work(self) -> None:
        text = result_text(["viewfinder|150|6.67|ok"]) + detail_text(item("viewfinder", 6.6667, computed(DIGEST_A)))
        self.assertEqual(saturation.parse_saturation_rows(text)["viewfinder"].frame_cost_ms, 6.6667)


class CompareTest(unittest.TestCase):
    def test_same_digest_and_faster_cost_is_reported_with_the_percent_change(self) -> None:
        rows = compare(
            detail_text(item("display_calibrate", 8.0, computed(DIGEST_A))),
            detail_text(item("display_calibrate", 6.0, computed(DIGEST_A))),
        )
        row = rows["display_calibrate"]
        self.assertEqual(row.digest_status, saturation.DIGEST_SAME)
        self.assertAlmostEqual(row.change_percent, -25.0)
        self.assertEqual((row.cost_a_ms, row.cost_b_ms), (8.0, 6.0))

    def test_different_digest_names_the_parts_that_changed(self) -> None:
        a = computed(DIGEST_A, {"counters": DIGEST_A, "picture": DIGEST_A})
        b = computed(DIGEST_B, {"counters": DIGEST_A, "picture": DIGEST_B})
        row = compare(detail_text(item("scopes_waveform", 1.0, a)), detail_text(item("scopes_waveform", 1.0, b)))["scopes_waveform"]
        self.assertEqual(row.digest_status, saturation.DIGEST_DIFFERENT)
        self.assertEqual(row.detail, "части: picture")

    def test_unavailable_or_absent_digest_is_reported_as_unavailable_not_as_an_error(self) -> None:
        unavailable = {"state": "unavailable", "reason": "нет доступа к счётчикам"}
        rows = compare(
            detail_text(item("scopes_waveform", 1.0, unavailable), item("viewfinder", 6.0, computed(DIGEST_A))),
            result_text(["scopes_waveform|1000|1.00|ok", "viewfinder|150|6.00|ok"]),  # старый формат без digests
        )
        self.assertEqual(rows["scopes_waveform"].digest_status, saturation.DIGEST_UNAVAILABLE)
        self.assertIn("нет доступа к счётчикам", rows["scopes_waveform"].detail)
        self.assertEqual(rows["viewfinder"].digest_status, saturation.DIGEST_UNAVAILABLE)
        self.assertIn("дайджеста нет в прогоне", rows["viewfinder"].detail)

    def test_a_work_present_in_one_run_only_is_listed_with_its_side(self) -> None:
        rows = compare(
            detail_text(item("viewfinder", 6.0, computed(DIGEST_A))),
            detail_text(item("viewfinder", 6.0, computed(DIGEST_A)), item("viewfinder_minimal", 2.0, computed(DIGEST_B))),
        )
        self.assertEqual(rows["viewfinder_minimal"].digest_status, "нет в A")
        self.assertEqual(rows["viewfinder_minimal"].cost_b_ms, 2.0)
        self.assertIsNone(rows["viewfinder_minimal"].change_percent)

    def test_equal_digests_of_different_awb_frames_carry_a_warning(self) -> None:
        a = computed(DIGEST_A, note="last_frame=awb_statistics")
        b = computed(DIGEST_A, note="last_frame=no_awb_statistics")
        row = compare(detail_text(item("viewfinder", 6.0, a)), detail_text(item("viewfinder", 6.0, b)))["viewfinder"]
        self.assertEqual(row.digest_status, saturation.DIGEST_SAME)
        self.assertIn("читали разные кадры", row.detail)

    def test_failed_row_without_cost_has_no_percent(self) -> None:
        rows = compare(
            detail_text(item("still", 98.0, computed(DIGEST_A))),
            detail_text(item("still", None, None, state="failed")),
        )
        self.assertIsNone(rows["still"].change_percent)
        self.assertEqual(rows["still"].digest_status, saturation.DIGEST_UNAVAILABLE)

    def test_table_text_has_every_work_cost_percent_and_digest_status(self) -> None:
        rows = saturation.compare_runs(
            saturation.parse_saturation_rows(detail_text(item("viewfinder", 6.67, computed(DIGEST_A)))),
            saturation.parse_saturation_rows(detail_text(item("viewfinder", 5.0, computed(DIGEST_B)))),
        )
        text = saturation.format_comparison(rows)
        self.assertIn("работа", text.splitlines()[0])
        line = text.splitlines()[1]
        for expected in ("viewfinder", "6.67", "5", "-25.0%", saturation.DIGEST_DIFFERENT):
            self.assertIn(expected, line)


class CommandLineTest(unittest.TestCase):
    def test_cli_prints_the_table_and_reports_an_empty_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base, new, empty = (Path(directory) / name for name in ("a.txt", "b.txt", "empty.txt"))
            base.write_text(detail_text(item("viewfinder", 6.0, computed(DIGEST_A))), encoding="utf-8")
            new.write_text(detail_text(item("viewfinder", 3.0, computed(DIGEST_A))), encoding="utf-8")
            empty.write_text("пусто\n", encoding="utf-8")
            script = str(SCRIPTS_DIR / "smoke_checks_saturation.py")
            ok = subprocess.run([sys.executable, script, "compare", str(base), str(new)], capture_output=True, text=True)
            self.assertEqual(ok.returncode, 0, ok.stderr)
            self.assertIn("-50.0%", ok.stdout)
            self.assertIn(saturation.DIGEST_SAME, ok.stdout)
            bad = subprocess.run([sys.executable, script, "compare", str(base), str(empty)], capture_output=True, text=True)
            self.assertEqual(bad.returncode, 2)
            self.assertIn("нет строк", bad.stderr)


class DigestCheckTest(unittest.TestCase):
    def report(self, digests: list[str] | None) -> dict:
        base = {
            "requested": ["viewfinder"],
            "table": ["viewfinder|150|6.67|ok"],
            "measured": 1,
            "cancelled": False,
            "camera": "closed_by_scenario",
            "render": {"state": "render_unavailable", "reason": "тест"},
        }
        if digests is not None:
            base["digests"] = digests
        return {"scenario": "gpu-saturation-bench", "status": "passed", "report": base}

    def test_digests_are_optional_but_must_be_well_formed_when_present(self) -> None:
        check = saturation.check_gpu_saturation_bench
        self.assertEqual(check(self.report(None)), [])
        self.assertEqual(check(self.report([f"viewfinder|{DIGEST_A}"])), [])
        self.assertEqual(check(self.report(["viewfinder|-"])), [])
        self.assertTrue(any("не в формате" in text for text in check(self.report(["viewfinder|xyz"]))))
        self.assertTrue(any("которую не просили" in text for text in check(self.report([f"still|{DIGEST_A}"]))))

    def test_note_counts_works_without_a_digest(self) -> None:
        notes = saturation.saturation_notes(self.report(["viewfinder|-"]))
        self.assertTrue(any("недоступен у 1 работ из 1" in text for text in notes), notes)


if __name__ == "__main__":
    unittest.main()
