"""Тесты вспышки (14d) в перепроверке сводки ``capture``: шаг ``single_flash_on`` обязан дать кадр со вспышкой и итог
камеры, шаг ``single_flash_auto`` наблюдательный, остальные шаги идут без вспышки. Помощники сводки — из
``test_smoke_capture`` (отдельный файл — ради предела размера модуля)."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_checks_capture  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import smoke_wait  # noqa: E402
from test_smoke_capture import capture_report, compact_shot, flash_auto_shot, flash_on_shot, passed, verdict_of  # noqa: E402


class FlashSummaryChecksTest(unittest.TestCase):
    def test_flash_on_must_produce_a_flash_frame_with_a_camera_report(self) -> None:
        def with_flash_on(**overrides: object) -> dict:
            report = capture_report()
            report["shots"][2] = flash_on_shot(3, **overrides)
            return report

        cases = {
            "wrong_choice": (dict(choice="exact"), "взят кадр 'exact', ожидался кадр со вспышкой"),
            "no_camera_report": (dict(flash_cam=None), "нет итога камеры по вспышке"),
            "wrong_setting": (dict(flash="auto"), "экран просил вспышку 'auto', ожидался 'on'"),
        }
        for name, (overrides, needle) in cases.items():
            with self.subTest(name):
                problems = smoke_checks_capture.check_capture(passed(with_flash_on(**overrides)))
                self.assertTrue(any(needle in problem for problem in problems), problems)
        self.assertEqual(smoke_checks_capture.check_capture(passed(with_flash_on())), [])

    def test_flash_frames_where_the_flash_is_off_are_a_problem_and_a_missing_flash_step_too(self) -> None:
        report = capture_report()
        report["shots"][0] = compact_shot(1, "single_heif", choice="flash_frame")
        problems = smoke_checks_capture.check_capture(passed(report))
        self.assertTrue(any("вспышка выключена, а взят кадр со вспышкой" in problem for problem in problems), problems)
        burst_on = capture_report()
        burst_on["shots"][6] = compact_shot(7, "burst", flash="on")
        self.assertTrue(any("вспышка 'on', ожидалась 'off'" in problem for problem in smoke_checks_capture.check_capture(passed(burst_on))))
        missing = capture_report()
        del missing["shots"][2]
        self.assertTrue(any("шаг single_flash_on: снимков 0" in problem for problem in smoke_checks_capture.check_capture(passed(missing))))

    def test_auto_flash_is_observed_whichever_way_it_went_and_the_notes_say_which(self) -> None:
        for fired, word in ((False, "вспышка сработала: нет"), (True, "вспышка сработала: да")):
            with self.subTest(fired=fired):
                report = capture_report()
                report["shots"][3] = flash_auto_shot(4, fired=fired)
                verdict = verdict_of(report)
                self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)
                joined = " ".join(verdict.notes)
                self.assertIn(f"NOTE вспышка auto (шаг single_flash_auto): кадр {'flash_frame' if fired else 'exact'}, {word}", joined)
        notes = " ".join(verdict_of(capture_report()).notes)
        self.assertIn("выдержка 12.5 мс, ISO 400, доля света вспышки 0.62", notes)

    def test_omitted_defaults_read_as_heif_flash_off_and_exact_choice(self) -> None:
        # Строка результата ≤ 4000 байт: умолчания (fmt heif, flash off, choice exact) в сводке не пишутся.
        report = capture_report()
        for shot in report["shots"]:
            for key in ("fmt", "flash", "choice"):
                if shot.get(key) in ("heif", "off", "exact"):
                    del shot[key]
        self.assertEqual(smoke_checks_capture.check_capture(passed(report)), [])


if __name__ == "__main__":
    unittest.main()
