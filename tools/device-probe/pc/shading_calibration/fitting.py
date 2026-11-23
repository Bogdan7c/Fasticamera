"""Подгонка полинома по каналам и построение таблицы усилений.

Часть пакета `shading_calibration` (см. `pc/shading_calibrate.py`).
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np

from .core import (
    CalibrationError,
    GainTable,
    GridPositionFn,
    GridSpec,
    Placement,
    SensorGeometry,
    block_centers,
    grid_pixel_positions,
    grid_sample_positions,
    sample_positions,
)


# ---------------------------------------------------------------------------------------------
# Подгонка полинома
# ---------------------------------------------------------------------------------------------

@dataclass(frozen=True)
class ShadingModel:
    """Полином по каналам: coefficients (4, число членов); координаты — непрерывные пиксели активного массива."""

    degree: int
    width: int
    height: int
    coefficients: np.ndarray

    def evaluate(self, x: np.ndarray, y: np.ndarray) -> np.ndarray:
        """Значения модели (4, N) в непрерывных координатах (x, y)."""
        basis = legendre_basis(self.degree, x / self.width * 2 - 1, y / self.height * 2 - 1)
        return (basis @ self.coefficients.T).T


def legendre_basis(degree: int, u: np.ndarray, v: np.ndarray) -> np.ndarray:
    """Матрица базиса (N, члены): L_i(u) * L_j(v) при i + j <= degree (полный полином данной ПОЛНОЙ степени)."""
    full = np.polynomial.legendre.legvander2d(u, v, [degree, degree])  # столбец i*(degree+1)+j
    index_i, index_j = np.divmod(np.arange((degree + 1) ** 2), degree + 1)
    return full[:, index_i + index_j <= degree]


def fit_model(placements: list[Placement], geometry: SensorGeometry, degree: int) -> ShadingModel:
    """МНК-подгонка среднего по укладкам нормированного поля, отдельно для каждого канала."""
    if not placements:
        raise CalibrationError("нет ни одной укладки для подгонки")
    mean_planes = np.mean([p.planes for p in placements], axis=0)  # (4, by, bx)
    x_grid, y_grid = np.meshgrid(block_centers(geometry.blocks_x), block_centers(geometry.blocks_y))
    basis = legendre_basis(degree, x_grid.ravel() / geometry.width * 2 - 1, y_grid.ravel() / geometry.height * 2 - 1)
    if basis.shape[0] < basis.shape[1]:
        raise CalibrationError(f"степень {degree} требует {basis.shape[1]} блоков, есть {basis.shape[0]}")
    targets = mean_planes.reshape(4, -1).T  # (N, 4)
    coefficients, *_ = np.linalg.lstsq(basis, targets, rcond=None)
    return ShadingModel(degree=degree, width=geometry.width, height=geometry.height, coefficients=coefficients.T)


def build_gain_table(
    model: ShadingModel,
    geometry: SensorGeometry,
    grid: GridSpec,
    grid_positions: GridPositionFn = grid_pixel_positions,
) -> GainTable:
    """gain = max(модель) / модель(узла); максимум — по центрам блоков и узлам вместе (вся рабочая область)."""
    dense_x, dense_y = sample_positions(geometry, grid, grid_positions)
    model_max = model.evaluate(dense_x, dense_y).max(axis=1)  # (4,)
    node_x, node_y = grid_sample_positions(geometry, grid, grid_positions)
    at_grid = model.evaluate(node_x, node_y)  # (4, rows*columns), без экстраполяции за измеренную область
    if np.any(at_grid <= 0):
        raise CalibrationError("модель неположительна в узле сетки: подгонка расходится, уменьшите степень")
    gains = (model_max[:, None] / at_grid).T.reshape(grid.rows, grid.columns, 4)
    return GainTable(geometry=geometry, grid=grid, gains=gains)


def calibrate(
    placements: list[Placement],
    geometry: SensorGeometry,
    grid: GridSpec,
    degree: int,
    grid_positions: GridPositionFn = grid_pixel_positions,
) -> GainTable:
    """Полный цикл: подгонка по укладкам → таблица усилений."""
    return build_gain_table(fit_model(placements, geometry, degree), geometry, grid, grid_positions)
