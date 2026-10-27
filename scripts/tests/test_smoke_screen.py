"""Тесты анализа скриншотов видоискателя: разбор сырого ``screencap`` (оба размера заголовка), пороги,
«чёрный» / «замер» / «меняется», исключение верхней плашки, PNG и пост-проверка из каталога артефактов."""

from __future__ import annotations

import struct
import sys
import tempfile
import unittest
import zlib
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_screen  # noqa: E402  (путь к scripts/ добавлен строкой выше)

WIDTH, HEIGHT = 40, 100
"""Крошечный «экран»: полоса анализа 30..90 % — строки 30..89."""

THRESHOLDS = smoke_screen.ScreenThresholds(
    top_fraction=0.30,
    bottom_fraction=0.90,
    sample_step_pixels=1,
    min_mean_luminance=2.0,
    min_mean_abs_difference=0.1,
)


def screencap_bytes(
    pixel_at,
    *,
    header_with_colorspace: bool,
    width: int = WIDTH,
    height: int = HEIGHT,
    pixel_format: int = smoke_screen.PIXEL_FORMAT_RGBA_8888,
) -> bytes:
    """Сырой вывод ``screencap``: заголовок (12 или 16 байт) и пиксели ``pixel_at(x, y) -> (r, g, b)``."""
    header = struct.pack("<III", width, height, pixel_format)
    if header_with_colorspace:
        header += struct.pack("<I", 0)
    pixels = bytearray()
    for y in range(height):
        for x in range(width):
            red, green, blue = pixel_at(x, y)
            pixels += bytes((red, green, blue, 255))
    return header + bytes(pixels)


def parsed(pixel_at, **kwargs) -> smoke_screen.Screenshot:
    kwargs.setdefault("header_with_colorspace", True)
    return smoke_screen.parse_screencap(screencap_bytes(pixel_at, **kwargs))


def black(_x: int, _y: int) -> tuple[int, int, int]:
    return (0, 0, 0)


def gray(level: int):
    return lambda _x, _y: (level, level, level)


def noisy(seed: int):
    """Детерминированный «шум сцены»: уровень зависит от координат и seed."""
    return lambda x, y: (((x * 7 + y * 13 + seed * 29) % 40) + 20,) * 3


class ParseScreencapTest(unittest.TestCase):
    def test_twelve_byte_header_without_colorspace(self) -> None:
        shot = parsed(gray(50), header_with_colorspace=False)
        self.assertEqual((shot.width, shot.height, shot.header_size), (WIDTH, HEIGHT, 12))
        self.assertEqual(len(shot.rgba), WIDTH * HEIGHT * 4)

    def test_sixteen_byte_header_with_colorspace(self) -> None:
        shot = parsed(gray(50), header_with_colorspace=True)
        self.assertEqual(shot.header_size, 16)
        self.assertEqual(shot.rgba[:4], bytes((50, 50, 50, 255)), "пиксели начинаются сразу после заголовка")

    def test_both_header_sizes_give_identical_pixels(self) -> None:
        old = parsed(noisy(1), header_with_colorspace=False)
        new = parsed(noisy(1), header_with_colorspace=True)
        self.assertEqual(old.rgba, new.rgba)

    def test_bgra_is_converted_to_rgba(self) -> None:
        data = struct.pack("<III", 1, 1, smoke_screen.PIXEL_FORMAT_BGRA_8888) + bytes((10, 20, 30, 255))
        shot = smoke_screen.parse_screencap(data)
        self.assertEqual(shot.rgba, bytes((30, 20, 10, 255)), "синий и красный переставлены")

    def test_truncated_output_is_an_error_not_a_guess(self) -> None:
        data = screencap_bytes(gray(10), header_with_colorspace=True)
        with self.assertRaisesRegex(smoke_screen.ScreenError, "не сходится"):
            smoke_screen.parse_screencap(data[:-5])

    def test_extra_trailing_bytes_are_an_error(self) -> None:
        data = screencap_bytes(gray(10), header_with_colorspace=True)
        with self.assertRaisesRegex(smoke_screen.ScreenError, "остаток"):
            smoke_screen.parse_screencap(data + b"\x00\x00")

    def test_unknown_pixel_format_is_reported(self) -> None:
        data = struct.pack("<III", 1, 1, 99) + b"\x00" * 4
        with self.assertRaisesRegex(smoke_screen.ScreenError, "не поддерживается"):
            smoke_screen.parse_screencap(data)

    def test_absurd_dimensions_mean_this_is_not_a_frame(self) -> None:
        with self.assertRaisesRegex(smoke_screen.ScreenError, "нелепый размер"):
            smoke_screen.parse_screencap(struct.pack("<III", 0, 5, 1) + b"\x00" * 100)

    def test_too_short_output(self) -> None:
        with self.assertRaisesRegex(smoke_screen.ScreenError, "слишком короткий"):
            smoke_screen.parse_screencap(b"\x00\x01")


