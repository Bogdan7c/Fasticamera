"""Проверка нейтральной карты по скриншотам ``viewfinder`` (по флагу, выключена по умолчанию).

Владелец наводит телефон на серую карту так, чтобы она закрывала центр кадра, и запускает
``device-smoke.sh --neutral-card viewfinder``. Центр серой карты после нашего ISP (баланс белого,
цвет, тон) обязан выйти серым: каналы R, G, B центральной области близки друг к другу. Мера —
относительная цветность ``(max(R,G,B) - min(R,G,B)) / среднее(R,G,B)`` по средним каналам области:
0 — идеально серый, 0,1 — заметный оттенок. Слишком тёмная область (карта не освещена, экран чёрный)
— отдельная проблема: цветность там не мерится.

Уровень. Карта на весь кадр после нашей AE выходит на средний серый тона: ``display_middle_gray`` в
линейном свете экрана (isp-model, tone.rs). Центр кадра линеаризуется обратной передаточной функцией
sRGB / Display P3 (она общая), яркость Y берётся с коэффициентами Display P3, а ошибка —
``log2(Y / display_middle_gray)`` ступеней (EV). Допущение: карта нейтральная (R≈G≈B), поэтому тег
цветового пространства скриншота на уровень не влияет.

Пороги и размер области — секция ``[neutral_card]`` в ``smoke_screen.toml`` (менять там, не в коде).
"""

from __future__ import annotations

import math
import tomllib
from dataclasses import dataclass
from pathlib import Path

import smoke_screen


@dataclass(frozen=True)
class NeutralCardThresholds:
    """Пороги проверки (см. ``smoke_screen.toml``, секция ``[neutral_card]``)."""

    region_fraction: float
    max_relative_chroma: float
    min_mean_level: float
    display_middle_gray: float
    max_middle_gray_error_ev: float
    p3_luminance_coefficients: tuple[float, float, float]

    def __post_init__(self) -> None:
        if not 0.0 < self.region_fraction <= 1.0:
            raise smoke_screen.ScreenError(f"region_fraction должна лежать в (0, 1], а не {self.region_fraction}")
        if self.max_relative_chroma <= 0 or self.min_mean_level < 0:
            raise smoke_screen.ScreenError("пороги нейтральной карты должны быть положительными")
        if not 0.0 < self.display_middle_gray < 1.0:
            raise smoke_screen.ScreenError(f"display_middle_gray должен лежать в (0, 1), а не {self.display_middle_gray}")
        if self.max_middle_gray_error_ev <= 0:
            raise smoke_screen.ScreenError("max_middle_gray_error_ev должен быть положительным")
        if len(self.p3_luminance_coefficients) != 3 or any(c <= 0 for c in self.p3_luminance_coefficients):
            raise smoke_screen.ScreenError("p3_luminance_coefficients: ждём три положительных числа")


def parse_thresholds(text: str) -> NeutralCardThresholds:
    """Пороги из текста TOML; нет секции или ключа — ``ScreenError`` с именем."""
    try:
        section = tomllib.loads(text)["neutral_card"]
        return NeutralCardThresholds(
            region_fraction=float(section["region_fraction"]),
            max_relative_chroma=float(section["max_relative_chroma"]),
            min_mean_level=float(section["min_mean_level"]),
            display_middle_gray=float(section["display_middle_gray"]),
            max_middle_gray_error_ev=float(section["max_middle_gray_error_ev"]),
            p3_luminance_coefficients=tuple(float(c) for c in section["p3_luminance_coefficients"]),  # type: ignore[arg-type]
        )
    except tomllib.TOMLDecodeError as error:
        raise smoke_screen.ScreenError(f"smoke_screen.toml: не TOML ({error})") from error
    except KeyError as error:
        raise smoke_screen.ScreenError(f"smoke_screen.toml: нет ключа {error} в [neutral_card]") from error


def load_thresholds(path: Path = smoke_screen.THRESHOLDS_FILE) -> NeutralCardThresholds:
    return parse_thresholds(path.read_text(encoding="utf-8"))


def center_mean_rgb(screenshot: smoke_screen.Screenshot, region_fraction: float) -> tuple[float, float, float]:
    """Средние R, G, B квадратной области в центре экрана (сторона — доля меньшей стороны экрана)."""
    side = max(1, int(min(screenshot.width, screenshot.height) * region_fraction))
    left = (screenshot.width - side) // 2
    top = (screenshot.height - side) // 2
    totals = [0, 0, 0]
    row_bytes = screenshot.width * 4
    for row in range(top, top + side):
        base = row * row_bytes + left * 4
        for column in range(side):
            offset = base + column * 4
            for channel in range(3):
                totals[channel] += screenshot.rgba[offset + channel]
    count = side * side
    return (totals[0] / count, totals[1] / count, totals[2] / count)


