"""Тесты проверки ровности поля нейтральной карты (``smoke_neutral_card``, секция ``[neutral_card.flatness]``).

Синтетические скриншоты 110x240 px (масштаб 0,25 px/pt): плоское поле, виньетка по четырём углам кадра,
один «плохой» угол, элемент интерфейса в зоне зума. Углы затемняются зонами «угловая четверть кадра», не
через ``corner_origins``, чтобы тест не повторял проверяемую геометрию."""

from __future__ import annotations

import re
import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_neutral_card  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import smoke_screen  # noqa: E402
from test_smoke_screen import parsed  # noqa: E402
from test_smoke_viewfinder import THRESHOLDS  # noqa: E402

WIDTH_PX, HEIGHT_PX = 110, 240
SCALE = WIDTH_PX / THRESHOLDS.flatness.screen_width_pt
FRAME_TOP_PX = round(THRESHOLDS.flatness.frame_top_pt * SCALE)
FRAME_BOTTOM_PX = round(THRESHOLDS.flatness.frame_bottom_pt * SCALE)
QUARTER_PX = WIDTH_PX // 4
# Яркость среднего серого цели; поле этой яркости проходит проверку уровня.
MIDDLE_LUMINANCE = THRESHOLDS.display_middle_gray
CORNERS = ("левый верхний", "правый верхний", "левый нижний", "правый нижний")


def gray_with_luminance(luminance: float) -> tuple[int, int, int]:
    """Серый (R=G=B) пиксель заданной линейной яркости (сумма коэффициентов P3 = 1, поэтому Y = линейный канал)."""
    level = round(smoke_neutral_card.srgb_linear_to_encoded(luminance) * 255)
    return (level, level, level)


def field(corner_ev: dict[str, float] | None = None, uniform_ev: float = 0.0):
    """Плоское поле среднего серого; ``corner_ev`` меняет яркость угловой четверти кадра на заданные EV."""
    deviations = corner_ev or {}

    def pixel_at(x: int, y: int) -> tuple[int, int, int]:
        ev = uniform_ev
        in_left, in_right = x < QUARTER_PX, x >= WIDTH_PX - QUARTER_PX
        in_top = FRAME_TOP_PX <= y < FRAME_TOP_PX + QUARTER_PX
        in_bottom = FRAME_BOTTOM_PX - QUARTER_PX <= y < FRAME_BOTTOM_PX
        for name, (horizontal, vertical) in zip(
            CORNERS, ((in_left, in_top), (in_right, in_top), (in_left, in_bottom), (in_right, in_bottom))
        ):
            if horizontal and vertical:
                ev += deviations.get(name, 0.0)
        return gray_with_luminance(MIDDLE_LUMINANCE * 2.0**ev)

    return pixel_at


def analyze(pixel_at, thresholds=THRESHOLDS) -> smoke_screen.ScreenAnalysis:
    screenshot = parsed(pixel_at, width=WIDTH_PX, height=HEIGHT_PX)
    return smoke_neutral_card.analyze_neutral_card(screenshot, thresholds, "viewfinder_a")


