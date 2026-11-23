#!/usr/bin/env python3
"""Разбор замера карты затенения (сценарий `shading` зонда), сессия 14f.

Вход: каталог прогона, полученный `python3 pc/probe.py pull-captures oneplus`
(`captures/<run>/` с файлами `<mode>.json`, `<mode>_<phase>.blocks.f32`, `<mode>_base.frame{0,1}.raw16`).
Можно указать несколько каталогов (по одному на режим).

Что считается по каждому режиму и фазе:
  1. Виньетирование «сырого» RAW: отношение угол/центр и радиальный профиль по четырём каналам (в разах и в EV).
  2. Остаточная неравномерность после применения средней карты затенения кадра при двух гипотезах геометрии:
       A — валидная область буфера == весь активный массив, растянутый (допущение isp-model);
       B — данные буфера — центральный кроп активного массива (с масштабом, если буфер биннингован).
  3. Изменчивость карты: между кадрами, между фазами и относительно `base`.
  4. Совпадение пикселей `base` и `map_off` (запрос карты не должен менять RAW).
  5. Шум по двум кадрам base против предсказания из карты и профиля шума сенсора.

Запуск (numpy есть в окружении ISP-референса):
  ~/.cache/fasticamera/isp-reference-venv/bin/python -I pc/shading_analyze.py <каталог> [<каталог> ...]
  ~/.cache/fasticamera/isp-reference-venv/bin/python -I pc/shading_analyze.py --selftest
"""

from __future__ import annotations

import argparse
import json
import math
import sys
import tempfile
from pathlib import Path

import numpy as np

# Сторона блока в пикселях буфера: совпадает с BLOCK_SIZE_PIXELS в Kotlin.
BLOCK = 32
# Отсчётов одной CFA-плоскости по стороне блока.
SAMPLES_PER_BLOCK_SIDE = BLOCK // 2
# Каналы карты затенения Camera2 в порядке интерливинга.
CHANNELS = ["R", "G_even", "G_odd", "B"]
# Цвет каждой позиции буфера (строка*2 + столбец) для раскладок CFA Camera2: 0 RGGB, 1 GRBG, 2 GBRG, 3 BGGR.
CFA_LAYOUT = {0: "RGGB", 1: "GRBG", 2: "GBRG", 3: "BGGR"}
# Число бинов радиального профиля.
RADIAL_BINS = 10
# Схема отчёта зонда, по которой отбираются JSON (должна совпадать с REPORT_SCHEMA в Kotlin).
REPORT_SCHEMA = "shading_probe_v1"
# Минимальная сетка блоков для «угол с отступом + центр 3x3».
MIN_GRID = 5


# ---------------------------------------------------------------------------------------------
# Каналы и геометрия
# ---------------------------------------------------------------------------------------------

def plane_channel(cfa: int, plane: int, row_offset: int = 0, col_offset: int = 0) -> int:
    """Индекс канала карты (0 R, 1 G_even, 2 G_odd, 3 B) для позиции буфера `plane` = строка*2 + столбец.

    G_even — зелёный в чётных строках АКТИВНОГО массива; `row_offset` — смещение буфера по строкам относительно него.
    """
    row, col = divmod(plane, 2)
    row_active = (row + row_offset) % 2
    col_active = (col + col_offset) % 2
    color = CFA_LAYOUT[cfa][row_active * 2 + col_active]
    if color == "R":
        return 0
    if color == "B":
        return 3
    return 1 if row_active == 0 else 2


def block_positions(grid_x: int, grid_y: int, plane: int) -> tuple[np.ndarray, np.ndarray]:
    """Средние координаты (в индексах пикселей буфера) отсчётов данной плоскости в каждом блоке."""
    row, col = divmod(plane, 2)
    # Отсчёты плоскости внутри блока стоят через 2 пикселя: среднее положение = смещение плоскости + (SAMPLES-1).
    x = BLOCK * np.arange(grid_x) + col + (SAMPLES_PER_BLOCK_SIDE - 1)
    y = BLOCK * np.arange(grid_y) + row + (SAMPLES_PER_BLOCK_SIDE - 1)
    return x.astype(np.float64), y.astype(np.float64)


