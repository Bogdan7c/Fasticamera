"""Самотест на синтетическом поле с известной виньеткой.

Часть пакета `shading_calibration` (см. `pc/shading_calibrate.py`).
"""

from __future__ import annotations

import json
from pathlib import Path
import tempfile

import numpy as np

from .core import (
    BLOCK,
    CALIBRATION_PHASE,
    CHANNEL_B,
    CHANNEL_G_EVEN,
    CHANNEL_G_ODD,
    CHANNEL_R,
    DEFAULT_DEGREE,
    DEFAULT_GRID_COLUMNS,
    DEFAULT_GRID_ROWS,
    GAIN_DECIMALS,
    GainTable,
    GridSpec,
    MIN_GAIN_UPPER_BOUND,
    SELFTEST_GAIN_TOLERANCE,
    SensorGeometry,
    block_centers,
    channels_of_positions,
    grid_pixel_positions,
    grid_sample_positions,
    sample_positions,
)
from .loading import (
    load_raw_run,
    to_placement,
)
from .fitting import (
    calibrate,
    fit_model,
)
from .output import (
    table_points,
)


# ---------------------------------------------------------------------------------------------
# Самотест на синтетическом поле с известной виньеткой
# ---------------------------------------------------------------------------------------------

# Синтетический кадр: достаточно мал для скорости и достаточно крупен для степени 16.
SYNTH_WIDTH, SYNTH_HEIGHT = 1024, 768
SYNTH_WHITE, SYNTH_BLACK_PATTERN = 1023.0, [64, 64, 64, 64]
# Динамический чёрный по позициям — разный, чтобы вычитание по позициям было важно.
SYNTH_DYNAMIC_BLACK = [64.0, 64.5, 63.5, 64.25]
SYNTH_CFA = 3  # BGGR
# Шум среднего блока на укладку. Самотест проверяет АЛГОРИТМ (каналы, конвенцию сетки, нормировку), а не шумоустойчивость:
# полином степени 16 усиливает шум у самого края кадра примерно в 40 раз (шум 0.05 % даёт ~2 % ошибки gain в крайних узлах).
# Реальную устойчивость края показывают leave-one-out и отложенный прогон в отчёте калибровки, а не этот допуск.
SYNTH_NOISE_SIGMA = 0.0001
SYNTH_TILT_SIGMA = 0.005      # 0.5 % случайный наклон подсветки на укладку (имитация неровного стенда)
SYNTH_PLACEMENTS = 4
SYNTH_SEED = 14
# Коэффициенты виньетки по каналам: (u², v², u·v, u, u³). Разные у каналов и асимметричные по углам.
SYNTH_VIGNETTE = {
    CHANNEL_R: (0.20, 0.17, -0.04, 0.04, 0.02),
    CHANNEL_G_EVEN: (0.26, 0.22, 0.05, -0.03, 0.0),
    CHANNEL_G_ODD: (0.19, 0.28, -0.05, 0.05, -0.03),  # заметно отличается от G_even: подмена каналов будет видна
    CHANNEL_B: (0.30, 0.24, 0.06, -0.04, 0.03),
}


def synthetic_vignette(channel: int, x: np.ndarray, y: np.ndarray) -> np.ndarray:
    """Известная виньетка канала в непрерывных координатах кадра (1 в центре)."""
    cu2, cv2, cuv, cu, cu3 = SYNTH_VIGNETTE[channel]
    u, v = x / SYNTH_WIDTH * 2 - 1, y / SYNTH_HEIGHT * 2 - 1
    return 1 - cu2 * u**2 - cv2 * v**2 + cuv * u * v + cu * u + cu3 * u**3


def true_gains(geometry: SensorGeometry, grid: GridSpec) -> np.ndarray:
    """Истинные gain в узлах (rows, columns, 4) по ТОЙ ЖЕ формуле: максимум по центрам блоков и узлам / значение."""
    dense_x, dense_y = sample_positions(geometry, grid, grid_pixel_positions)
    node_x, node_y = grid_sample_positions(geometry, grid, grid_pixel_positions)
    gains = np.empty((grid.rows, grid.columns, 4))
    for channel in range(4):
        peak = synthetic_vignette(channel, dense_x, dense_y).max()
        gains[:, :, channel] = (peak / synthetic_vignette(channel, node_x, node_y)).reshape(grid.rows, grid.columns)
    return gains


