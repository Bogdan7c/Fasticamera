"""Проверка нейтральной карты по скриншотам ``viewfinder`` (по флагу, выключена по умолчанию).

Владелец наводит телефон на серую карту так, чтобы она закрывала центр кадра, и запускает
``device-smoke.sh --neutral-card viewfinder``. Центр серой карты после нашего ISP (баланс белого,
цвет, тон) обязан выйти серым: каналы R, G, B центральной области близки друг к другу. Мера —
относительная цветность ``(max(R,G,B) - min(R,G,B)) / среднее(R,G,B)`` по средним каналам области:
0 — идеально серый, 0,1 — заметный оттенок. Слишком тёмная область (карта не освещена, экран чёрный)
— отдельная проблема: цветность там не мерится.

Пороги и размер области — секция ``[neutral_card]`` в ``smoke_screen.toml`` (менять там, не в коде).
"""

from __future__ import annotations

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

    def __post_init__(self) -> None:
        if not 0.0 < self.region_fraction <= 1.0:
            raise smoke_screen.ScreenError(f"region_fraction должна лежать в (0, 1], а не {self.region_fraction}")
        if self.max_relative_chroma <= 0 or self.min_mean_level < 0:
            raise smoke_screen.ScreenError("пороги нейтральной карты должны быть положительными")


def parse_thresholds(text: str) -> NeutralCardThresholds:
    """Пороги из текста TOML; нет секции или ключа — ``ScreenError`` с именем."""
    try:
        section = tomllib.loads(text)["neutral_card"]
        return NeutralCardThresholds(
            region_fraction=float(section["region_fraction"]),
            max_relative_chroma=float(section["max_relative_chroma"]),
            min_mean_level=float(section["min_mean_level"]),
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


def analyze_neutral_card(
    screenshot: smoke_screen.Screenshot, thresholds: NeutralCardThresholds, label: str
) -> smoke_screen.ScreenAnalysis:
    """Один снимок → нарушения и пояснение с числами."""
    rgb = center_mean_rgb(screenshot, thresholds.region_fraction)
    mean = sum(rgb) / 3
    chroma = relative_chroma(rgb)
    note = f"нейтральная карта ({label}): центр R={rgb[0]:.1f} G={rgb[1]:.1f} B={rgb[2]:.1f}, цветность {chroma:.3f}"
    problems: list[str] = []
    if mean < thresholds.min_mean_level:
        problems.append(f"центр слишком тёмный для проверки карты ({label}): уровень {mean:.1f} < {thresholds.min_mean_level}")
    elif chroma > thresholds.max_relative_chroma:
        problems.append(f"карта не серая ({label}): цветность {chroma:.3f} > {thresholds.max_relative_chroma}")
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