def relative_chroma(rgb: tuple[float, float, float]) -> float:
    """``(max - min) / среднее`` каналов; среднее 0 — бесконечность (не серый, а чёрный)."""
    mean = sum(rgb) / 3
    if mean <= 0:
        return float("inf")
    return (max(rgb) - min(rgb)) / mean


def srgb_encoded_to_linear(encoded: float) -> float:
    """Обратная передаточная функция sRGB / Display P3: закодированное 0..1 → линейный свет."""
    if encoded <= 0.04045:
        return encoded / 12.92
    return ((encoded + 0.055) / 1.055) ** 2.4


def srgb_linear_to_encoded(linear: float) -> float:
    """Передаточная функция sRGB / Display P3: линейный свет → закодированное 0..1."""
    if linear <= 0.0031308:
        return 12.92 * linear
    return 1.055 * linear ** (1 / 2.4) - 0.055


def display_luminance(rgb: tuple[float, float, float], coefficients: tuple[float, float, float]) -> float:
    """Яркость Y в линейном свете экрана по средним каналам 0..255 (линеаризация, затем взвешенная сумма)."""
    return sum(weight * srgb_encoded_to_linear(channel / 255) for weight, channel in zip(coefficients, rgb))


def middle_gray_error_ev(luminance: float, target: float) -> float:
    """Ошибка уровня в ступенях: ``log2(Y / target)``; ноль света — минус бесконечность."""
    if luminance <= 0:
        return float("-inf")
    return math.log2(luminance / target)


def analyze_neutral_card(
    screenshot: smoke_screen.Screenshot, thresholds: NeutralCardThresholds, label: str
) -> smoke_screen.ScreenAnalysis:
    """Один снимок → нарушения и пояснение с числами."""
    rgb = center_mean_rgb(screenshot, thresholds.region_fraction)
    mean = sum(rgb) / 3
    chroma = relative_chroma(rgb)
    note = f"нейтральная карта ({label}): центр R={rgb[0]:.1f} G={rgb[1]:.1f} B={rgb[2]:.1f}, цветность {chroma:.3f}"
    luminance = display_luminance(rgb, thresholds.p3_luminance_coefficients)
    error_ev = middle_gray_error_ev(luminance, thresholds.display_middle_gray)
    expected_encoded = srgb_linear_to_encoded(thresholds.display_middle_gray) * 255
    note += (
        f"; уровень: Y={luminance:.4f} (цель {thresholds.display_middle_gray}, ждём ~{expected_encoded:.1f}/255), "
        f"ошибка {error_ev:+.2f} EV (допуск ±{thresholds.max_middle_gray_error_ev})"
    )
    problems: list[str] = []
    if mean < thresholds.min_mean_level:
        problems.append(f"центр слишком тёмный для проверки карты ({label}): уровень {mean:.1f} < {thresholds.min_mean_level}")
    elif chroma > thresholds.max_relative_chroma:
        problems.append(f"карта не серая ({label}): цветность {chroma:.3f} > {thresholds.max_relative_chroma}")
    # Уровень судим, только когда кадр не чёрный (тёмный центр уже назван проблемой выше).
    if mean >= thresholds.min_mean_level and abs(error_ev) > thresholds.max_middle_gray_error_ev:
        direction = "светлее" if error_ev > 0 else "темнее"
        problems.append(
            f"уровень карты не на среднем сером ({label}): {error_ev:+.2f} EV ({direction} цели {thresholds.display_middle_gray}), "
            f"допуск ±{thresholds.max_middle_gray_error_ev} EV"
        )
    return smoke_screen.ScreenAnalysis(problems, [note])


def neutral_card_post_check(
    artifacts_dir: Path, thresholds: NeutralCardThresholds | None = None
) -> smoke_screen.ScreenAnalysis:
    """Оба снимка ``viewfinder_live`` из каталога артефактов → вердикт по нейтральной карте."""
    try:
        limits = thresholds or load_thresholds()
        problems: list[str] = []
        notes: list[str] = []
        for label in smoke_screen.VIEWFINDER_LABELS:
            path = smoke_screen.capture_path(artifacts_dir, label)
            if not path.is_file():
                return smoke_screen.ScreenAnalysis([f"нет скриншота {path.name}: проверять карту нечем"], [])
            analysis = analyze_neutral_card(smoke_screen.parse_screencap(path.read_bytes()), limits, label)
            problems.extend(analysis.problems)
            notes.extend(analysis.notes)
        return smoke_screen.ScreenAnalysis(problems, notes)
    except (smoke_screen.ScreenError, OSError) as error:
        return smoke_screen.ScreenAnalysis([f"проверка нейтральной карты не удалась: {error}"], [])