def write_synthetic_run(run_dir: Path, rng: np.random.Generator, geometry: SensorGeometry) -> Path:
    """Пишет синтетический прогон в формате зонда (json + blocks.f32) с известной виньеткой, шумом и наклоном света."""
    positions = channels_of_positions(SYNTH_CFA)
    level = rng.uniform(0.3, 0.5)  # произвольная яркость стенда
    tilt = rng.normal(0.0, SYNTH_TILT_SIGMA, size=2)
    x, y = np.meshgrid(block_centers(geometry.blocks_x), block_centers(geometry.blocks_y))
    light = 1 + tilt[0] * (x / SYNTH_WIDTH * 2 - 1) + tilt[1] * (y / SYNTH_HEIGHT * 2 - 1)
    means = np.empty((4, geometry.blocks_y, geometry.blocks_x), dtype="<f4")
    for position, channel in enumerate(positions):
        signal = level * light * synthetic_vignette(channel, x, y) * (1 + rng.normal(0, SYNTH_NOISE_SIGMA, x.shape))
        black = SYNTH_DYNAMIC_BLACK[position]
        means[position] = black + signal * (SYNTH_WHITE - black)
    run_dir.mkdir(parents=True)
    means.tofile(run_dir / "synth_base.blocks.f32")
    report = {
        "mode": "synth",
        "characteristics": {"active_array": f"0 0 {geometry.width} {geometry.height}", "cfa": SYNTH_CFA,
                            "white_level": int(SYNTH_WHITE), "black_level_pattern": SYNTH_BLACK_PATTERN},
        "buffer_geometry": {"width": geometry.width, "height": geometry.height},
        "phases": [{"phase": CALIBRATION_PHASE, "valid_rows": geometry.height,
                    "grid": {"blocks_x": geometry.blocks_x, "blocks_y": geometry.blocks_y, "block_size": BLOCK},
                    "frames": [{"dynamic_black_level": SYNTH_DYNAMIC_BLACK}] * 2}],
    }
    (run_dir / "synth.json").write_text(json.dumps(report), encoding="utf-8")
    return run_dir / "synth.json"


def max_relative_gain_error(table: GainTable, truth: np.ndarray) -> float:
    """Максимальная относительная ошибка gain по всем узлам и каналам."""
    return float(np.abs(table.gains / truth - 1).max())


