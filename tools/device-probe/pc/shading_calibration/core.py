"""Константы, модели данных и геометрия калибровки затенения (каналы, сетка, зажим без экстраполяции).

Часть пакета `shading_calibration` (см. `pc/shading_calibrate.py`).
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Callable

import numpy as np


# ---------------------------------------------------------------------------------------------
# Константы (каждая объяснена)
# ---------------------------------------------------------------------------------------------

# Схема выходного файла (ровно такая строка ожидается на стороне Rust).
SCHEMA = "fasticamera.lens-shading-calibration/1"
# Фаза прогона, по которой калибруем: ручная экспозиция + запрошенная карта HAL, остальные фазы не нужны.
CALIBRATION_PHASE = "base"
# Сторона блока в пикселях буфера: совпадает с BLOCK_SIZE_PIXELS в Kotlin и с `shading_analyze.py`.
BLOCK = 32
# Число каналов карты Camera2 и их порядок.
CHANNEL_NAMES = ("R", "G_even", "G_odd", "B")
CHANNEL_R, CHANNEL_G_EVEN, CHANNEL_G_ODD, CHANNEL_B = range(4)
# Раскладка CFA Camera2 (SENSOR_INFO_COLOR_FILTER_ARRANGEMENT): цвета позиций буфера (строка*2 + столбец).
CFA_LAYOUT = {0: "RGGB", 1: "GRBG", 2: "GBRG", 3: "BGGR"}
# Центр для нормировки плоскости: половина стороны окна в блоках (окно 4x4 блока = 128x128 px).
CENTER_HALF_WINDOW = 2
# Отступ угловой точки отчёта от края в блоках (самый крайний блок бывает «грязным» из-за края диффузора).
CORNER_INSET_BLOCKS = 2
# Бинов радиального профиля (как в shading_analyze.py).
RADIAL_BINS = 10
# Умолчания подгонки (почему именно они — в докстринге модуля).
DEFAULT_DEGREE = 16
DEFAULT_GRID_COLUMNS = 33
DEFAULT_GRID_ROWS = 25
# Знаков после запятой у gain в итоговом файле.
GAIN_DECIMALS = 6
# Допуск самотеста: относительная ошибка восстановленного gain в узлах сетки.
SELFTEST_GAIN_TOLERANCE = 0.01
# Верхняя граница минимального gain таблицы: максимум модели может лежать не в узле, а в центре блока.
MIN_GAIN_UPPER_BOUND = 1.01
# Допуск «gain == 1» (для тестов формата).
UNITY_TOLERANCE = 1e-9


class CalibrationError(Exception):
    """Ошибка входных данных или метода; сообщение должно быть понятно без трассировки."""


# ---------------------------------------------------------------------------------------------
# Модели данных
# ---------------------------------------------------------------------------------------------

@dataclass(frozen=True)
class RawRun:
    """Один прогон `shading`: сырые средние DN по блокам и всё, что нужно для их приведения к линейным."""

    label: str                    # человекочитаемый идентификатор прогона (стенд/каталог)
    run_id: str                   # имя каталога прогона
    mode: str                     # режим зонда (main43, wide43, front30)
    cfa: int                      # раскладка CFA Camera2 (0..3)
    width: int                    # ширина активного массива (= буфера), px
    height: int                   # высота активного массива (= валидных строк буфера), px
    white_level: float            # белый уровень сенсора, DN
    dynamic_black: np.ndarray     # (4,) средний динамический чёрный по позициям буфера, DN
    block_means: np.ndarray       # (4, blocks_y, blocks_x) среднее DN блока по позициям буфера (чёрный НЕ вычтен)


@dataclass(frozen=True)
class Placement:
    """Укладка: плоскости каналов [R, G_even, G_odd, B], нормированные на свой центр."""

    label: str
    planes: np.ndarray            # (4, blocks_y, blocks_x), центр = 1


@dataclass(frozen=True)
class SensorGeometry:
    """Геометрия активного массива и блоков, общая для всех укладок одной камеры."""

    width: int
    height: int
    blocks_x: int
    blocks_y: int


@dataclass(frozen=True)
class GridSpec:
    """Размер gain-таблицы."""

    columns: int
    rows: int


@dataclass(frozen=True)
class GainTable:
    """Таблица усилений: gains[row, column, channel], канал в порядке CHANNEL_NAMES."""

    geometry: SensorGeometry
    grid: GridSpec
    gains: np.ndarray

    def sample_block_centers(self) -> np.ndarray:
        """Билинейно интерполирует таблицу в центры блоков: (4, blocks_y, blocks_x). Так её применит потребитель."""
        geo = self.geometry
        # Координата центра блока непрерывная (пиксель i покрывает [i, i+1)); узлы заданы по индексам пикселей.
        x_index = (block_centers(geo.blocks_x) - 0.5) / (geo.width - 1) * (self.grid.columns - 1)
        y_index = (block_centers(geo.blocks_y) - 0.5) / (geo.height - 1) * (self.grid.rows - 1)
        col0, col_frac = _split_grid_index(x_index, self.grid.columns)
        row0, row_frac = _split_grid_index(y_index, self.grid.rows)
        g = self.gains  # (rows, cols, 4)
        top = g[row0][:, col0] * (1 - col_frac)[None, :, None] + g[row0][:, col0 + 1] * col_frac[None, :, None]
        bottom = g[row0 + 1][:, col0] * (1 - col_frac)[None, :, None] + g[row0 + 1][:, col0 + 1] * col_frac[None, :, None]
        blended = top * (1 - row_frac)[:, None, None] + bottom * row_frac[:, None, None]  # (by, bx, 4)
        return np.moveaxis(blended, -1, 0)


def _split_grid_index(index: np.ndarray, count: int) -> tuple[np.ndarray, np.ndarray]:
    """Дробный индекс узла → (индекс левого узла, доля), с зажатием в [0, count-1]."""
    clipped = np.clip(index, 0.0, count - 1.0)
    left = np.minimum(np.floor(clipped).astype(int), count - 2)
    return left, clipped - left


# ---------------------------------------------------------------------------------------------
# Геометрия и каналы
# ---------------------------------------------------------------------------------------------

def block_centers(blocks: int) -> np.ndarray:
    """Непрерывные координаты центров блоков вдоль оси: (i + 0.5) * BLOCK."""
    return (np.arange(blocks) + 0.5) * BLOCK


def grid_pixel_positions(width: int, height: int, grid: GridSpec) -> tuple[np.ndarray, np.ndarray]:
    """КОНВЕНЦИЯ СЕТКИ: индексы пикселей (x по столбцам, y по строкам) узлов, строки подряд.

    Узел (c, r) = ((W-1)/(columns-1)*c, (H-1)/(rows-1)*r): крайние узлы — в первом и последнем пикселе.
    """
    x = (width - 1) / (grid.columns - 1) * np.arange(grid.columns)
    y = (height - 1) / (grid.rows - 1) * np.arange(grid.rows)
    grid_x, grid_y = np.meshgrid(x, y)  # (rows, columns)
    return grid_x.ravel(), grid_y.ravel()


GridPositionFn = Callable[[int, int, GridSpec], tuple[np.ndarray, np.ndarray]]


def channels_of_positions(cfa: int) -> tuple[int, ...]:
    """Канал Camera2 каждой позиции буфера (строка*2 + столбец) для раскладки CFA.

    G_even — зелёный в чётных строках: у позиции в строке 0 это G_even, в строке 1 — G_odd.
    """
    if cfa not in CFA_LAYOUT:
        raise CalibrationError(f"неизвестная раскладка CFA {cfa}: ожидалось одно из {sorted(CFA_LAYOUT)}")
    channels = []
    for position, color in enumerate(CFA_LAYOUT[cfa]):
        row = position // 2
        if color == "R":
            channels.append(CHANNEL_R)
        elif color == "B":
            channels.append(CHANNEL_B)
        else:
            channels.append(CHANNEL_G_EVEN if row == 0 else CHANNEL_G_ODD)
    return tuple(channels)


def clamp_to_measured(geometry: SensorGeometry, x: np.ndarray, y: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """Зажимает непрерывные координаты в прямоугольник центров крайних блоков (без экстраполяции).

    Данные есть только до центров крайних блоков, то есть до 16 px от края кадра. Полином степени 16 на этих
    последних 16 px экстраполирует: угловой gain между степенями 12 и 16 отличался на ~17 %, а шум подгонки там
    усиливается в десятки раз. Поэтому узлы во внешних 16 px берут значение модели в ближайшей измеренной точке
    по каждой оси (u и v зажимаются независимо). Тот же зажим применяется и при поиске максимума модели.
    """
    return (np.clip(x, block_centers(1)[0], (geometry.blocks_x - 0.5) * BLOCK),
            np.clip(y, block_centers(1)[0], (geometry.blocks_y - 0.5) * BLOCK))


def grid_sample_positions(geometry: SensorGeometry, grid: GridSpec, grid_positions: GridPositionFn) -> tuple[np.ndarray, np.ndarray]:
    """Непрерывные координаты, в которых считается модель для узлов сетки: центр пикселя узла, зажатый в измеренную область."""
    grid_x, grid_y = grid_positions(geometry.width, geometry.height, grid)
    # Узел задан индексом пикселя; полином считаем в центре пикселя (+0.5), см. докстринг модуля.
    return clamp_to_measured(geometry, grid_x + 0.5, grid_y + 0.5)


def sample_positions(geometry: SensorGeometry, grid: GridSpec, grid_positions: GridPositionFn) -> tuple[np.ndarray, np.ndarray]:
    """Непрерывные координаты всех точек, по которым ищется максимум модели: центры блоков + узлы сетки (зажатые)."""
    block_x, block_y = np.meshgrid(block_centers(geometry.blocks_x), block_centers(geometry.blocks_y))
    node_x, node_y = grid_sample_positions(geometry, grid, grid_positions)
    return np.concatenate([block_x.ravel(), node_x]), np.concatenate([block_y.ravel(), node_y])
