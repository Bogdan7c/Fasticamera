#!/usr/bin/env python3
"""Анализ остаточного геометрического рассогласования основной камеры и ширика (сессия 13b, `dual mode=product_alignment`).

Что делает:
  1. Читает пары зелёных плоскостей (`pairN_main.f32/.json`, `pairN_wide.f32/.json`) из каталога прогона.
  2. Переносит кадр ширика в систему координат полного кадра основной по МОДЕЛИ ПРИЛОЖЕНИЯ:
     вырезка ширика размером k (доля его массива) с центром на его оптической оси ложится на весь кадр основной,
     центр кадра основной = ось ширика (так видит видоискатель при зуме 1x).
  3. Оценивает остаточное подобие «ширик-в-модели -> основная»: сдвиг (dx, dy), масштаб s, поворот theta.
     Грубо — фазовая корреляция на сетке (s, theta), точно — максимизация NCC по полосовым изображениям.
  4. Повторяет по поддиапазонам кадра (левая/центр/правая и верх/центр/низ колонки), чтобы отличить чистый сдвиг
     (параллакс/смещение оси) от зависящего от позиции (масштаб/поворот/дисторсия).
  5. Считает ожидаемый параллакс по LENS_POSE_TRANSLATION и дистанциям фокуса и поправку кропа ширика.

Запуск (нужен numpy/scipy из venv): ~/.cache/fasticamera/isp-reference-venv/bin/python -I pc/align_analyze.py <каталог прогона>...
Самопроверка на синтетике:                          ... -I pc/align_analyze.py --selftest

Определение остатка: контент, который в кадре ширика-в-модели лежит в точке q, в основной оказывается в точке
p = C + s * R(theta) * (q - C) + d, C — центр кадра основной. d = (dx, dy) — сдвиг контента ширика в основной
(в пикселях полного разрешения основной), s > 1 — в основной контент крупнее, чем в модели.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from scipy import fft, ndimage, optimize

# --- Геометрия ---------------------------------------------------------------------------


@dataclass
class Plane:
    """Зелёная плоскость и её геометрия: пиксель (i, j) покрывает [origin + step*i, origin + step*(i+1)) активных пикселей."""

    pixels: np.ndarray
    origin: tuple[float, float]
    step: tuple[float, float]
    meta: dict


def load_plane(directory: Path, name: str) -> Plane:
    meta = json.loads((directory / f"{name}.json").read_text())
    pixels = np.fromfile(directory / meta["file"], dtype="<f4").reshape(meta["height"], meta["width"]).astype(np.float64)
    return Plane(pixels, tuple(meta["crop_origin_active_px"]), tuple(meta["output_px_step_active_px"]), meta)


def model_resample(wide: Plane, main: Plane, k: float, main_center: tuple[float, float], wide_center: tuple[float, float]) -> np.ndarray:
    """Кадр ширика на сетке основной по модели приложения; NaN там, где ширик не покрывает."""
    height, width = main.pixels.shape
    # Центры пикселей основной в её активных пикселях (непрерывные координаты, центр пикселя j = (j + 0.5) * step).
    xs = (np.arange(width) + 0.5) * main.step[0] + main.origin[0]
    ys = (np.arange(height) + 0.5) * main.step[1] + main.origin[1]
    # Масштаб: пикселей ширика на пиксель основной = k * W_wide / W_main (доли массивов совпадают по обеим осям, 4:3).
    wide_array = wide.meta["raw_buffer_size"]
    main_array = main.meta["raw_buffer_size"]
    ratio_x = k * wide_array[0] / main_array[0]
    ratio_y = k * wide_array[1] / main_array[1]
    wide_x = wide_center[0] + (xs - main_center[0]) * ratio_x
    wide_y = wide_center[1] + (ys - main_center[1]) * ratio_y
    index_x = (wide_x - wide.origin[0]) / wide.step[0] - 0.5
    index_y = (wide_y - wide.origin[1]) / wide.step[1] - 0.5
    grid_y, grid_x = np.meshgrid(index_y, index_x, indexing="ij")
    return sample(wide.pixels, grid_x, grid_y)


def sample(image: np.ndarray, index_x: np.ndarray, index_y: np.ndarray) -> np.ndarray:
    """Билинейная выборка; вне изображения — NaN (а не край), чтобы маска была честной."""
    out = ndimage.map_coordinates(image, [index_y, index_x], order=1, mode="constant", cval=np.nan)
    outside = (index_x < 0) | (index_y < 0) | (index_x > image.shape[1] - 1) | (index_y > image.shape[0] - 1)
    out[outside] = np.nan
    return out


# --- Предобработка и метрика -------------------------------------------------------------


def bandpass(image: np.ndarray, valid: np.ndarray, sigma_low: float = 1.5, sigma_high: float = 10.0) -> np.ndarray:
    """DoG-полоса: убирает шум (мелко) и неравномерность освещения/виньетирование (крупно). Невалидное = 0."""
    filled = np.where(valid, image, 0.0)
    weight = valid.astype(np.float64)
    def smooth(sigma: float) -> np.ndarray:
        return ndimage.gaussian_filter(filled, sigma) / np.maximum(ndimage.gaussian_filter(weight, sigma), 1e-6)
    return np.where(valid, smooth(sigma_low) - smooth(sigma_high), 0.0)


def warp_to_main(wide_bp: np.ndarray, wide_valid: np.ndarray, params: np.ndarray, center: tuple[float, float]) -> tuple[np.ndarray, np.ndarray]:
    """Ширик-в-модели -> на сетку основной при остатке params = (dx, dy, s, theta_rad); координаты в пикселях сетки."""
    dx, dy, s, theta = params
    height, width = wide_bp.shape
    grid_y, grid_x = np.meshgrid(np.arange(height) + 0.5, np.arange(width) + 0.5, indexing="ij")
    # p = C + s R (q - C) + d  =>  q = C + R^T (p - C - d) / s
    rel_x, rel_y = grid_x - center[0] - dx, grid_y - center[1] - dy
    cos_t, sin_t = math.cos(theta), math.sin(theta)
    query_x = center[0] + (cos_t * rel_x + sin_t * rel_y) / s - 0.5
    query_y = center[1] + (-sin_t * rel_x + cos_t * rel_y) / s - 0.5
    warped = ndimage.map_coordinates(wide_bp, [query_y, query_x], order=1, mode="constant", cval=0.0)
    valid = ndimage.map_coordinates(wide_valid.astype(np.float64), [query_y, query_x], order=1, mode="constant", cval=0.0) > 0.999
    return warped, valid


def ncc(a: np.ndarray, b: np.ndarray, mask: np.ndarray) -> float:
    """Нормированная кросс-корреляция по маске (-1..1); при пустой маске — -1."""
    if mask.sum() < 500:
        return -1.0
    x, y = a[mask], b[mask]
    x = x - x.mean()
    y = y - y.mean()
    denom = math.sqrt(float((x * x).sum() * (y * y).sum()))
    return float((x * y).sum() / denom) if denom > 0 else -1.0


# --- Оценка -----------------------------------------------------------------------------


@dataclass
class Estimate:
    dx: float          # пиксели сетки основной (умножить на step для полного разрешения)
    dy: float
    scale: float
    theta_rad: float
    ncc: float
    peak_ratio: float  # пик фазовой корреляции / второй по величине пик (грубая стадия), >1.5 — уверенно
    coarse_peak: float


def phase_correlate(a: np.ndarray, b: np.ndarray) -> tuple[float, float, float, float]:
    """Сдвиг (x, y), на который нужно сдвинуть b, чтобы совпало с a; пик и отношение к второму пику."""
    window = np.outer(np.hanning(a.shape[0]), np.hanning(a.shape[1]))
    fa, fb = fft.rfft2(a * window), fft.rfft2(b * window)
    cross = fa * np.conj(fb)
    cross /= np.maximum(np.abs(cross), 1e-9)
    surface = fft.irfft2(cross, s=a.shape)
    flat = surface.ravel()
    best = int(np.argmax(flat))
    peak = float(flat[best])
    iy, ix = np.unravel_index(best, surface.shape)
    # Второй пик: вне окрестности 5x5 вокруг первого.
    masked = surface.copy()
    masked[max(0, iy - 2):iy + 3, max(0, ix - 2):ix + 3] = -np.inf
    second = float(masked.max())
    shift_y = iy if iy <= a.shape[0] // 2 else iy - a.shape[0]
    shift_x = ix if ix <= a.shape[1] // 2 else ix - a.shape[1]
    return float(shift_x), float(shift_y), peak, peak / max(second, 1e-9)


def estimate_similarity(
    main_bp: np.ndarray, wide_bp: np.ndarray, wide_valid: np.ndarray, center: tuple[float, float],
    scales: np.ndarray | None = None, thetas_deg: np.ndarray | None = None, region: np.ndarray | None = None,
    translation_only: bool = False, start: np.ndarray | None = None,
) -> Estimate:
    """Грубо по сетке (s, theta) с фазовой корреляцией, затем уточнение NCC по (dx, dy, s, theta)."""
    mask_region = np.ones(main_bp.shape, bool) if region is None else region
    if start is None:
        scales = np.arange(0.94, 1.0601, 0.004) if scales is None else scales
        thetas_deg = np.arange(-1.5, 1.5001, 0.25) if thetas_deg is None else thetas_deg
        if translation_only:
            scales, thetas_deg = np.array([1.0]), np.array([0.0])
        best = (-1.0, None, 0.0, 0.0)
        for s in scales:
            for theta_deg in thetas_deg:
                warped, valid = warp_to_main(wide_bp, wide_valid, np.array([0.0, 0.0, s, math.radians(theta_deg)]), center)
                shift_x, shift_y, peak, ratio = phase_correlate(main_bp * mask_region, warped * valid * mask_region)
                if peak > best[0]:
                    best = (peak, np.array([shift_x, shift_y, s, math.radians(theta_deg)]), peak, ratio)
        _, start, coarse_peak, peak_ratio = best
        # Фазовая корреляция даёт сдвиг warped(d=0) -> main; остаток p = ... + d совпадает по знаку со сдвигом контента.
        start = np.array(start, float)
    else:
        coarse_peak, peak_ratio = float("nan"), float("nan")

    def cost(x: np.ndarray) -> float:
        params = np.array([x[0], x[1], 1.0, 0.0]) if translation_only else np.array([x[0], x[1], x[2], x[3]])
        warped, valid = warp_to_main(wide_bp, wide_valid, params, center)
        return -ncc(main_bp, warped, valid & mask_region)

    x0 = start[:2] if translation_only else start
    steps = [1.0, 1.0] if translation_only else [1.0, 1.0, 0.004, 0.004]
    simplex = np.vstack([x0] + [x0 + np.eye(len(x0))[i] * steps[i] for i in range(len(x0))])
    result = optimize.minimize(cost, x0, method="Nelder-Mead", options={"initial_simplex": simplex, "xatol": 1e-4, "fatol": 1e-7, "maxiter": 600})
    x = result.x
    return Estimate(float(x[0]), float(x[1]), 1.0 if translation_only else float(x[2]), 0.0 if translation_only else float(x[3]),
                    float(-result.fun), float(peak_ratio), float(coarse_peak))


def prepare(main_pixels: np.ndarray, wide_on_canvas: np.ndarray) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Полосовые изображения, одинаково нормированные; маска валидности ширика."""
    valid = np.isfinite(wide_on_canvas)
    main_bp = bandpass(main_pixels, np.ones_like(valid))
    wide_bp = bandpass(np.nan_to_num(wide_on_canvas), valid)
    main_bp /= max(main_bp.std(), 1e-9)
    wide_bp /= max(wide_bp[valid].std(), 1e-9)
    return main_bp, wide_bp, valid


