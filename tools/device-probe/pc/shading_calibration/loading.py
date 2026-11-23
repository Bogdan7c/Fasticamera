"""Загрузка прогонов зонда и приведение блоков к линейным нормированным плоскостям каналов.

Часть пакета `shading_calibration` (см. `pc/shading_calibrate.py`).
"""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np

from .core import (
    BLOCK,
    CALIBRATION_PHASE,
    CENTER_HALF_WINDOW,
    CalibrationError,
    Placement,
    RawRun,
    channels_of_positions,
)


# ---------------------------------------------------------------------------------------------
# Загрузка прогонов
# ---------------------------------------------------------------------------------------------

def _parse_active_size(active_array: str) -> tuple[int, int]:
    """'0 0 4096 3072' (left top right bottom) → (width, height)."""
    try:
        left, top, right, bottom = (int(part) for part in active_array.split())
    except ValueError as error:
        raise CalibrationError(f"не разобрать active_array {active_array!r}") from error
    return right - left, bottom - top


def _mean_dynamic_black(phase: dict, source: Path) -> np.ndarray:
    """Среднее по кадрам фазы динамического чёрного по 4 позициям буфера (в DN)."""
    values = [frame["dynamic_black_level"] for frame in phase["frames"] if frame.get("dynamic_black_level")]
    if not values:
        # Без динамического чёрного калибровка тихо уехала бы на статический — не прощаем.
        raise CalibrationError(f"{source}: в кадрах фазы нет dynamic_black_level")
    return np.mean(np.asarray(values, dtype=np.float64), axis=0)


def _run_label(json_path: Path) -> str:
    """`led-a/062913-db3ea5` для .../led-a/captures/062913-db3ea5/main43.json, иначе имя каталога прогона."""
    run_dir = json_path.parent
    if run_dir.parent.name == "captures":
        return f"{run_dir.parent.parent.name}/{run_dir.name}"
    return run_dir.name


def load_raw_run(json_path: Path, phase_name: str = CALIBRATION_PHASE) -> RawRun:
    """Читает `<mode>.json` и `<mode>_<phase>.blocks.f32` прогона."""
    report = json.loads(json_path.read_text(encoding="utf-8"))
    phase = next((p for p in report["phases"] if p["phase"] == phase_name), None)
    if phase is None:
        raise CalibrationError(f"{json_path}: нет фазы {phase_name!r}")
    characteristics = report["characteristics"]
    width, height = _parse_active_size(characteristics["active_array"])
    buffer_width = report["buffer_geometry"]["width"]
    # Гипотеза A (проверена в 14f): валидная область буфера == весь активный массив. Иначе координаты сетки неверны.
    if (buffer_width, phase["valid_rows"]) != (width, height):
        raise CalibrationError(
            f"{json_path}: буфер {buffer_width}x{phase['valid_rows']} не совпадает с активным массивом {width}x{height}"
        )
    grid = phase["grid"]
    if grid["block_size"] != BLOCK:
        raise CalibrationError(f"{json_path}: блок {grid['block_size']} px, ожидался {BLOCK}")
    blocks_path = json_path.parent / f"{report['mode']}_{phase_name}.blocks.f32"
    expected = (4, grid["blocks_y"], grid["blocks_x"])
    block_means = np.fromfile(blocks_path, dtype="<f4").astype(np.float64)
    if block_means.size != int(np.prod(expected)):
        raise CalibrationError(f"{blocks_path}: {block_means.size} значений, ожидалось {int(np.prod(expected))}")
    return RawRun(
        label=_run_label(json_path),
        run_id=json_path.parent.name,
        mode=report["mode"],
        cfa=characteristics["cfa"],
        width=width,
        height=height,
        white_level=float(characteristics["white_level"]),
        dynamic_black=_mean_dynamic_black(phase, json_path),
        block_means=block_means.reshape(expected),
    )


def linearize_by_position(run: RawRun) -> np.ndarray:
    """(DN - динамический чёрный) / (белый - чёрный), по позициям буфера: (4, by, bx)."""
    black = run.dynamic_black.reshape(4, 1, 1)
    return (run.block_means - black) / (run.white_level - black)


def center_window(plane: np.ndarray) -> np.ndarray:
    """Окно 4x4 блока в центре плоскости (view)."""
    blocks_y, blocks_x = plane.shape[-2:]
    return plane[..., blocks_y // 2 - CENTER_HALF_WINDOW: blocks_y // 2 + CENTER_HALF_WINDOW,
                 blocks_x // 2 - CENTER_HALF_WINDOW: blocks_x // 2 + CENTER_HALF_WINDOW]


def to_placement(run: RawRun, position_channels: tuple[int, ...] | None = None) -> Placement:
    """Раскладывает позиции буфера по каналам Camera2 и нормирует каждый канал на свой центр.

    `position_channels` переопределяет сопоставление позиция→канал только для самотеста (заведомо неверный CFA).
    """
    if position_channels is None:
        position_channels = channels_of_positions(run.cfa)
    linear = linearize_by_position(run)
    planes = np.empty_like(linear)
    for position, channel in enumerate(position_channels):
        planes[channel] = linear[position]
    centers = center_window(planes).mean(axis=(1, 2)).reshape(4, 1, 1)
    if np.any(centers <= 0):
        raise CalibrationError(f"{run.label}: центр плоскости <= 0 после вычитания чёрного (прогон пуст или пересвечен)")
    return Placement(label=run.label, planes=planes / centers)