class MeasuresTest(unittest.TestCase):
    def test_luminance_uses_rec709_weights_in_integers(self) -> None:
        self.assertEqual(smoke_screen.mean_luminance(parsed(gray(100)), THRESHOLDS), 100 * 256 // 256)
        green = parsed(lambda _x, _y: (0, 255, 0))
        self.assertEqual(smoke_screen.mean_luminance(green, THRESHOLDS), (183 * 255) >> 8)

    def test_status_overlay_above_the_band_is_ignored(self) -> None:
        # Верхние 25 % — яркая плашка Kotlin, полоса анализа чёрная.
        shot = parsed(lambda _x, y: (255, 255, 255) if y < 25 else (0, 0, 0))
        self.assertEqual(smoke_screen.mean_luminance(shot, THRESHOLDS), 0.0)

    def test_navigation_area_below_the_band_is_ignored(self) -> None:
        shot = parsed(lambda _x, y: (255, 255, 255) if y >= 92 else (0, 0, 0))
        self.assertEqual(smoke_screen.mean_luminance(shot, THRESHOLDS), 0.0)

    def test_difference_of_identical_shots_is_exactly_zero(self) -> None:
        self.assertEqual(smoke_screen.mean_abs_difference(parsed(noisy(3)), parsed(noisy(3)), THRESHOLDS), 0.0)

    def test_difference_is_the_mean_absolute_luminance_gap(self) -> None:
        self.assertEqual(smoke_screen.mean_abs_difference(parsed(gray(10)), parsed(gray(14)), THRESHOLDS), 4.0)

    def test_shots_of_different_size_cannot_be_compared(self) -> None:
        other = parsed(gray(10), width=20)
        with self.assertRaisesRegex(smoke_screen.ScreenError, "разного размера"):
            smoke_screen.mean_abs_difference(parsed(gray(10)), other, THRESHOLDS)

    def test_sampling_step_still_sees_the_whole_band(self) -> None:
        coarse = smoke_screen.ScreenThresholds(0.30, 0.90, 4, 2.0, 0.1)
        self.assertEqual(smoke_screen.mean_luminance(parsed(gray(80)), coarse), 80.0)


class AnalyzeViewfinderTest(unittest.TestCase):
    def test_changing_live_picture_passes_and_reports_numbers(self) -> None:
        outcome = smoke_screen.analyze_viewfinder(parsed(noisy(1)), parsed(noisy(2)), THRESHOLDS)
        self.assertEqual(outcome.problems, [])
        self.assertIn("яркость", outcome.notes[0])
        self.assertIn(f"{WIDTH}x{HEIGHT}", outcome.notes[0])

    def test_black_screen_is_a_problem_on_each_shot(self) -> None:
        outcome = smoke_screen.analyze_viewfinder(parsed(black), parsed(black), THRESHOLDS)
        black_problems = [text for text in outcome.problems if "чёрный" in text]
        self.assertEqual(len(black_problems), 2, outcome.problems)
        self.assertIn("viewfinder_a", black_problems[0])
        self.assertIn("viewfinder_b", black_problems[1])

    def test_only_the_second_shot_black(self) -> None:
        outcome = smoke_screen.analyze_viewfinder(parsed(noisy(1)), parsed(black), THRESHOLDS)
        self.assertTrue(any("viewfinder_b" in text and "чёрный" in text for text in outcome.problems), outcome.problems)

    def test_frozen_picture_is_a_problem_even_though_it_is_bright(self) -> None:
        outcome = smoke_screen.analyze_viewfinder(parsed(noisy(5)), parsed(noisy(5)), THRESHOLDS)
        self.assertEqual(len(outcome.problems), 1, outcome.problems)
        self.assertIn("не меняется", outcome.problems[0])

    def test_black_band_with_bright_overlay_is_still_black(self) -> None:
        overlay = lambda _x, y: (255, 255, 255) if y < 25 else (0, 0, 0)  # noqa: E731
        outcome = smoke_screen.analyze_viewfinder(parsed(overlay), parsed(overlay), THRESHOLDS)
        self.assertTrue(any("чёрный" in text for text in outcome.problems), outcome.problems)


class ThresholdsFileTest(unittest.TestCase):
    def test_shipped_file_loads_with_low_but_nonzero_thresholds(self) -> None:
        thresholds = smoke_screen.load_thresholds()
        self.assertGreater(thresholds.min_mean_luminance, 0)
        self.assertGreater(thresholds.min_mean_abs_difference, 0)
        self.assertLess(thresholds.min_mean_luminance, 10, "кадр линейный и тёмный: порог низкий")
        self.assertEqual((thresholds.top_fraction, thresholds.bottom_fraction), (0.30, 0.90))

    def test_missing_key_names_it(self) -> None:
        with self.assertRaisesRegex(smoke_screen.ScreenError, "min_mean_abs_difference"):
            smoke_screen.parse_thresholds(
                "[band]\ntop_fraction=0.3\nbottom_fraction=0.9\nsample_step_pixels=4\n[thresholds]\nmin_mean_luminance=2\n"
            )

    def test_inverted_band_is_rejected(self) -> None:
        with self.assertRaisesRegex(smoke_screen.ScreenError, "полоса анализа"):
            smoke_screen.parse_thresholds(
                "[band]\ntop_fraction=0.9\nbottom_fraction=0.3\nsample_step_pixels=4\n"
                "[thresholds]\nmin_mean_luminance=2\nmin_mean_abs_difference=0.1\n"
            )

    def test_not_toml(self) -> None:
        with self.assertRaisesRegex(smoke_screen.ScreenError, "не TOML"):
            smoke_screen.parse_thresholds("это = = не toml")


class PngTest(unittest.TestCase):
    def test_png_round_trips_the_pixels(self) -> None:
        shot = parsed(noisy(4), width=8, height=6)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "shot.png"
            smoke_screen.write_png(path, shot)
            data = path.read_bytes()
        self.assertTrue(data.startswith(b"\x89PNG\r\n\x1a\n"))
        position, idat, header = 8, b"", None
        while position < len(data):
            (length,) = struct.unpack(">I", data[position : position + 4])
            kind = data[position + 4 : position + 8]
            payload = data[position + 8 : position + 8 + length]
            (crc,) = struct.unpack(">I", data[position + 8 + length : position + 12 + length])
            self.assertEqual(crc, zlib.crc32(kind + payload), f"контрольная сумма куска {kind!r}")
            if kind == b"IHDR":
                header = struct.unpack(">IIBBBBB", payload)
            elif kind == b"IDAT":
                idat += payload
            position += 12 + length
        self.assertEqual(header, (8, 6, 8, 6, 0, 0, 0))
        rows = zlib.decompress(idat)
        row_bytes = 8 * 4
        restored = b"".join(rows[1 + y * (row_bytes + 1) : (y + 1) * (row_bytes + 1)] for y in range(6))
        self.assertEqual(restored, shot.rgba)


class ScreenPostCheckTest(unittest.TestCase):
    def write_pair(self, directory: Path, first, second) -> None:
        for label, pixel_at in zip(smoke_screen.VIEWFINDER_LABELS, (first, second), strict=True):
            smoke_screen.capture_path(directory, label).write_bytes(screencap_bytes(pixel_at, header_with_colorspace=True))

    def test_live_pair_passes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            self.write_pair(Path(directory), noisy(1), noisy(2))
            outcome = smoke_screen.screen_post_check(Path(directory), THRESHOLDS)
        self.assertEqual(outcome.problems, [])

    def test_black_pair_fails(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            self.write_pair(Path(directory), black, black)
            outcome = smoke_screen.screen_post_check(Path(directory), THRESHOLDS)
        self.assertTrue(any("чёрный" in text for text in outcome.problems), outcome.problems)

    def test_missing_shot_is_a_problem_naming_the_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            outcome = smoke_screen.screen_post_check(Path(directory), THRESHOLDS)
        self.assertEqual(len(outcome.problems), 1)
        self.assertIn("viewfinder_a.screencap", outcome.problems[0])

    def test_corrupt_shot_is_reported_not_raised(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            self.write_pair(Path(directory), noisy(1), noisy(2))
            smoke_screen.capture_path(Path(directory), "viewfinder_b").write_bytes(b"\x01\x02\x03")
            outcome = smoke_screen.screen_post_check(Path(directory), THRESHOLDS)
        self.assertTrue(any("анализ скриншотов не удался" in text for text in outcome.problems), outcome.problems)


if __name__ == "__main__":
    unittest.main()