# --- Параллакс и поправка ----------------------------------------------------------------


def region_masks(shape: tuple[int, int]) -> dict[str, np.ndarray]:
    height, width = shape
    masks = {}
    for name, (x0, x1) in {"left": (0, 1 / 3), "center_x": (1 / 3, 2 / 3), "right": (2 / 3, 1)}.items():
        mask = np.zeros(shape, bool)
        mask[:, int(x0 * width):int(x1 * width)] = True
        masks[name] = mask
    for name, (y0, y1) in {"top": (0, 1 / 3), "center_y": (1 / 3, 2 / 3), "bottom": (2 / 3, 1)}.items():
        mask = np.zeros(shape, bool)
        mask[int(y0 * height):int(y1 * height), :] = True
        masks[name] = mask
    core = np.zeros(shape, bool)
    core[int(height * 0.25):int(height * 0.75), int(width * 0.25):int(width * 0.75)] = True
    masks["core50"] = core
    return masks


def block_mean(image: np.ndarray, factor: int) -> np.ndarray:
    """Среднее блоками factor x factor; блок с хоть одним NaN остаётся NaN (граница покрытия не размазывается)."""
    height, width = (image.shape[0] // factor) * factor, (image.shape[1] // factor) * factor
    return image[:height, :width].reshape(height // factor, factor, width // factor, factor).mean(axis=(1, 3))


def analyze_pair(directory: Path, index: int, static: dict, center_mode: str, start: np.ndarray | None, factor: int = 2) -> dict:
    main = load_plane(directory, f"pair{index}_main")
    wide = load_plane(directory, f"pair{index}_wide")
    k = static["zoom_ratio_wide_k"]
    main_array = main.meta["raw_buffer_size"]
    center = (main_array[0] / 2, main_array[1] / 2) if center_mode == "frame" else tuple(main.meta["optical_center_active_px"])
    wide_center = tuple(wide.meta["optical_center_active_px"])
    canvas = model_resample(wide, main, k, center, wide_center)
    # Для скорости работаем на сетке в `factor` раз грубее (субпиксельность даёт NCC-уточнение, см. selftest).
    coarse_pixels, coarse_canvas = block_mean(main.pixels, factor), block_mean(canvas, factor)
    main.step = (main.step[0] * factor, main.step[1] * factor)
    main_bp, wide_bp, valid = prepare(coarse_pixels, coarse_canvas)
    center_grid = (center[0] / main.step[0], center[1] / main.step[1])
    full = estimate_similarity(main_bp, wide_bp, valid, center_grid, start=start)
    translation = estimate_similarity(main_bp, wide_bp, valid, center_grid, translation_only=True,
                                      start=None if start is None else start[:2])
    out = {"index": index, "skew_ms": None, "full": full, "translation_only": translation, "regions": {}, "step": main.step, "center": center,
           "wide_center": wide_center, "main_meta": main.meta["result"], "wide_meta": wide.meta["result"]}
    for name, mask in region_masks(coarse_pixels.shape).items():
        region_translation = estimate_similarity(main_bp, wide_bp, valid, center_grid, region=mask, translation_only=True, start=np.array([full.dx, full.dy]))
        region_after_global = estimate_similarity(main_bp, wide_bp, valid, center_grid, region=mask, start=np.array([full.dx, full.dy, full.scale, full.theta_rad]))
        out["regions"][name] = (region_translation, region_after_global)
    return out


def parallax_prediction(static: dict, diopters: float) -> dict:
    """Ожидаемый параллакс в пикселях основной: dx = fx_main * baseline * диоптрии (знак — по оси позы)."""
    translation = static["lens_wide"]["lens_pose_translation_m"]
    fx, fy = static["main"]["lens_intrinsic_calibration"][:2]
    return {"baseline_m": translation, "dx_px": fx * translation[0] * diopters, "dy_px": fy * translation[1] * diopters}


# --- Самопроверка -----------------------------------------------------------------------


def selftest() -> int:
    """Синтетика: известный сдвиг/масштаб/поворот на текстуре со смесью частот, шум и разная яркость."""
    rng = np.random.default_rng(7)
    height, width = 384, 512
    base = ndimage.gaussian_filter(rng.standard_normal((height + 200, width + 200)), 3.0) * 40 + 100
    base += ndimage.gaussian_filter(rng.standard_normal(base.shape), 8.0) * 80
    center = (width / 2, height / 2)
    # Основная = центральное окно большой текстуры (без краевых артефактов), ширик-в-модели = её деформация.
    big_center = (center[0] + 100, center[1] + 100)
    main_pixels = base[100:100 + height, 100:100 + width]
    failures = 0
    for truth in [(3.3, -2.1, 1.012, math.radians(0.4)), (-7.6, 4.4, 0.985, math.radians(-0.7)), (0.0, 0.0, 1.0, 0.0)]:
        dx, dy, s, theta = truth
        grid_y, grid_x = np.meshgrid(np.arange(height) + 0.5, np.arange(width) + 0.5, indexing="ij")
        # Ширик в точке q видит то, что основная видит в p = C + sR(q-C) + d.
        rel_x, rel_y = grid_x - center[0], grid_y - center[1]
        p_x = center[0] + s * (math.cos(theta) * rel_x - math.sin(theta) * rel_y) + dx
        p_y = center[1] + s * (math.sin(theta) * rel_x + math.cos(theta) * rel_y) + dy
        wide_pixels = ndimage.map_coordinates(base, [p_y - 0.5 + 100, p_x - 0.5 + 100], order=3)
        wide_pixels = wide_pixels * 1.07 + 5 + rng.standard_normal(wide_pixels.shape) * 3.0
        main_noisy = main_pixels + rng.standard_normal(main_pixels.shape) * 3.0
        main_bp, wide_bp, valid = prepare(main_noisy, wide_pixels)
        estimate = estimate_similarity(main_bp, wide_bp, valid, center)
        errors = (estimate.dx - dx, estimate.dy - dy, estimate.scale - s, math.degrees(estimate.theta_rad - theta))
        ok = abs(errors[0]) < 0.1 and abs(errors[1]) < 0.1 and abs(errors[2]) < 0.001 and abs(errors[3]) < 0.05
        failures += 0 if ok else 1
        print(f"truth dx={dx:+.2f} dy={dy:+.2f} s={s:.4f} th={math.degrees(theta):+.2f}  ->  "
              f"err dx={errors[0]:+.4f} dy={errors[1]:+.4f} s={errors[2]:+.5f} th={errors[3]:+.4f} deg ncc={estimate.ncc:.3f} "
              f"{'OK' if ok else 'FAIL'}")
    print("selftest:", "PASS" if failures == 0 else f"FAIL ({failures})")
    return 0 if failures == 0 else 1


# --- Отчёт ------------------------------------------------------------------------------


def run_report(directory: Path, center_mode: str) -> dict:
    record = json.loads((directory / "run_record.json").read_text())
    data = next(r for r in record["records"] if r.get("kind") == "result")["data"]
    static = data["static"]
    pairs = []
    start = None
    for pair in data["pairs"]:
        analysis = analyze_pair(directory, pair["index"], static, center_mode, start)
        # Следующие пары стартуют с найденного решения: сцена и камеры те же, грубый поиск нужен один раз.
        if start is None:
            start = np.array([analysis["full"].dx, analysis["full"].dy, analysis["full"].scale, analysis["full"].theta_rad])
        analysis["skew_ms"] = pair["skew_ms"]
        pairs.append(analysis)
    return {"directory": str(directory), "static": static, "pairs": pairs, "center_mode": center_mode}


def format_report(report: dict) -> list[str]:
    static = report["static"]
    k = static["zoom_ratio_wide_k"]
    main_width = static["main"]["raw_output_width"]
    wide_width = static["wide"]["raw_output_width"]
    lines = [f"== {report['directory']}  (центр модели: {report['center_mode']}, k={k:.5f})"]
    for pair in report["pairs"]:
        step_x, step_y = pair["step"]
        full, trans = pair["full"], pair["translation_only"]
        lines.append(f"pair{pair['index']} skew={pair['skew_ms']:.2f}ms diopt main={pair['main_meta']['focus_distance_diopters']:.3f} "
                     f"wide={pair['wide_meta']['focus_distance_diopters']:.3f}")
        lines.append(f"  similarity:  dx={full.dx * step_x:+.2f}px ({full.dx * step_x / main_width * 100:+.3f}%W) dy={full.dy * step_y:+.2f}px "
                     f"s={full.scale:.5f} (EV {math.log2(full.scale):+.5f}) theta={math.degrees(full.theta_rad):+.3f}deg ncc={full.ncc:.3f} "
                     f"peak_ratio={full.peak_ratio:.2f}")
        lines.append(f"  translation: dx={trans.dx * step_x:+.2f}px ({trans.dx * step_x / main_width * 100:+.3f}%W) dy={trans.dy * step_y:+.2f}px ncc={trans.ncc:.3f}")
        for name, (translation, after_global) in pair["regions"].items():
            lines.append(f"    {name:9s} translation-only dx={translation.dx * step_x:+7.2f} dy={translation.dy * step_y:+7.2f} ncc={translation.ncc:.3f} | "
                         f"after-global dx={after_global.dx * step_x:+7.2f} dy={after_global.dy * step_y:+7.2f} s={after_global.scale:.4f} ncc={after_global.ncc:.3f}")
    lines.extend(summary_lines(report))
    return lines


def summary_lines(report: dict) -> list[str]:
    """Медиана по парам, параллакс по LENS_POSE и дистанциям фокуса, поправка кропа ширика для ЭТОЙ сцены."""
    static, pairs = report["static"], report["pairs"]
    step_x, step_y = pairs[0]["step"]
    med = lambda values: float(np.median(values))  # noqa: E731
    dx = med([p["full"].dx * step_x for p in pairs])
    dy = med([p["full"].dy * step_y for p in pairs])
    scale = med([p["full"].scale for p in pairs])
    theta = med([math.degrees(p["full"].theta_rad) for p in pairs])
    core = med([p["regions"]["core50"][1].scale for p in pairs])
    core_dx = med([p["regions"]["core50"][1].dx * step_x for p in pairs])
    core_dy = med([p["regions"]["core50"][1].dy * step_y for p in pairs])
    main_width = static["main"]["raw_output_width"]
    lines = ["-- медиана по парам (пиксели полного разрешения основной) --",
             f"  full similarity: dx={dx:+.1f} ({dx / main_width * 100:+.2f}%W) dy={dy:+.1f} s={scale:.5f} (log2 {math.log2(scale):+.5f}) theta={theta:+.3f}deg",
             f"  core50 similarity: dx={core_dx:+.1f} dy={core_dy:+.1f} s={core:.5f}"]
    diopters = {"main": med([p["main_meta"]["focus_distance_diopters"] for p in pairs]),
                "wide": med([p["wide_meta"]["focus_distance_diopters"] for p in pairs])}
    for who, d in diopters.items():
        par = parallax_prediction(static, d)
        lines.append(f"  focus {who}: {d:.3f} D = {1 / d:.2f} m -> parallax dx={par['dx_px']:+.1f} dy={par['dy_px']:+.1f} px (baseline {par['baseline_m']})")
    # Поправка кропа: c' = c_w - rho * d / s, k' = k / s (rho — пикселей ширика на пиксель основной в модели).
    k = static["zoom_ratio_wide_k"]
    wide_w, wide_h = static["wide"]["raw_output_width"], static["wide"]["active_array"][3]
    main_w, main_h = static["main"]["raw_output_width"], static["main"]["active_array"][3]
    rho_x, rho_y = k * wide_w / main_w, k * wide_h / main_h
    for label, (sx_dx, sx_dy, sx_s) in {"full": (dx, dy, scale), "core50": (core_dx, core_dy, core)}.items():
        off_x, off_y = -rho_x * sx_dx / sx_s, -rho_y * sx_dy / sx_s
        crop_w, crop_h = k * wide_w, k * wide_h
        wide_axis = pairs[0]["wide_center"]
        lines.append(f"  correction[{label}]: crop centre offset = ({off_x:+.1f}, {off_y:+.1f}) wide px = ({off_x / crop_w:+.4f}, {off_y / crop_h:+.4f}) crop units; "
                     f"new centre = ({wide_axis[0] + off_x:.1f}, {wide_axis[1] + off_y:.1f}) (array centre {wide_w / 2:.0f}, {wide_h / 2:.0f}); crop size factor k'/k = {1 / sx_s:.5f} (k'={k / sx_s:.5f})")
    return lines


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("directories", nargs="*", type=Path)
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--center", choices=["frame", "axis"], default="frame", help="центр основной в модели: центр кадра (по умолчанию) или её оптическая ось")
    parser.add_argument("--json-out", type=Path)
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    reports = []
    for directory in args.directories:
        report = run_report(directory, args.center)
        reports.append(report)
        print("\n".join(format_report(report)), flush=True)
    if args.json_out:
        args.json_out.write_text(json.dumps(reports, default=lambda o: o.__dict__ if hasattr(o, "__dict__") else str(o), indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
