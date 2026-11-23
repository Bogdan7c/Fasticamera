"""Фикстура «плоское поле» для тестов Rust (формат test-assets/isp-model).

Часть пакета `shading_calibration` (см. `pc/shading_calibrate.py`).
"""

from __future__ import annotations

import json
from pathlib import Path
import sys

import numpy as np

from .core import (
    BLOCK,
    CFA_LAYOUT,
    RawRun,
)
from .loading import (
    load_raw_run,
)


# ---------------------------------------------------------------------------------------------
# Фикстура «плоское поле» для тестов Rust
# ---------------------------------------------------------------------------------------------

def build_fixture_mosaic(run: RawRun) -> np.ndarray:
    """Мозаика u16 размера (2*by, 2*bx): ячейка 2x2 блока (i, j) хранит 4 средних блока в ИХ позициях буфера.

    Это «уменьшенный сенсор» с той же фазой CFA, что и буфер; чёрный НЕ вычтен.
    """
    blocks_y, blocks_x = run.block_means.shape[1:]
    mosaic = np.zeros((2 * blocks_y, 2 * blocks_x), dtype="<u2")
    for position in range(4):
        row, col = divmod(position, 2)
        mosaic[row::2, col::2] = np.rint(run.block_means[position]).astype("<u2")
    return mosaic


def write_fixture(run_json: Path, out_dir: Path, name: str) -> Path:
    """Пишет фикстуру в формате test-assets/isp-model (манифест + .bin) из отложенного прогона."""
    sys.path.insert(0, str(Path(__file__).resolve().parents[4] / "tools" / "isp-reference"))
    from asset_io import write_asset  # noqa: PLC0415 — путь добавлен строкой выше

    run = load_raw_run(run_json)
    mosaic = build_fixture_mosaic(run)
    extra = {
        "cfa_arrangement": CFA_LAYOUT[run.cfa],
        "cfa_arrangement_code": run.cfa,
        "black_level_per_position": [round(float(v), 4) for v in run.dynamic_black],
        "black_level_note": "динамический чёрный (среднее по кадрам), порядок позиций буфера: (0,0), (0,1), (1,0), (1,1)",
        "white_level": run.white_level,
        "source_run": run.run_id,
        "source_mode": run.mode,
        "scaled_from_active_array": {"width": run.width, "height": run.height},
        "block_size_px": BLOCK,
        "note": "Отложен от калибровки затенения (14f): в подгонку lens-shading.json не входил. "
                "Ячейка 2x2 = средние блока 32x32 по 4 позициям CFA буфера, чёрный не вычтен.",
    }
    command = ("tools/device-probe/pc/shading_calibrate.py --write-fixture <каталог прогонов>/"
               f"{run.run_id}/{run.mode}.json --fixture-dir test-assets/isp-model --fixture-name {name}")
    return write_asset(
        out_dir, name, mosaic,
        description=f"Плоское поле основной камеры ({run.mode}), уменьшенный сенсор {mosaic.shape[1]}x{mosaic.shape[0]}, отложен от калибровки затенения",
        generator=command, extra=extra,
    )
