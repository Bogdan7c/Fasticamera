"""14o: разбор строк лога рецепта вида (`scripts/look_recipe_log.py`) на синтетическом тексте logcat."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import look_recipe_log  # noqa: E402  (путь к scripts/ добавлен строкой выше)

SETTINGS = json.dumps({"contrast": 0.25, "bands": [], "wheels": {}}, separators=(",", ":"))
PREFIX = "11-27 12:00:00.123  4242  4343 I fasticamera::look: "


def recipe(origin: str, fp: str, base: str = "agx") -> str:
    return f"{PREFIX}look_recipe origin={origin} fp={fp} base={base} settings={SETTINGS}"


def press(fp: str, view_fp: str | None = None, clean: str = "show_edited", claim: str = "free") -> str:
    return f"{PREFIX}look_still_press fp={fp} view_fp={view_fp or fp} clean_view={clean} scenario_claim={claim}"


def render(fp: str) -> str:
    return f"{PREFIX}look_still_render fp={fp}"


A, B, C = "aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb", "cccccccccccccccc"
NOISE = "11-27 12:00:00.200  1  2 D other: not about the look"


class ParseTest(unittest.TestCase):
    def test_events_are_read_with_their_fields_and_noise_is_skipped(self) -> None:
        events = look_recipe_log.parse_lines([NOISE, recipe("startup", A, "aces"), press(A, B, "show_base", "held"), render(A)])
        self.assertEqual(len(events), 3)
        first, second, third = events
        self.assertEqual((first.origin, first.fingerprint, first.base, first.settings["contrast"]), ("startup", A, "aces", 0.25))
        self.assertEqual((second.fingerprint, second.view_fingerprint, second.clean_view, second.scenario_claim), (A, B, "show_base", "held"))
        self.assertEqual(third.fingerprint, A)

    def test_a_broken_look_line_is_an_error_and_never_silently_skipped(self) -> None:
        for broken in (
            f"{PREFIX}look_recipe origin=startup fp=XYZ base=agx settings={{}}",
            f"{PREFIX}look_recipe origin=startup fp={A} base=reinhard settings={{}}",
            f"{PREFIX}look_recipe origin=startup fp={A} base=agx settings={{не json}}",
            f"{PREFIX}look_still_press fp={A}",
            f"{PREFIX}look_still_render fp=zz",
        ):
            with self.subTest(line=broken):
                self.assertRaises(look_recipe_log.LogParseError, look_recipe_log.parse_lines, [broken])
                problems = look_recipe_log.check_log([broken])
                self.assertEqual(len(problems), 1)


class StillMatchesViewfinderTest(unittest.TestCase):
    def check(self, *lines: str, **options: bool) -> list[str]:
        return look_recipe_log.check_log(list(lines), **options)

    def test_a_press_that_matches_the_viewfinder_and_the_last_product_recipe_passes(self) -> None:
        self.assertEqual(self.check(recipe("startup", A), recipe("edit_settled", B), press(B), render(B)), [])

    def test_a_press_after_an_edit_flushed_before_the_still_passes(self) -> None:
        self.assertEqual(self.check(recipe("startup", A), recipe("edit_before_still", B), press(B), render(B)), [])

    def test_the_still_look_differs_from_the_viewfinder(self) -> None:
        problems = self.check(recipe("startup", A), press(A, view_fp=B), render(A))
        self.assertEqual(len(problems), 1)
        self.assertIn(f"видоискатель показывал {B}", problems[0])

    def test_the_still_look_differs_from_the_last_committed_product_recipe(self) -> None:
        problems = self.check(recipe("startup", A), recipe("preset_changed", B), press(A), render(A))
        self.assertEqual(len(problems), 1)
        self.assertIn(f"последний записанный вид продукта {B}", problems[0])

    def test_an_unknown_render_look_is_a_problem(self) -> None:
        problems = self.check(recipe("startup", A), press(A, view_fp="unknown"), render(A))
        self.assertTrue(any("неизвестно" in problem for problem in problems), problems)

    def test_the_clean_view_pressed_does_not_need_the_viewfinder_to_match_but_the_product_still_must(self) -> None:
        self.assertEqual(self.check(recipe("startup", A), press(A, view_fp=C, clean="show_base"), render(A)), [])
        problems = self.check(recipe("startup", A), press(B, view_fp=C, clean="show_base"), render(B))
        self.assertEqual(len(problems), 1)

    def test_a_scenario_claim_excuses_the_viewfinder_comparison_only(self) -> None:
        lines = [recipe("startup", A), recipe("scenario_claim", C, "legacy")]
        self.assertEqual(self.check(*lines, press(A, view_fp=C, claim="held"), render(A)), [])
        # Пока сценарий держит вид, снимок несёт вид продукта, а видоискатель показывает вид сценария: сверять нечего.
        self.assertEqual(self.check(*lines, press(B, view_fp=C, claim="held"), render(B)), [])

    def test_the_scenario_release_returns_the_product_recipe_as_the_reference(self) -> None:
        lines = [recipe("startup", A), recipe("scenario_claim", C), recipe("scenario_release", A)]
        self.assertEqual(self.check(*lines, press(A), render(A)), [])
        self.assertEqual(len(self.check(*lines, press(C), render(C))), 1, "вид сценария — не вид продукта")

    def test_a_scenario_look_never_counts_as_the_product_recipe(self) -> None:
        problems = self.check(recipe("startup", A), recipe("scenario_claim", C), press(C), render(C))
        self.assertTrue(any(f"вид продукта {A}" in problem for problem in problems), problems)

    def test_the_developing_look_must_equal_the_pressed_one(self) -> None:
        problems = self.check(recipe("startup", A), press(A), render(B))
        self.assertEqual(len(problems), 1)
        self.assertIn(f"проявлен видом {B}", problems[0])

    def test_a_press_without_a_render_line_is_reported_unless_allowed(self) -> None:
        lines = [recipe("startup", A), press(A)]
        self.assertTrue(any("не записал вид проявки" in problem for problem in self.check(*lines)))
        self.assertEqual(self.check(*lines, require_render_line=False), [])

    def test_a_press_before_any_recipe_and_a_log_without_recipes_or_presses_fail(self) -> None:
        self.assertTrue(any("ни одного вида продукта" in p for p in self.check(press(A), render(A), recipe("scenario_claim", C))))
        self.assertTrue(any("нет ни одной строки look_recipe" in p for p in self.check(NOISE)))
        self.assertTrue(any("нет ни одного нажатия" in p for p in self.check(recipe("startup", A))))

    def test_an_unknown_origin_is_a_problem(self) -> None:
        problems = self.check(recipe("startup", A), recipe("mystery", B), press(A), render(A))
        self.assertTrue(any("mystery" in problem for problem in problems), problems)

    def test_several_stills_are_judged_each_against_the_state_at_its_own_press(self) -> None:
        lines = [recipe("startup", A), press(A), render(A), recipe("preset_changed", B), press(B), render(B)]
        self.assertEqual(self.check(*lines), [])
        bad = [*lines[:3], recipe("preset_changed", B), press(A), render(A)]
        self.assertEqual(len(self.check(*bad)), 1)


class CommandLineTest(unittest.TestCase):
    def run_cli(self, text: str, *arguments: str) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "logcat.txt"
            path.write_text(text, encoding="utf-8")
            return subprocess.run(
                [sys.executable, str(SCRIPTS_DIR / "look_recipe_log.py"), str(path), *arguments],
                capture_output=True,
                text=True,
                timeout=30,
            )

    def test_a_consistent_log_exits_zero_and_lists_the_recipes(self) -> None:
        text = "\n".join([recipe("startup", A), press(A), render(A)]) + "\n"
        completed = self.run_cli(text)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn(f"startup fp={A}", completed.stdout)

    def test_a_mismatch_exits_one_and_names_it_on_stderr(self) -> None:
        text = "\n".join([recipe("startup", A), press(B, view_fp=A), render(B)]) + "\n"
        completed = self.run_cli(text)
        self.assertEqual(completed.returncode, 1)
        self.assertIn("НАРУШЕНИЕ", completed.stderr)

    def test_a_missing_file_is_a_usage_error(self) -> None:
        completed = subprocess.run(
            [sys.executable, str(SCRIPTS_DIR / "look_recipe_log.py"), "/nonexistent/logcat.txt"],
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(completed.returncode, 2)


if __name__ == "__main__":
    unittest.main()
