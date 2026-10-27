"""Реальные кропы RAW для isp-model и их внешние эталоны (сессия 06).

Из DNG набора C7 (00, вне репозитория) вырезаются маленькие кропы мозаики, и для каждого
считаются два независимых эталона:

1. «Независимая цепочка» на чужих реализациях: уровни и баланс по формуле, демозаик
   colour-demosaicing (Menon 2007), матрица камера → XYZ D50 по colour-hdri (DNG) с нормировкой
   ForwardMatrix как в Adobe DNG SDK. Результат — XYZ D50 каждого пикселя: Rust сверяется
   почти точно (f32 против f64).
2. LibRaw (rawpy) на полном кадре: линейный sRGB D65 со своим демозаиком (AHD) и своей
   матрицей цвета. Это другая реализация, совпадение только приблизительное; в манифест
   идёт среднее центральной области кропа, тест считает ΔE2000 и объясняет разницу.

Запуск (venv эталонов, см. mem:suggested_commands):
    ~/.cache/fasticamera/isp-reference-venv/bin/python -I tools/isp-reference/make_real_crops.py \\
        --capture-dir ~/fasticamera-captures/c7-reference/captures/204802-f42af9 --stem 2_lamp \\
        --out-dir test-assets/isp-model

Метаданные кропа — только нужные ISP числа (уровни, CFA, матрицы, нейтраль). Путей,
серийников и времени съёмки в манифестах нет.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from dataclasses import dataclass
from pathlib import Path

import numpy as np

# Модуль записи лежит рядом; запуск с -I не добавляет каталог скрипта в sys.path.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from asset_io import reference_versions, write_asset  # noqa: E402

EXIT_OK = 0
EXIT_BAD_INPUT = 2

# Сторона квадратного кропа. Чётная (целые квадраты 2×2), маленькая — фикстуры в git.
CROP_SIDE = 48
# Отступ от края кропа для средних по LibRaw: демозаик у края кропа и у кадра различается.
REFERENCE_MARGIN = 8

# Температуры калибровочных иллюминантов по Adobe DNG SDK (dng_camera_profile, IlluminantToTemperature).
ILLUMINANT_TEMPERATURE_KELVIN = {17: 2850.0, 21: 6500.0}

# Кропы кадра «лампа» основной камеры: левый верхний угол (чётный — фаза CFA сохраняется).
LAMP_CROPS = {
    "gray-tile": (1200, 2600),
    "towel": (800, 520),
    "jar": (2600, 1220),
    "tap-highlight": (2240, 1800),
}

GENERATOR = (
    "tools/isp-reference/make_real_crops.py --capture-dir <C7>/204802-f42af9 --stem 2_lamp "
    "--out-dir test-assets/isp-model"
)


@dataclass(frozen=True)
class CaptureMetadata:
    """Числа паспорта, которые нужны ISP (кадр DNG — still_frame)."""

    black: list[float]
    white: float
    cfa_code: int
    neutral: list[float]
    illuminants: list[int]
    color_matrices: list[np.ndarray]
    forward_matrices: list[np.ndarray]
    calibrations: list[np.ndarray]


def read_metadata(passport: dict) -> CaptureMetadata:
    sensor = passport["still_frame"]["sensor"]
    frame = passport["still_frame"]["frame"]
    color = passport["stream_frame"]["color"]
    as_matrix = lambda key: np.array(color[key], dtype=np.float64).reshape(3, 3)  # noqa: E731
    return CaptureMetadata(
        black=[float(level) for level in frame["dynamic_black_level"]],
        white=float(frame["dynamic_white_level"]),
        cfa_code=int(sensor["cfa"]),
        neutral=[float(value) for value in frame["neutral_color_point"]],
        illuminants=[int(color["reference_illuminant1"]), int(color["reference_illuminant2"])],
        color_matrices=[as_matrix("color_matrix1"), as_matrix("color_matrix2")],
        forward_matrices=[as_matrix("forward_matrix1"), as_matrix("forward_matrix2")],
        calibrations=[as_matrix("calibration1"), as_matrix("calibration2")],
    )


def normalize_forward_matrix(forward: np.ndarray, d50_xyz: np.ndarray) -> np.ndarray:
    """Как dng_camera_profile::NormalizeForwardMatrix: FM·(1,1,1) = XYZ(D50)."""
    return np.diag(d50_xyz) @ np.linalg.inv(np.diag(forward @ np.ones(3))) @ forward


def camera_to_xyz_d50(metadata: CaptureMetadata) -> tuple[np.ndarray, np.ndarray]:
    """Матрица камера (до баланса) → XYZ D50 по colour-hdri и найденная белая xy."""
    import colour
    from colour_hdri import camera_neutral_to_xy, matrix_camera_space_to_XYZ

    d50_xyz = colour.xy_to_XYZ(np.array([0.3457, 0.3585]))
    # colour-hdri требует калибровки по возрастанию температуры: A (2850 K) первой.
    order = sorted(range(2), key=lambda index: ILLUMINANT_TEMPERATURE_KELVIN[metadata.illuminants[index]])
    temperatures = [ILLUMINANT_TEMPERATURE_KELVIN[metadata.illuminants[index]] for index in order]
    color_matrices = [metadata.color_matrices[index] for index in order]
    calibrations = [metadata.calibrations[index] for index in order]
    forwards = [normalize_forward_matrix(metadata.forward_matrices[index], d50_xyz) for index in order]
    neutral = np.array(metadata.neutral)
    analog_balance = np.ones(3)
    white_xy = camera_neutral_to_xy(neutral, *temperatures, *color_matrices, *calibrations, analog_balance)
    matrix = matrix_camera_space_to_XYZ(
        white_xy, *temperatures, *color_matrices, *calibrations, analog_balance, *forwards
    )
    return matrix, white_xy


def independent_xyz(raw_crop: np.ndarray, metadata: CaptureMetadata, matrix: np.ndarray) -> np.ndarray:
    """Уровни → баланс с обрезкой → Menon 2007 → XYZ D50, на чужих реализациях."""
    from colour_demosaicing import demosaicing_CFA_Bayer_Menon2007

    black = np.array(metadata.black).reshape(2, 2)
    black_plane = np.tile(black, (raw_crop.shape[0] // 2, raw_crop.shape[1] // 2))
    normalized = np.maximum(raw_crop.astype(np.float64) - black_plane, 0.0) / (metadata.white - black_plane)
    neutral = np.array(metadata.neutral)
    gains = neutral.max() / neutral
    pattern = cfa_pattern_name(metadata.cfa_code)
    channel_of_cell = {"R": 0, "G": 1, "B": 2}
    gain_plane = np.tile(
        np.array([gains[channel_of_cell[letter]] for letter in pattern]).reshape(2, 2),
        (raw_crop.shape[0] // 2, raw_crop.shape[1] // 2),
    )
    balanced = np.minimum(normalized * gain_plane, 1.0)
    camera_rgb = demosaicing_CFA_Bayer_Menon2007(balanced, pattern)
    balanced_to_xyz = matrix @ np.diag(1.0 / gains)
    return np.einsum("ij,hwj->hwi", balanced_to_xyz, camera_rgb)


def cfa_pattern_name(cfa_code: int) -> str:
    """Код Camera2 SENSOR_INFO_COLOR_FILTER_ARRANGEMENT → раскладка 2×2."""
    names = {0: "RGGB", 1: "GRBG", 2: "GBRG", 3: "BGGR"}
    if cfa_code not in names:
        raise ValueError(f"CFA {cfa_code} не Bayer")
    return names[cfa_code]


def libraw_linear_srgb(dng_path: Path) -> tuple[np.ndarray, np.ndarray]:
    """Линейный sRGB D65 (0…1) от LibRaw: баланс камеры, AHD, обрезка пересвета, без автояркости.

    Вторым значением — матрица LibRaw «камера после баланса → линейный sRGB» (rgb_cam). Для DNG
    LibRaw строит её только из ColorMatrix1 (нормировка строк cam_xyz_coeff), без ForwardMatrix
    и без интерполяции по температуре: это главный источник расхождения с DNG-путём.
    """
    import rawpy

    with rawpy.imread(str(dng_path)) as raw:
        rgb_cam = np.array(raw.color_matrix, dtype=np.float64)[:, :3]
        rgb = raw.postprocess(
            use_camera_wb=True,
            no_auto_bright=True,
            gamma=(1, 1),
            output_bps=16,
            output_color=rawpy.ColorSpace.sRGB,
            demosaic_algorithm=rawpy.DemosaicAlgorithm.AHD,
            highlight_mode=rawpy.HighlightMode.Clip,
            user_flip=0,
        )
    return rgb.astype(np.float64) / 65535.0, rgb_cam


def capture_extra(
    metadata: CaptureMetadata, white_xy: np.ndarray, matrix: np.ndarray, capture_digest: str, origin: tuple[int, int]
) -> dict:
    """Метаданные кадра для манифеста: всё, что нужно Rust, чтобы собрать описание сенсора."""
    return {
        "capture": f"C7 «лампа», основная камера, DNG sha256 {capture_digest}…",
        "crop_origin": list(origin),
        "cfa": cfa_pattern_name(metadata.cfa_code).lower(),
        "black_level": metadata.black,
        "white_level": metadata.white,
        "neutral": metadata.neutral,
        "illuminants": metadata.illuminants,
        "color_matrices": [m.flatten().tolist() for m in metadata.color_matrices],
        "forward_matrices": [m.flatten().tolist() for m in metadata.forward_matrices],
        "calibrations": [m.flatten().tolist() for m in metadata.calibrations],
        "reference_white_xy": white_xy.tolist(),
        "reference_camera_to_xyz_d50": matrix.flatten().tolist(),
        "reference_versions": reference_versions(
            ["rawpy", "numpy", "colour-science", "colour-hdri", "colour-demosaicing"]
        ),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--capture-dir", type=Path, required=True)
    parser.add_argument("--stem", required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument(
        "--full-frame-dir",
        type=Path,
        help="дополнительно записать полный кадр (для examples/render_capture.rs) — только вне репозитория",
    )
    args = parser.parse_args()

    dng_path = args.capture_dir / f"{args.stem}.dng"
    passport_path = args.capture_dir / f"{args.stem}.passport.json"
    if not dng_path.is_file() or not passport_path.is_file():
        print(f"нет {dng_path.name} или {passport_path.name} в каталоге набора", file=sys.stderr)
        return EXIT_BAD_INPUT

    import rawpy

    metadata = read_metadata(json.loads(passport_path.read_text(encoding="utf-8")))
    matrix, white_xy = camera_to_xyz_d50(metadata)
    with rawpy.imread(str(dng_path)) as raw:
        mosaic = raw.raw_image_visible.copy()
    libraw, libraw_rgb_cam = libraw_linear_srgb(dng_path)
    capture_digest = hashlib.sha256(dng_path.read_bytes()).hexdigest()[:12]

    if args.full_frame_dir is not None:
        write_asset(
            args.full_frame_dir,
            "full-main-lamp",
            mosaic.astype("<u2"),
            "Полный кадр мозаики C7 «лампа» для визуальной проверки (не для репозитория)",
            GENERATOR + " --full-frame-dir <вне репозитория>",
            capture_extra(metadata, white_xy, matrix, capture_digest, origin=(0, 0)),
        )

    for crop_name, (left, top) in LAMP_CROPS.items():
        raw_crop = mosaic[top : top + CROP_SIDE, left : left + CROP_SIDE]
        xyz = independent_xyz(raw_crop, metadata, matrix)
        inner = slice(REFERENCE_MARGIN, CROP_SIDE - REFERENCE_MARGIN)
        libraw_crop = libraw[top : top + CROP_SIDE, left : left + CROP_SIDE]
        extra = capture_extra(metadata, white_xy, matrix, capture_digest, origin=(left, top))
        extra |= {
            "reference_margin": REFERENCE_MARGIN,
            "libraw_inner_mean_linear_srgb": libraw_crop[inner, inner].reshape(-1, 3).mean(axis=0).tolist(),
            "libraw_rgb_cam": libraw_rgb_cam.flatten().tolist(),
            "libraw_settings": "rawpy postprocess: camera WB, AHD, clip, gamma 1, no auto bright, sRGB, 16 bit",
            "raw_max_in_crop": int(raw_crop.max()),
        }
        write_asset(
            args.out_dir,
            f"real-main-lamp-{crop_name}",
            raw_crop.astype("<u2"),
            f"Кроп {CROP_SIDE}×{CROP_SIDE} мозаики RAW ({crop_name}) кадра C7 «лампа», основная камера",
            GENERATOR,
            extra,
        )
        write_asset(
            args.out_dir,
            f"real-main-lamp-{crop_name}-xyz",
            xyz.astype("<f4"),
            f"XYZ D50 кропа {crop_name}: независимая цепочка colour-demosaicing (Menon 2007) + colour-hdri",
            GENERATOR,
            {"source_crop": f"real-main-lamp-{crop_name}"},
        )
        print(f"{crop_name}: raw max {raw_crop.max()}, xyz mean {xyz.reshape(-1, 3).mean(axis=0)}")
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main())