def map_coordinates(hypothesis: str, geometry: dict, plane: int, grid_x: int, grid_y: int) -> tuple[np.ndarray, np.ndarray] | None:
    """Координаты узлов карты (gx по столбцам, gy по строкам; дробные) для блоков сетки.

    Конвенция Camera2: узел (i, j) карты N x M стоит в пикселе ((W-1)/(N-1)*i, (H-1)/(M-1)*j) активного массива.
    Возвращает None, если гипотеза геометрически невозможна.
    """
    x, y = block_positions(grid_x, grid_y, plane)
    buffer_w, valid_rows = geometry["buffer_width"], geometry["valid_rows"]
    active_w, active_h = geometry["active_width"], geometry["active_height"]
    nodes_x, nodes_y = geometry["map_columns"], geometry["map_rows"]
    if hypothesis == "A":
        # Валидная область буфера растянута на весь активный массив (угол в угол).
        gx = x / (buffer_w - 1) * (nodes_x - 1)
        gy = y / (valid_rows - 1) * (nodes_y - 1)
        return gx, gy
    if hypothesis == "B":
        # Буфер — центральный кроп активного массива; scale — пикселей буфера на пиксель активного (биннинг < 1).
        scale = buffer_w / active_w
        offset_y = (active_h - valid_rows / scale) / 2
        if offset_y < -1e-6:
            return None
        px = (x + 0.5) / scale - 0.5
        py = (y + 0.5) / scale - 0.5 + offset_y
        return px / (active_w - 1) * (nodes_x - 1), py / (active_h - 1) * (nodes_y - 1)
    raise ValueError(f"неизвестная гипотеза {hypothesis}")


def bilinear(gains: np.ndarray, channel: int, gx: np.ndarray, gy: np.ndarray) -> np.ndarray:
    """Билинейная выборка канала карты (rows, cols, 4) в сетке (gy по строкам, gx по столбцам); за краем — зажим."""
    rows, cols = gains.shape[0], gains.shape[1]
    gx = np.clip(gx, 0, cols - 1)
    gy = np.clip(gy, 0, rows - 1)
    x0 = np.minimum(np.floor(gx).astype(int), cols - 2)
    y0 = np.minimum(np.floor(gy).astype(int), rows - 2)
    fx = (gx - x0)[None, :]
    fy = (gy - y0)[:, None]
    plane = gains[:, :, channel]
    top = plane[np.ix_(y0, x0)] * (1 - fx) + plane[np.ix_(y0, x0 + 1)] * fx
    bottom = plane[np.ix_(y0 + 1, x0)] * (1 - fx) + plane[np.ix_(y0 + 1, x0 + 1)] * fx
    return top * (1 - fy) + bottom * fy


# ---------------------------------------------------------------------------------------------
# Метрики по сетке блоков
# ---------------------------------------------------------------------------------------------

def corner_blocks(arr: np.ndarray) -> np.ndarray:
    """Четыре угловых блока с отступом на блок от края."""
    rows, cols = arr.shape
    return np.array([arr[1, 1], arr[1, cols - 2], arr[rows - 2, 1], arr[rows - 2, cols - 2]])


def centre_blocks(arr: np.ndarray) -> np.ndarray:
    """Центральные 3x3 блока."""
    rows, cols = arr.shape
    y0, x0 = (rows - 3) // 2, (cols - 3) // 2
    return arr[y0:y0 + 3, x0:x0 + 3]


def to_ev(ratio: float | None) -> float | None:
    return None if ratio is None or ratio <= 0 else math.log2(ratio)


def corner_centre(arr: np.ndarray) -> float | None:
    if min(arr.shape) < MIN_GRID:
        return None
    return float(corner_blocks(arr).mean() / centre_blocks(arr).mean())


def radial_profile(arr: np.ndarray) -> list[float | None]:
    """Среднее по блокам в кольцах нормированного радиуса (0 — центр, 1 — угол), делённое на центр 3x3."""
    rows, cols = arr.shape
    yy, xx = np.mgrid[0:rows, 0:cols]
    dx = (xx + 0.5 - cols / 2) / (cols / 2)
    dy = (yy + 0.5 - rows / 2) / (rows / 2)
    radius = np.hypot(dx, dy) / math.sqrt(2)
    centre = centre_blocks(arr).mean()
    profile: list[float | None] = []
    for index in range(RADIAL_BINS):
        low, high = index / RADIAL_BINS, (index + 1) / RADIAL_BINS
        mask = (radius >= low) & ((radius < high) if index < RADIAL_BINS - 1 else (radius <= high))
        profile.append(float(arr[mask].mean() / centre) if mask.any() else None)
    return profile