def run_selftest() -> int:
    """Самотест: верная конфигурация восстанавливает gain, заведомо неверные — проваливаются."""
    geometry = SensorGeometry(SYNTH_WIDTH, SYNTH_HEIGHT, SYNTH_WIDTH // BLOCK, SYNTH_HEIGHT // BLOCK)
    grid = GridSpec(DEFAULT_GRID_COLUMNS, DEFAULT_GRID_ROWS)
    truth = true_gains(geometry, grid)
    rng = np.random.default_rng(SYNTH_SEED)
    failures: list[str] = []

    def check(condition: bool, message: str) -> None:
        print(("ok   " if condition else "FAIL ") + message)
        if not condition:
            failures.append(message)

    with tempfile.TemporaryDirectory(prefix="shading-selftest-") as tmp:
        paths = [write_synthetic_run(Path(tmp) / f"run{i}", rng, geometry) for i in range(SYNTH_PLACEMENTS)]
        runs = [load_raw_run(p) for p in paths]
        placements = [to_placement(r) for r in runs]
        table = calibrate(placements, geometry, grid, DEFAULT_DEGREE)
        error = max_relative_gain_error(table, truth)
        check(error < SELFTEST_GAIN_TOLERANCE, f"верная конфигурация: макс. ошибка gain {error:.4%} < {SELFTEST_GAIN_TOLERANCE:.0%}")
        check(bool(np.all(table.gains >= 1.0)), "все gain >= 1")
        minima = table.gains.min(axis=(0, 1))
        check(bool(np.all(minima < MIN_GAIN_UPPER_BOUND)), f"минимум gain по каналам ≈ 1: {np.round(minima, 5).tolist()}")
        check(float(truth.max()) > 2.0, f"синтетическая виньетка достаточно сильная (макс. истинный gain {truth.max():.2f})")
        # Конвенция сетки: крайние узлы лежат в первом и последнем пикселе.
        gx, gy = grid_pixel_positions(geometry.width, geometry.height, grid)
        check(gx[0] == 0 and gy[0] == 0 and gx[grid.columns - 1] == geometry.width - 1 and gy[-1] == geometry.height - 1,
              "конвенция сетки: узлы (0,0) и (columns-1, rows-1) в пикселях 0 и W-1/H-1")
        check(table_points(table)[1][0] == round(float(table.gains[0, 1, 0]), GAIN_DECIMALS)
              and len(table_points(table)) == grid.columns * grid.rows, "порядок точек row-major, 4 gain на точку")
        # Без экстраполяции: угловой узел равен модели в зажатой точке (центр крайнего блока), а не в пикселе (0, 0).
        model = fit_model(placements, geometry, DEFAULT_DEGREE)
        corner_x, corner_y = block_centers(1)[0], block_centers(1)[0]
        dense_x, dense_y = sample_positions(geometry, grid, grid_pixel_positions)
        expected_corner = model.evaluate(dense_x, dense_y).max(axis=1) / model.evaluate(np.array([corner_x]), np.array([corner_y]))[:, 0]
        check(bool(np.allclose(table.gains[0, 0], expected_corner, rtol=1e-9)),
              "угловой узел = max/модель в центре крайнего блока (зажим, без экстраполяции)")
        # Заведомо неверное сопоставление G_even/G_odd должно провалить проверку (иначе тест ничего не защищает).
        swapped = list(channels_of_positions(SYNTH_CFA))
        swapped = [{CHANNEL_G_EVEN: CHANNEL_G_ODD, CHANNEL_G_ODD: CHANNEL_G_EVEN}.get(c, c) for c in swapped]
        wrong_channels = calibrate([to_placement(r, tuple(swapped)) for r in runs], geometry, grid, DEFAULT_DEGREE)
        wrong_error = max_relative_gain_error(wrong_channels, truth)
        check(wrong_error > SELFTEST_GAIN_TOLERANCE, f"подмена G_even/G_odd ловится: ошибка {wrong_error:.2%} > 1 %")

        # Заведомо неверная конвенция сетки: узлы в центрах ячеек вместо крайних пикселей.
        def cell_centre_positions(width: int, height: int, spec: GridSpec) -> tuple[np.ndarray, np.ndarray]:
            x = (np.arange(spec.columns) + 0.5) / spec.columns * width - 0.5
            y = (np.arange(spec.rows) + 0.5) / spec.rows * height - 0.5
            grid_x, grid_y = np.meshgrid(x, y)
            return grid_x.ravel(), grid_y.ravel()

        wrong_grid = calibrate(placements, geometry, grid, DEFAULT_DEGREE, grid_positions=cell_centre_positions)
        wrong_grid_error = max_relative_gain_error(wrong_grid, truth)
        check(wrong_grid_error > SELFTEST_GAIN_TOLERANCE, f"неверная конвенция сетки ловится: ошибка {wrong_grid_error:.2%} > 1 %")
        # Неверная (меньшая) степень: проверка, что степень вообще важна и тест чувствителен к качеству подгонки.
        low_degree = calibrate(placements, geometry, grid, 1)
        check(max_relative_gain_error(low_degree, truth) > SELFTEST_GAIN_TOLERANCE, "слишком низкая степень ловится")
    if failures:
        print(f"САМОТЕСТ ПРОВАЛЕН: {len(failures)} проверок")
        return 1
    print("самотест пройден")
    return 0
