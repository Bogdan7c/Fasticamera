"""Сериализация таблицы в итоговый JSON.

Часть пакета `shading_calibration` (см. `pc/shading_calibrate.py`).
"""

from __future__ import annotations

import json

from .core import (
    GAIN_DECIMALS,
    GainTable,
)


# ---------------------------------------------------------------------------------------------
# Выходной файл
# ---------------------------------------------------------------------------------------------

def table_points(table: GainTable) -> list[list[float]]:
    """Точки таблицы построчно (row-major), по 4 gain [R, G_even, G_odd, B], округление до GAIN_DECIMALS."""
    flat = table.gains.reshape(-1, 4)
    return [[round(float(v), GAIN_DECIMALS) for v in point] for point in flat]


def render_document(document: dict) -> str:
    """JSON с отступами, но каждая точка таблицы — в одной строке (иначе файл раздувается в 5000 строк)."""
    point_markers: dict[str, list[list[float]]] = {}
    for camera in document["cameras"]:
        marker = f"__POINTS_{camera['camera_id']}__"
        point_markers[marker] = camera["grid"]["points"]
        camera["grid"] = {**camera["grid"], "points": marker}
    text = json.dumps(document, ensure_ascii=False, indent=2)
    for marker, points in point_markers.items():
        lines = ",\n".join("          " + json.dumps(point) for point in points)
        text = text.replace(json.dumps(marker), "[\n" + lines + "\n        ]")
    return text + "\n"


def max_corner_gain(table: GainTable) -> float:
    """Максимальный gain в таблице по всем каналам (по сути — самый тёмный угол)."""
    return float(table.gains.max())