def flatness(arr: np.ndarray) -> dict:
    """Неравномерность одной плоскости: угол/центр, макс. отклонение от центра, верх против низа."""
    centre = centre_blocks(arr).mean()
    normalized = arr / centre
    quarter = max(1, arr.shape[0] // 4)
    top, bottom = normalized[:quarter].mean(), normalized[-quarter:].mean()
    ratio = corner_centre(arr)
    return {
        "corner_centre": ratio,
        "corner_centre_ev": to_ev(ratio),
        "max_abs_deviation": float(np.abs(normalized - 1).max()),
        "top_bottom_asymmetry": float(top / bottom - 1),
    }


# ---------------------------------------------------------------------------------------------
# Чтение прогона
# ---------------------------------------------------------------------------------------------

def parse_rect(text: str) -> tuple[int, int, int, int]:
    """Rect.flattenToString: 'left top right bottom' (допускаем и вид [l,t][r,b])."""
    numbers = [int(token) for token in "".join(ch if ch.isdigit() or ch == "-" else " " for ch in text).split()]
    if len(numbers) != 4:
        raise ValueError(f"не Rect: {text!r}")
    return numbers[0], numbers[1], numbers[2], numbers[3]


def frame_gains(frame: dict) -> np.ndarray | None:
    """Карта кадра как массив (rows, columns, 4) или None, если карты нет."""
    shading_map = frame.get("lens_shading_map")
    if not shading_map:
        return None
    flat = np.asarray(shading_map["gains"], dtype=np.float64)
    return flat.reshape(shading_map["rows"], shading_map["columns"], 4)


def load_blocks(directory: Path, name: str, grid: dict) -> np.ndarray:
    data = np.fromfile(directory / name, dtype="<f4")
    expected = 4 * grid["blocks_y"] * grid["blocks_x"]
    if data.size != expected:
        raise ValueError(f"{name}: {data.size} значений, ожидалось {expected}")
    return data.reshape(4, grid["blocks_y"], grid["blocks_x"]).astype(np.float64)


def mean_map(frames: list[dict]) -> tuple[np.ndarray | None, list[np.ndarray]]:
    maps = [gains for gains in (frame_gains(frame) for frame in frames) if gains is not None]
    return (np.mean(maps, axis=0) if maps else None), maps


# ---------------------------------------------------------------------------------------------
# Анализ фазы
# ---------------------------------------------------------------------------------------------

class ModeContext:
    """Статика режима: уровни, CFA, геометрия активного массива и буфера."""

    def __init__(self, report: dict):
        characteristics = report["characteristics"]
        self.cfa = characteristics["cfa"]
        self.black = np.asarray(characteristics["black_level_pattern"], dtype=np.float64)
        self.white = float(characteristics["white_level"])
        left, top, right, bottom = parse_rect(characteristics["active_array"])
        self.active_width, self.active_height = right - left, bottom - top
        self.buffer_width = report["buffer_geometry"]["width"]
        self.channels = [plane_channel(self.cfa, plane) for plane in range(4)]


def geometry_for(context: ModeContext, valid_rows: int, gains: np.ndarray) -> dict:
    return {
        "buffer_width": context.buffer_width, "valid_rows": valid_rows,
        "active_width": context.active_width, "active_height": context.active_height,
        "map_columns": gains.shape[1], "map_rows": gains.shape[0],
    }


def correct(context: ModeContext, means: np.ndarray, gains: np.ndarray, hypothesis: str, valid_rows: int) -> np.ndarray | None:
    """Блоки (4, by, bx) после вычитания чёрного и применения карты по гипотезе; None — гипотеза невозможна."""
    _, grid_y, grid_x = means.shape
    geometry = geometry_for(context, valid_rows, gains)
    corrected = np.empty_like(means)
    for plane in range(4):
        coords = map_coordinates(hypothesis, geometry, plane, grid_x, grid_y)
        if coords is None:
            return None
        corrected[plane] = (means[plane] - context.black[plane]) * bilinear(gains, context.channels[plane], *coords)
    return corrected


def per_channel(values: list, context: ModeContext) -> dict:
    return {CHANNELS[context.channels[plane]]: values[plane] for plane in range(4)}


def analyze_raw(context: ModeContext, means: np.ndarray) -> dict:
    """Виньетирование сырого RAW (без карты): угол/центр и радиальный профиль по каналам."""
    black_free = [means[plane] - context.black[plane] for plane in range(4)]
    ratios = [corner_centre(arr) for arr in black_free]
    return {
        "corner_centre": per_channel(ratios, context),
        "corner_centre_ev": per_channel([to_ev(r) for r in ratios], context),
        "radial_profile": per_channel([radial_profile(arr) for arr in black_free], context),
        "radial_profile_ev": per_channel([[to_ev(v) for v in radial_profile(arr)] for arr in black_free], context),
        "top_bottom_asymmetry": per_channel([flatness(arr)["top_bottom_asymmetry"] for arr in black_free], context),
    }


def analyze_correction(context: ModeContext, means: np.ndarray, gains: np.ndarray | None, valid_rows: int) -> dict:
    """Плоскость после применения средней карты кадра при гипотезах A и B."""
    if gains is None:
        return {"error": "карты нет"}
    result = {}
    for hypothesis in ("A", "B"):
        corrected = correct(context, means, gains, hypothesis, valid_rows)
        if corrected is None:
            result[hypothesis] = {"error": "гипотеза геометрически невозможна"}
            continue
        result[hypothesis] = per_channel([flatness(corrected[plane]) for plane in range(4)], context)
    return result


def map_summary(gains: np.ndarray | None) -> dict | None:
    if gains is None:
        return None
    rows, cols = gains.shape[0], gains.shape[1]
    centre = gains[rows // 2, cols // 2]
    corners = np.mean([gains[0, 0], gains[0, -1], gains[-1, 0], gains[-1, -1]], axis=0)
    return {
        "columns": cols, "rows": rows,
        "centre_gain": dict(zip(CHANNELS, map(float, centre))),
        "corner_gain": dict(zip(CHANNELS, map(float, corners))),
        "corner_over_centre_gain": dict(zip(CHANNELS, map(float, corners / centre))),
    }


def max_relative_difference(gains: np.ndarray, reference: np.ndarray) -> dict:
    relative = np.abs(gains / reference - 1)
    return dict(zip(CHANNELS, (float(relative[:, :, channel].max()) for channel in range(4))))


def map_variation(maps: list[np.ndarray], phase_map: np.ndarray | None, base_map: np.ndarray | None) -> dict:
    """Изменчивость карты внутри фазы (кадр к кадру) и относительно средней карты `base`."""
    result: dict = {"frames_with_map": len(maps)}
    if phase_map is None:
        return result
    result["within_phase_max_rel_diff"] = {
        name: max(max_relative_difference(m, phase_map)[name] for m in maps) for name in CHANNELS
    }
    if base_map is not None and base_map.shape == phase_map.shape:
        result["vs_base_max_rel_diff"] = max_relative_difference(phase_map, base_map)
    elif base_map is not None:
        result["vs_base_max_rel_diff"] = "размеры карт различаются"
    return result


def noise_check(context: ModeContext, directory: Path, phase: dict, means: np.ndarray, gains: np.ndarray | None) -> dict:
    """Шум по двум кадрам base: измеренное и предсказанное (по профилю шума) отношение угол/центр."""
    frame_files = [name for name in phase.get("files", []) if name.endswith(".raw16")]
    if len(frame_files) < 2:
        return {"error": "нет двух кадров RAW16"}
    valid_rows, width = phase["valid_rows"], context.buffer_width
    frames = [np.fromfile(directory / name, dtype="<u2").reshape(valid_rows, width).astype(np.float64) for name in sorted(frame_files)[:2]]
    grid_y, grid_x = means.shape[1], means.shape[2]
    profile = next((f["noise_profile"] for f in phase["frames"] if f.get("noise_profile")), None)
    difference = (frames[0] - frames[1]) / math.sqrt(2)
    measured_var, predicted_var = [], []
    for plane in range(4):
        row, col = divmod(plane, 2)
        sub = difference[row::2, col::2][: grid_y * SAMPLES_PER_BLOCK_SIDE, : grid_x * SAMPLES_PER_BLOCK_SIDE]
        blocks = sub.reshape(grid_y, SAMPLES_PER_BLOCK_SIDE, grid_x, SAMPLES_PER_BLOCK_SIDE)
        measured_var.append(blocks.var(axis=(1, 3)))
        if profile:
            signal = (means[plane] - context.black[plane]) / (context.white - context.black[plane])
            slope, offset = profile[plane]
            predicted_var.append((context.white - context.black[plane]) ** 2 * (slope * signal + offset))
    result: dict = {"raw": {}, "corrected": {}}

    def ratio(variances: list[np.ndarray], gain_squared: list[np.ndarray | None]) -> list:
        out = []
        for plane in range(4):
            weight = gain_squared[plane] if gain_squared[plane] is not None else np.ones_like(variances[plane])
            scaled = variances[plane] * weight
            out.append(float(math.sqrt(corner_blocks(scaled).mean() / centre_blocks(scaled).mean())))
        return out

    no_gain: list = [None] * 4
    result["raw"]["measured_corner_centre"] = per_channel(ratio(measured_var, no_gain), context)
    if profile:
        result["raw"]["predicted_corner_centre"] = per_channel(ratio(predicted_var, no_gain), context)
    else:
        result["raw"]["predicted_corner_centre"] = "нет noise_profile"
    if gains is None:
        return result
    for hypothesis in ("A", "B"):
        geometry = geometry_for(context, valid_rows, gains)
        squares = []
        for plane in range(4):
            coords = map_coordinates(hypothesis, geometry, plane, grid_x, grid_y)
            squares.append(None if coords is None else bilinear(gains, context.channels[plane], *coords) ** 2)
        if any(s is None for s in squares):
            result["corrected"][hypothesis] = {"error": "гипотеза геометрически невозможна"}
            continue
        entry = {"measured_corner_centre": per_channel(ratio(measured_var, squares), context)}
        if profile:
            entry["predicted_corner_centre"] = per_channel(ratio(predicted_var, squares), context)
        result["corrected"][hypothesis] = entry
    return result


def analyze_mode(report: dict, directory: Path) -> dict:
    """Полный разбор одного режима по его отчёту и бинарным файлам."""
    context = ModeContext(report)
    phases = {entry["phase"]: entry for entry in report["phases"] if "grid" in entry}
    skipped = {entry["phase"]: entry["skipped_reason"] for entry in report["phases"] if "skipped_reason" in entry}
    loaded = {}
    for name, entry in phases.items():
        means = load_blocks(directory, next(f for f in entry["files"] if f.endswith(".blocks.f32")), entry["grid"])
        phase_map, maps = mean_map(entry["frames"])
        loaded[name] = (entry, means, phase_map, maps)
    base_map = loaded["base"][2] if "base" in loaded else None
    output: dict = {
        "mode": report["mode"], "run": report.get("run"), "cfa": CFA_LAYOUT[context.cfa],
        "channels_by_plane": [CHANNELS[c] for c in context.channels],
        "base_exposure": report.get("base_exposure"), "skipped_phases": skipped, "phases": {},
    }
    for name, (entry, means, phase_map, maps) in loaded.items():
        valid_rows = entry["valid_rows"]
        output["phases"][name] = {
            "valid_rows": valid_rows,
            "collected_frames": entry["collected_frames"],
            "raw": analyze_raw(context, means),
            "after_map": analyze_correction(context, means, phase_map, valid_rows),
            "map": map_summary(phase_map),
            "map_variation": map_variation(maps, phase_map, base_map),
        }
    if "base" in loaded and "map_off" in loaded:
        base_means, off_means = loaded["base"][1], loaded["map_off"][1]
        if base_means.shape == off_means.shape:
            relative = np.abs(base_means - off_means) / np.maximum(base_means - context.black[:, None, None], 1e-6)
            output["base_vs_map_off_max_rel_block_diff"] = float(relative.max())
        else:
            output["base_vs_map_off_max_rel_block_diff"] = "сетки различаются"
    if "base" in loaded:
        entry, means, phase_map, _ = loaded["base"]
        output["noise_base"] = noise_check(context, directory, entry, means, phase_map)
    return output


# ---------------------------------------------------------------------------------------------
# Вывод
# ---------------------------------------------------------------------------------------------

def fmt(value, digits: int = 3) -> str:
    return "—" if value is None or isinstance(value, str) else f"{value:.{digits}f}"


def markdown(summary: dict) -> str:
    lines = [f"## {summary['mode']} (CFA {summary['cfa']}, каналы по позициям {summary['channels_by_plane']})", ""]
    lines.append(f"База: {summary['base_exposure']}. Пропущено: {summary['skipped_phases'] or 'нет'}")
    lines += ["", "### Сырой RAW: угол/центр (разы | EV)", "", "| фаза | " + " | ".join(CHANNELS) + " |", "|---|" + "---|" * 4]
    for name, phase in summary["phases"].items():
        cells = [f"{fmt(phase['raw']['corner_centre'].get(c))} \\| {fmt(phase['raw']['corner_centre_ev'].get(c), 2)}" for c in CHANNELS]
        lines.append(f"| {name} | " + " | ".join(cells) + " |")
    for hypothesis in ("A", "B"):
        lines += ["", f"### После карты, гипотеза {hypothesis}: угол/центр | макс.откл. | верх/низ−1", "", "| фаза | " + " | ".join(CHANNELS) + " |", "|---|" + "---|" * 4]
        for name, phase in summary["phases"].items():
            after = phase["after_map"].get(hypothesis)
            if not isinstance(after, dict) or "error" in after:
                lines.append(f"| {name} | " + " | ".join(["—"] * 4) + " |")
                continue
            cells = [f"{fmt(after[c]['corner_centre'])} \\| {fmt(after[c]['max_abs_deviation'])} \\| {fmt(after[c]['top_bottom_asymmetry'])}" for c in CHANNELS]
            lines.append(f"| {name} | " + " | ".join(cells) + " |")
    lines += ["", "### Карта: усиление угол/центр по каналам и расхождение с base (макс. отн.)", "", "| фаза | угол/центр усиления | в кадре Δ | vs base Δ |", "|---|---|---|---|"]
    for name, phase in summary["phases"].items():
        gain = phase["map"]["corner_over_centre_gain"] if phase["map"] else None
        within = phase["map_variation"].get("within_phase_max_rel_diff")
        versus = phase["map_variation"].get("vs_base_max_rel_diff")
        show = lambda d: "—" if not isinstance(d, dict) else " ".join(f"{fmt(d[c], 4)}" for c in CHANNELS)
        lines.append(f"| {name} | {show(gain)} | {show(within)} | {show(versus)} |")
    lines += ["", f"Совпадение пикселей base и map_off, макс. относительная разница блока: {summary.get('base_vs_map_off_max_rel_block_diff', '—')}"]
    noise = summary.get("noise_base")
    if noise and "raw" in noise:
        lines += ["", "### Шум угол/центр (σ): измерено | предсказано", ""]
        lines.append("| вариант | " + " | ".join(CHANNELS) + " |")
        lines.append("|---|" + "---|" * 4)
        rows = [("сырой", noise["raw"])] + [(f"после карты {h}", noise["corrected"].get(h, {})) for h in ("A", "B")]
        for label, entry in rows:
            measured, predicted = entry.get("measured_corner_centre"), entry.get("predicted_corner_centre")
            cells = []
            for channel in CHANNELS:
                cells.append(f"{fmt(measured.get(channel)) if isinstance(measured, dict) else '—'} \\| {fmt(predicted.get(channel)) if isinstance(predicted, dict) else '—'}")
            lines.append(f"| {label} | " + " | ".join(cells) + " |")
    return "\n".join(lines)


def find_reports(directory: Path) -> list[tuple[Path, dict]]:
    reports = []
    for path in sorted(directory.rglob("*.json")):
        try:
            data = json.loads(path.read_text())
        except (json.JSONDecodeError, UnicodeDecodeError):
            continue
        if isinstance(data, dict) and data.get("schema") == REPORT_SCHEMA:
            reports.append((path, data))
    return reports


def analyze_directory(directory: Path) -> dict:
    """Разбирает все режимы каталога; JSON-сводка пишется рядом с отчётами."""
    reports = find_reports(directory)
    if not reports:
        raise SystemExit(f"в {directory} нет отчётов {REPORT_SCHEMA}")
    summaries: dict = {}
    for path, report in reports:
        summaries[report["mode"]] = analyze_mode(report, path.parent)
        (path.parent / "shading_analysis.json").write_text(json.dumps(summaries, indent=2, ensure_ascii=False))
    return summaries


# ---------------------------------------------------------------------------------------------
# Самопроверка: синтетический прогон с известным виньетированием и известной картой
# ---------------------------------------------------------------------------------------------

SELFTEST_ACTIVE = (640, 480)
SELFTEST_MAP = (17, 13)  # (столбцов, строк)
SELFTEST_BLACK, SELFTEST_WHITE, SELFTEST_LEVEL = 64.0, 1023.0, 0.30
SELFTEST_VIGNETTE = {0: 0.40, 1: 0.44, 2: 0.48, 3: 0.52}  # сила виньетирования по каналам (R, G_even, G_odd, B)
SELFTEST_NOISE_PROFILE = (2.0e-4, 1.0e-6)  # (S, O) профиль шума, нормированные единицы


def vignette(channel: int, active_x: np.ndarray, active_y: np.ndarray) -> np.ndarray:
    """Известное виньетирование: 1 - k*r^2, r — радиус от центра активного массива (0 в центре, 1 в углу)."""
    width, height = SELFTEST_ACTIVE
    dx = (active_x / (width - 1) - 0.5) * 2
    dy = (active_y / (height - 1) - 0.5) * 2
    return 1 - SELFTEST_VIGNETTE[channel] * (dx * dx + dy * dy) / 2


def synthetic_map(perturbation: float = 0.0) -> np.ndarray:
    """Карта = 1/виньетирование в узлах (узел (i, j) в пикселе (W-1)/(N-1)*i, (H-1)/(M-1)*j активного массива)."""
    columns, rows = SELFTEST_MAP
    width, height = SELFTEST_ACTIVE
    node_x = (width - 1) / (columns - 1) * np.arange(columns)
    node_y = (height - 1) / (rows - 1) * np.arange(rows)
    grid_x, grid_y = np.meshgrid(node_x, node_y)
    gains = np.stack([1 / vignette(channel, grid_x, grid_y) for channel in range(4)], axis=-1)
    return gains * (1 + perturbation)


def synthetic_frame_blocks(valid_rows: int, row_offset: int, rng: np.random.Generator) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Мозаика кадра (valid_rows x W) по формуле «буфер = кроп активного массива со смещением строк row_offset».

    Возвращает (мозаика-среднее, кадр0, кадр1) в DN; каналы — RGGB, G_even = плоскость 1, G_odd = плоскость 2.
    """
    width = SELFTEST_ACTIVE[0]
    yy, xx = np.mgrid[0:valid_rows, 0:width].astype(np.float64)
    mosaic_channel = np.array([[0, 1], [2, 3]])[(yy.astype(int) % 2), (xx.astype(int) % 2)]
    clean = np.empty((valid_rows, width))
    for channel in range(4):
        selected = mosaic_channel == channel
        signal = SELFTEST_LEVEL * vignette(channel, xx[selected], yy[selected] + row_offset)
        clean[selected] = SELFTEST_BLACK + signal * (SELFTEST_WHITE - SELFTEST_BLACK)
    slope, offset = SELFTEST_NOISE_PROFILE
    sigma = (SELFTEST_WHITE - SELFTEST_BLACK) * np.sqrt(slope * (clean - SELFTEST_BLACK) / (SELFTEST_WHITE - SELFTEST_BLACK) + offset)
    frames = [np.clip(np.round(clean + sigma * rng.standard_normal(clean.shape)), 0, 1023) for _ in range(2)]
    return clean, frames[0], frames[1]


def block_means_of(mosaic: np.ndarray) -> np.ndarray:
    """Средние по блокам 32x32 для 4 плоскостей (4, by, bx), как у зонда."""
    rows, cols = mosaic.shape
    grid_y, grid_x = rows // BLOCK, cols // BLOCK
    planes = []
    for plane in range(4):
        row, col = divmod(plane, 2)
        sub = mosaic[row::2, col::2][: grid_y * SAMPLES_PER_BLOCK_SIDE, : grid_x * SAMPLES_PER_BLOCK_SIDE]
        planes.append(sub.reshape(grid_y, SAMPLES_PER_BLOCK_SIDE, grid_x, SAMPLES_PER_BLOCK_SIDE).mean(axis=(1, 3)))
    return np.stack(planes).astype(np.float32)


def write_synthetic_mode(directory: Path, mode: str, valid_rows: int, row_offset: int, rng: np.random.Generator) -> None:
    """Пишет в `directory` файлы и отчёт, неотличимые по формату от прогона на телефоне."""
    width, height = SELFTEST_ACTIVE
    clean, frame0, frame1 = synthetic_frame_blocks(valid_rows, row_offset, rng)
    base_means = block_means_of(clean)
    grid = {"blocks_x": base_means.shape[2], "blocks_y": base_means.shape[1], "block_size": BLOCK}
    columns, rows = SELFTEST_MAP

    def map_record(perturbation: float | None) -> dict:
        if perturbation is None:
            return {"lens_shading_map": None, "noise_profile": [list(SELFTEST_NOISE_PROFILE)] * 4}
        gains = synthetic_map(perturbation)
        return {
            "lens_shading_map": {"columns": columns, "rows": rows, "gains": gains.reshape(-1).tolist()},
            "noise_profile": [list(SELFTEST_NOISE_PROFILE)] * 4,
        }

    def phase_entry(name: str, perturbation: float | None, means: np.ndarray, extra_files: list[str]) -> dict:
        blocks_name = f"{mode}_{name}.blocks.f32"
        means.astype("<f4").tofile(directory / blocks_name)
        return {
            "phase": name, "requested_frames": 3, "collected_frames": 3, "grid": grid, "valid_rows": valid_rows,
            "files": [blocks_name] + extra_files, "frames": [map_record(perturbation) for _ in range(3)],
        }

    names = [f"{mode}_base.frame0.raw16", f"{mode}_base.frame1.raw16"]
    frame0.astype("<u2").tofile(directory / names[0])
    frame1.astype("<u2").tofile(directory / names[1])
    phases = [
        phase_entry("base", 0.0, base_means, names),
        phase_entry("map_off", None, base_means, []),
        phase_entry("ae_on", 0.01, base_means, []),
        {"phase": "focus_inf", "skipped_reason": "тест: фиксированный фокус"},
    ]
    report = {
        "schema": REPORT_SCHEMA, "mode": mode, "run": "selftest",
        "characteristics": {
            "cfa": 0, "black_level_pattern": [SELFTEST_BLACK] * 4, "white_level": SELFTEST_WHITE,
            "active_array": f"0 0 {width} {height}",
        },
        "buffer_geometry": {"width": width, "height": height, "row_stride_bytes": width * 5 // 4},
        "base_exposure": {"exposure_ns": 1, "iso": 100}, "phases": phases,
    }
    (directory / f"{mode}.json").write_text(json.dumps(report))


def selftest() -> None:
    rng = np.random.default_rng(14)
    with tempfile.TemporaryDirectory() as temp:
        directory = Path(temp)
        height = SELFTEST_ACTIVE[1]
        # main43-подобный: буфер = весь активный массив. main169-подобный: 360 валидных строк из 480, кроп по центру.
        crop_rows = 360
        write_synthetic_mode(directory, "full", valid_rows=height, row_offset=0, rng=rng)
        write_synthetic_mode(directory, "crop", valid_rows=crop_rows, row_offset=(height - crop_rows) // 2, rng=rng)
        summaries = analyze_directory(directory)
        print(markdown(summaries["full"]))
        print(markdown(summaries["crop"]))

        def worst(summary: dict, hypothesis: str) -> float:
            after = summary["phases"]["base"]["after_map"][hypothesis]
            return max(after[c]["max_abs_deviation"] for c in CHANNELS)

        # Допуск: билинейная интерполяция 17x13 узлов квадратичного профиля + дискретизация блоков.
        tolerance = 0.015
        full, crop = summaries["full"], summaries["crop"]
        assert worst(full, "A") < tolerance, f"full/A должен выровнять кадр: {worst(full, 'A')}"
        assert worst(crop, "B") < tolerance, f"crop/B должен выровнять кадр: {worst(crop, 'B')}"
        # Неверная гипотеза обязана быть заметно хуже: иначе формула геометрии ничего не проверяет.
        assert worst(crop, "A") > 4 * tolerance, f"crop/A должен остаться неровным: {worst(crop, 'A')}"
        # Сырой кадр виден как виньетированный: угол заметно темнее центра.
        assert full["phases"]["base"]["raw"]["corner_centre"]["G_even"] < 0.8
        # Каналы: G_even и G_odd различимы по силе виньетирования (проверка чётности строк).
        raw = full["phases"]["base"]["raw"]["corner_centre"]
        assert raw["G_odd"] < raw["G_even"] < raw["R"], raw
        # map_off: пиксели те же, карта отсутствует.
        assert full["base_vs_map_off_max_rel_block_diff"] == 0.0
        assert full["phases"]["map_off"]["map"] is None
        # Вариации карты: ae_on отличается от base ровно на заданный 1%.
        diff = full["phases"]["ae_on"]["map_variation"]["vs_base_max_rel_diff"]
        assert all(abs(value - 0.01) < 1e-6 for value in diff.values()), diff
        assert full["skipped_phases"] == {"focus_inf": "тест: фиксированный фокус"}
        # Шум: предсказание (профиль шума x усиление карты) совпадает с измерением по разности кадров.
        noise = full["noise_base"]
        for channel in CHANNELS:
            measured = noise["raw"]["measured_corner_centre"][channel]
            predicted = noise["raw"]["predicted_corner_centre"][channel]
            assert abs(measured / predicted - 1) < 0.12, (channel, measured, predicted)
            corrected = noise["corrected"]["A"]
            assert abs(corrected["measured_corner_centre"][channel] / corrected["predicted_corner_centre"][channel] - 1) < 0.12
    print("\nselftest: OK")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("directories", nargs="*", type=Path, help="каталоги прогонов (captures/<run>)")
    parser.add_argument("--selftest", action="store_true", help="проверка на синтетических данных")
    args = parser.parse_args()
    if args.selftest:
        selftest()
        return 0
    if not args.directories:
        parser.error("нужен каталог прогона или --selftest")
    for directory in args.directories:
        for summary in analyze_directory(directory).values():
            print(markdown(summary))
            print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
