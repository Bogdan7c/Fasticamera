"""Метрики коррекции: leave-one-out, отложенный прогон, кросс-проверка на DNG другого стенда.

Часть пакета `shading_calibration` (см. `pc/shading_calibrate.py`).
"""

from __future__ import annotations

from pathlib import Path

import numpy as np

from .core import (
    BLOCK,
    CHANNEL_B,
    CHANNEL_G_EVEN,
    CHANNEL_G_ODD,
    CHANNEL_R,
    CORNER_INSET_BLOCKS,
    CalibrationError,
    GainTable,
    GridSpec,
    Placement,
    RADIAL_BINS,
    SensorGeometry,
)
from .loading import (
    center_window,
)
from .fitting import (
    calibrate,
)


# ---------------------------------------------------------------------------------------------
# Метрики коррекции
# ---------------------------------------------------------------------------------------------

def corrected_green(planes: np.ndarray, table: GainTable) -> np.ndarray:
    """Среднее двух зелёных плоскостей после коррекции таблицей (каждая своим gain)."""
    corrected = planes * table.sample_block_centers()
    return (corrected[CHANNEL_G_EVEN] + corrected[CHANNEL_G_ODD]) / 2


def corner_ratios(plane: np.ndarray) -> np.ndarray:
    """Углы TL, TR, BL, BR (блок с отступом) относительно центра плоскости."""
    k = CORNER_INSET_BLOCKS
    centre = center_window(plane).mean()
    return np.array([plane[k, k], plane[k, -k - 1], plane[-k - 1, k], plane[-k - 1, -k - 1]]) / centre


def radial_profile(plane: np.ndarray) -> list[float]:
    """Среднее по 10 радиальным кольцам (r=0 центр, r=1 угол) относительно центра."""
    blocks_y, blocks_x = plane.shape
    y, x = np.mgrid[0:blocks_y, 0:blocks_x]
    radius = np.hypot((y + 0.5 - blocks_y / 2) / (blocks_y / 2), (x + 0.5 - blocks_x / 2) / (blocks_x / 2)) / np.sqrt(2)
    centre = center_window(plane).mean()
    edges = np.linspace(0.0, 1.0, RADIAL_BINS + 1)
    return [float(plane[(radius >= lo) & (radius < hi)].mean() / centre) for lo, hi in zip(edges[:-1], edges[1:])]


def uniformity_report(green: np.ndarray) -> dict:
    """Углы, средняя ошибка в EV и макс. отклонение от центра для скорректированного зелёного."""
    corners = corner_ratios(green)
    return {
        "corners_tl_tr_bl_br": [round(float(c), 4) for c in corners],
        "mean_ev": round(float(np.log2(corners.mean())), 4),
        "max_abs_deviation": round(float(np.abs(green / center_window(green).mean() - 1).max()), 4),
    }


def leave_one_out(
    placements: list[Placement], geometry: SensorGeometry, grid: GridSpec, degree: int
) -> list[dict]:
    """Для каждой укладки: подгонка по остальным, проверка на отложенной. Нужно >= 2 укладок."""
    if len(placements) < 2:
        raise CalibrationError("для leave-one-out нужно минимум 2 укладки")
    reports = []
    for held_index, held in enumerate(placements):
        train = [p for i, p in enumerate(placements) if i != held_index]
        table = calibrate(train, geometry, grid, degree)
        report = uniformity_report(corrected_green(held.planes, table))
        reports.append({"held_out": held.label, **report})
    return reports


def validate_on_placement(placement: Placement, table: GainTable) -> dict:
    """Проверка готовой таблицы на укладке, не участвовавшей в подгонке."""
    return {"run": placement.label, **uniformity_report(corrected_green(placement.planes, table))}


# ---------------------------------------------------------------------------------------------
# Кросс-проверка на DNG приложения (другой стенд, 4 листа бумаги на мониторе)
# ---------------------------------------------------------------------------------------------

def dng_to_placement(dng_path: Path, geometry: SensorGeometry) -> Placement:
    """DNG приложения → плоскости каналов по блокам 32x32 px (16 отсчётов плоскости), нормированные на центр."""
    import rawpy  # локальный импорт: rawpy нужен только для кросс-проверки

    with rawpy.imread(str(dng_path)) as dng:
        raw = dng.raw_image_visible.astype(np.float64)
        pattern, color_desc = dng.raw_pattern, dng.color_desc.decode()
        blacks, white = dng.black_level_per_channel, float(dng.white_level)
    if raw.shape != (geometry.height, geometry.width):
        raise CalibrationError(f"{dng_path}: DNG {raw.shape[::-1]} не совпадает с активным массивом {geometry.width}x{geometry.height}")
    side = BLOCK // 2  # отсчётов плоскости на сторону блока
    planes = np.empty((4, geometry.blocks_y, geometry.blocks_x))
    for row in range(2):
        for col in range(2):
            color_index = int(pattern[row, col])
            color = color_desc[color_index]
            channel = {"R": CHANNEL_R, "B": CHANNEL_B}.get(color, CHANNEL_G_EVEN if row == 0 else CHANNEL_G_ODD)
            plane = (raw[row::2, col::2] - blacks[color_index]) / (white - blacks[color_index])
            blocks = plane[: geometry.blocks_y * side, : geometry.blocks_x * side]
            planes[channel] = blocks.reshape(geometry.blocks_y, side, geometry.blocks_x, side).mean(axis=(1, 3))
    planes /= center_window(planes).mean(axis=(1, 2)).reshape(4, 1, 1)
    return Placement(label=dng_path.name, planes=planes)


def cross_bench_report(placement: Placement, table: GainTable) -> dict:
    """Углы и радиальный профиль скорректированного зелёного на DNG."""
    green = corrected_green(placement.planes, table)
    return {
        "source": placement.label,
        **uniformity_report(green),
        "radial_profile_10_bins": [round(v, 4) for v in radial_profile(green)],
    }
