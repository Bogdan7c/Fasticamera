"""Фикстуры паритета демозаика Menon 2007: сцена + результат colour-demosaicing для 4 раскладок Bayer.

Эталон — colour-demosaicing 0.2.7 (BSD-3-Clause), `demosaicing_CFA_Bayer_Menon2007(CFA, pattern,
refining_step=True)`. Rust-реализация (`crates/isp-model/src/demosaic/menon2007.rs`) сверяется с ним
в `crates/isp-model/tests/demosaic_parity.rs`.

Запуск (из корня репозитория):
    ~/.cache/fasticamera/isp-reference-venv/bin/python tools/isp-reference/make_demosaic_fixtures.py

Выход: test-assets/isp-model/demosaic-scene.{json,bin} (RGB-сцена f32) и
demosaic-menon-<pattern>.{json,bin} (результат демозаика сцены, прошедшей через CFA, f32 RGB).
"""

from __future__ import annotations

import sys
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from colour_demosaicing import demosaicing_CFA_Bayer_Menon2007

# Модуль записи лежит рядом; запуск с -I не добавляет каталог скрипта в sys.path.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from asset_io import reference_versions, write_asset  # noqa: E402

EXIT_OK = 0
EXIT_BAD_OUTPUT_DIR = 2

GENERATOR_COMMAND = "~/.cache/fasticamera/isp-reference-venv/bin/python tools/isp-reference/make_demosaic_fixtures.py"
REFERENCE_NAME = "colour-demosaicing 0.2.7, demosaicing_CFA_Bayer_Menon2007(refining_step=True)"

# Раскладки совпадают с BayerPattern::{RGGB, GRBG, GBRG, BGGR} в Rust (порядок ячеек по строкам 2x2).
PATTERNS = ("RGGB", "GRBG", "GBRG", "BGGR")
CHANNEL_INDEX = {"R": 0, "G": 1, "B": 2}


@dataclass(frozen=True)
class SceneSettings:
    """Параметры детерминированной сцены 40x40 (чётный размер: квадрат 2x2 целиком)."""

    size: int = 40
    seed: int = 20260606
    noise_amplitude: float = 0.01  # шум разрушает случайные ничьи классификатора H/V
    sine_amplitude: float = 0.25  # гладкий фон: у каждого канала своя фаза и частота
    background_level: float = 0.5


def build_scene(settings: SceneSettings) -> np.ndarray:
    """Гладкий цветной фон + резкие цветные прямоугольники + полоса мелкой решётки + шум; f32, 0..1."""
    rng = np.random.default_rng(settings.seed)
    n = settings.size
    ys, xs = np.mgrid[0:n, 0:n].astype(np.float64)
    scene = np.empty((n, n, 3))
    for channel, (fx, fy, phase) in enumerate(((0.11, 0.07, 0.0), (0.05, 0.13, 1.3), (0.09, 0.04, 2.6))):
        scene[:, :, channel] = settings.background_level + settings.sine_amplitude * np.sin(
            2 * np.pi * (fx * xs + fy * ys) + phase
        )
    # Резкие насыщенные края: красный квадрат и синяя полоса.
    scene[6:18, 8:20] = (0.9, 0.1, 0.1)
    scene[24:30, 4:36] = (0.1, 0.2, 0.9)
    # Мелкая серая решётка с периодом 3 пикселя — провоцирует выбор направления.
    scene[32:38, 4:36] = 0.5 + 0.35 * np.sign(np.sin(2 * np.pi * xs[32:38, 4:36] / 3.0 + 0.4))[..., None]
    scene += rng.normal(0.0, settings.noise_amplitude, scene.shape)
    return np.clip(scene, 0.0, 1.0).astype(np.float32)


def mosaic_of(scene: np.ndarray, pattern: str) -> np.ndarray:
    """CFA: в пикселе остаётся канал фильтра (буквы pattern по строкам квадрата 2x2)."""
    cfa = np.empty(scene.shape[:2], dtype=np.float32)
    for cell, letter in enumerate(pattern):
        row, column = divmod(cell, 2)
        cfa[row::2, column::2] = scene[row::2, column::2, CHANNEL_INDEX[letter]]
    return cfa


def reference_demosaic(scene: np.ndarray, pattern: str) -> np.ndarray:
    """Результат эталона (f64) → f32 для записи; вход эталона — те же f32-отсчёты, что получит Rust."""
    return demosaicing_CFA_Bayer_Menon2007(mosaic_of(scene, pattern).astype(np.float64), pattern, refining_step=True).astype(
        np.float32
    )


def write_all(out_dir: Path, settings: SceneSettings) -> None:
    scene = build_scene(settings)
    write_asset(
        out_dir,
        "demosaic-scene",
        scene,
        "Сцена 40x40 RGB (гладкий фон, цветные края, решётка, шум) — вход паритета демозаика; мозаику строит тест.",
        GENERATOR_COMMAND,
        {"seed": settings.seed, "noise_amplitude": settings.noise_amplitude},
    )
    for pattern in PATTERNS:
        write_asset(
            out_dir,
            f"demosaic-menon-{pattern.lower()}",
            reference_demosaic(scene, pattern),
            f"Эталонный демозаик Menon 2007 (с уточнением) сцены demosaic-scene, раскладка {pattern}.",
            GENERATOR_COMMAND,
            {
                "pattern": pattern,
                "reference": REFERENCE_NAME,
                "scene": "demosaic-scene",
                "reference_versions": reference_versions(["colour-demosaicing", "numpy"]),
            },
        )


def main() -> int:
    out_dir = Path(__file__).resolve().parents[2] / "test-assets" / "isp-model"
    if not out_dir.parent.is_dir():
        print(f"нет каталога {out_dir.parent}", file=sys.stderr)
        return EXIT_BAD_OUTPUT_DIR
    write_all(out_dir, SceneSettings())
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main())