class FlatnessAnalysisTest(unittest.TestCase):
    def test_flat_field_passes_and_reports_every_corner(self) -> None:
        analysis = analyze(field())
        self.assertEqual(analysis.problems, [])
        report = analysis.notes[1]
        for name in CORNERS:
            self.assertRegex(report, re.escape(name) + r" [+-]0\.0\d")
        self.assertRegex(report, r"среднее \|EV\| 0\.0\d")

    def test_vignetted_corners_fail_naming_each_corner_and_the_mean(self) -> None:
        analysis = analyze(field({name: -1.0 for name in CORNERS}))
        corner_problems = [p for p in analysis.problems if "угол" in p]
        self.assertEqual(len(corner_problems), 4, analysis.problems)
        for name in CORNERS:
            self.assertTrue(any(name in p and re.search(r"-(0\.9\d|1\.0\d) EV", p) and "темнее" in p for p in corner_problems), name)
        self.assertTrue(any("среднее |EV|" in p for p in analysis.problems), analysis.problems)
        self.assertRegex(analysis.notes[1], r"левый верхний -(0\.9\d|1\.0\d)")

    def test_single_bad_corner_fails_naming_only_that_corner(self) -> None:
        analysis = analyze(field({"правый нижний": -0.5}))
        self.assertEqual(len(analysis.problems), 1, analysis.problems)
        self.assertRegex(analysis.problems[0], r"правый нижний угол -0\.5\d EV")
        for other in CORNERS[:3]:
            self.assertNotIn(other, analysis.problems[0])

    def test_brighter_corner_is_reported_as_lighter(self) -> None:
        analysis = analyze(field({"левый верхний": 0.5}))
        self.assertEqual(len(analysis.problems), 1, analysis.problems)
        self.assertRegex(analysis.problems[0], r"левый верхний угол \+0\.[45]\d EV")
        self.assertIn("светлее", analysis.problems[0])

    def test_small_single_corner_deviation_within_tolerance_passes(self) -> None:
        self.assertEqual(analyze(field({"левый нижний": -0.2})).problems, [])

    def test_uniform_undercorrection_trips_only_the_mean_limit(self) -> None:
        # Каждый угол на -0.2 EV — в допуске на угол (0.25), но среднее 0.2 больше допуска 0.15.
        analysis = analyze(field({name: -0.2 for name in CORNERS}))
        self.assertEqual(len(analysis.problems), 1, analysis.problems)
        self.assertIn("среднее |EV|", analysis.problems[0])

    def test_black_corner_fails_instead_of_crashing(self) -> None:
        analysis = analyze(field({"правый верхний": -12.0}))
        self.assertTrue(any("правый верхний угол" in p for p in analysis.problems), analysis.problems)

    def test_zoom_row_ui_in_the_middle_of_the_frame_bottom_is_not_measured(self) -> None:
        flat = field()

        def with_zoom_pill(x: int, y: int) -> tuple[int, int, int]:
            # Яркая пилюля зума по центру нижней части кадра: между левым и правым нижними углами.
            if WIDTH_PX // 2 - 15 <= x < WIDTH_PX // 2 + 15 and FRAME_BOTTOM_PX - 12 <= y < FRAME_BOTTOM_PX - 2:
                return (255, 255, 255)
            return flat(x, y)

        self.assertEqual(analyze(with_zoom_pill).problems, [])

    def test_level_and_chroma_checks_are_independent_of_flatness(self) -> None:
        tinted = analyze(lambda _x, _y: (140, 120, 100))
        self.assertEqual(len(tinted.problems), 1, tinted.problems)
        self.assertIn("не серая", tinted.problems[0])
        too_bright = analyze(lambda _x, _y: (160, 160, 160))
        self.assertEqual(len(too_bright.problems), 1, too_bright.problems)
        self.assertIn("светлее цели", too_bright.problems[0])

    def test_dark_screen_is_not_judged_for_flatness(self) -> None:
        analysis = analyze(lambda _x, _y: (5, 5, 5))
        self.assertEqual(len(analysis.problems), 1, analysis.problems)
        self.assertIn("тёмный", analysis.problems[0])
        self.assertEqual(len(analysis.notes), 1)

    def test_frame_outside_the_screenshot_is_reported_without_losing_other_verdicts(self) -> None:
        short = parsed(lambda _x, _y: (140, 120, 100), width=WIDTH_PX, height=HEIGHT_PX // 3)
        analysis = smoke_neutral_card.analyze_neutral_card(short, THRESHOLDS, "viewfinder_a")
        self.assertTrue(any("не серая" in p for p in analysis.problems), analysis.problems)
        self.assertTrue(any("ровность поля не измерена" in p for p in analysis.problems), analysis.problems)


class FlatnessThresholdsTest(unittest.TestCase):
    def test_real_toml_carries_the_flatness_section(self) -> None:
        loaded = smoke_neutral_card.load_thresholds().flatness
        self.assertEqual((loaded.max_corner_ev, loaded.max_mean_ev), (0.25, 0.15))
        self.assertLess(loaded.frame_top_pt, loaded.frame_bottom_pt)
        # Кадр 4:3 на всю ширину экрана: высота = 4/3 ширины.
        self.assertAlmostEqual(loaded.frame_bottom_pt - loaded.frame_top_pt, loaded.screen_width_pt * 4 / 3, delta=0.1)

    def test_missing_flatness_section_names_the_key(self) -> None:
        text = smoke_screen.THRESHOLDS_FILE.read_text(encoding="utf-8").split("[neutral_card.flatness]")[0]
        with self.assertRaises(smoke_screen.ScreenError) as raised:
            smoke_neutral_card.parse_thresholds(text)
        self.assertIn("flatness", str(raised.exception))

    def test_overlapping_corner_regions_are_rejected(self) -> None:
        with self.assertRaises(smoke_screen.ScreenError):
            smoke_neutral_card.FlatnessThresholds(
                screen_width_pt=440.0, frame_top_pt=186.0, frame_bottom_pt=772.7,
                corner_inset_fraction=0.3, corner_region_fraction=0.3, max_corner_ev=0.25, max_mean_ev=0.15,
            )

    def test_corner_origins_stay_inside_the_frame(self) -> None:
        screenshot = parsed(lambda _x, _y: (0, 0, 0), width=WIDTH_PX, height=HEIGHT_PX)
        origins, side = smoke_neutral_card.corner_origins(screenshot, THRESHOLDS.flatness)
        for left, top in origins:
            self.assertGreaterEqual(left, 0)
            self.assertLessEqual(left + side, WIDTH_PX)
            self.assertGreaterEqual(top, FRAME_TOP_PX)
            self.assertLessEqual(top + side, FRAME_BOTTOM_PX)


if __name__ == "__main__":
    unittest.main()
