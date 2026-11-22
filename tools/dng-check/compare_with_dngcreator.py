#!/usr/bin/env python3
"""Сверка нашего DNG (`photo-output`) с эталоном платформенного `DngCreator` (вне CI).

Запуск (окружение с rawpy и numpy, см. README.md):
    ~/.cache/fasticamera/isp-reference-venv/bin/python -I tools/dng-check/compare_with_dngcreator.py \
        ~/fasticamera-captures/c7-reference/captures/204802-f42af9 [...другие каталоги прогонов]

Что делает для каждого прогона:
 1. Берёт эталонный `*.dng` (DngCreator) и `*.stream.raw16`.
 2. Собирает НАШ DNG двумя способами через пример `dng_from_capture`:
    * «те же пиксели» — метаданные кадра `still_frame` и отсчёты, вынутые из эталонного DNG. Любое расхождение
      картинки здесь — расхождение тегов (матрицы, уровни, белый, CFA), а не сцены;
    * «кадр потока» — отсчёты `*.stream.raw16` (другой кадр той же сцены: эталон DNG снят отдельным
      запросом, поэтому расхождение включает шум и мерцание лампы).
 3. Рендерит эталон и наши DNG через rawpy одинаково: баланс белого — множители эталона (`user_wb` из
    AsShotNeutral эталона), без автояркости, линейный выход 16 бит, sRGB-примары, демозаика AHD.
 4. Печатает таблицу: PSNR (пик 65535), средний и максимальный ΔE76 по плиткам, а также различия тегов IFD0.
"""

import argparse
import glob
import os
import struct
import subprocess
import sys
import tempfile

import numpy as np
import rawpy

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
TILE = 128  # сторона плитки для ΔE, пикселей
# Размеры типов TIFF: код → (байт на значение, формат struct)
TIFF_TYPES = {1: (1, "B"), 2: (1, "c"), 3: (2, "H"), 4: (4, "I"), 5: (8, "II"), 7: (1, "B"), 10: (8, "ii"), 12: (8, "d")}
TAG_NAMES = {
    254: "NewSubFileType", 256: "ImageWidth", 257: "ImageLength", 258: "BitsPerSample", 259: "Compression",
    262: "Photometric", 271: "Make", 272: "Model", 274: "Orientation", 277: "SamplesPerPixel",
    278: "RowsPerStrip", 284: "PlanarConfiguration", 305: "Software", 306: "DateTime",
    33421: "CFARepeatPatternDim", 33422: "CFAPattern", 33434: "ExposureTime", 33437: "FNumber",
    34665: "ExifIFD", 34855: "ISO", 36867: "DateTimeOriginal", 37386: "FocalLength",
    50706: "DNGVersion", 50707: "DNGBackwardVersion", 50708: "UniqueCameraModel", 50710: "CFAPlaneColor",
    50711: "CFALayout", 50713: "BlackLevelRepeatDim", 50714: "BlackLevel", 50717: "WhiteLevel",
    50718: "DefaultScale", 50719: "DefaultCropOrigin", 50720: "DefaultCropSize", 50721: "ColorMatrix1",
    50722: "ColorMatrix2", 50723: "CameraCalibration1", 50724: "CameraCalibration2", 50728: "AsShotNeutral",
    50730: "BaselineExposure", 50778: "CalibrationIlluminant1", 50779: "CalibrationIlluminant2",
    50829: "ActiveArea", 50964: "ForwardMatrix1", 50965: "ForwardMatrix2", 51009: "OpcodeList2",
    51022: "OpcodeList3", 51041: "NoiseProfile",
}
# Теги, которые у DngCreator и у нас различаются по замыслу или не несут смысла для картинки.
EXPECTED_TAG_DIFFERENCES = {
    273, 278, 279,  # полосы: у нас по 64 КиБ, у DngCreator по строке
    305, 306, 36867, 270, 282, 283, 296, 33432,  # программа, время, пустые служебные поля
    50708,  # UniqueCameraModel: имя в примере совпадает, но поле допускает разные строки
}


def read_ifd0_tags(path):
    """Читает IFD0: {тег: список значений}. Только то, что нужно для сверки."""
    data = open(path, "rb").read(1 << 20)  # IFD0 лежит в первом мегабайте у обоих писателей
    assert data[:4] == b"II*\0", "ожидается TIFF little-endian"
    offset = struct.unpack_from("<I", data, 4)[0]
    count = struct.unpack_from("<H", data, offset)[0]
    tags = {}
    for index in range(count):
        tag, type_code, n = struct.unpack_from("<HHI", data, offset + 2 + index * 12)
        size, fmt = TIFF_TYPES.get(type_code, (1, "B"))
        total = size * n
        value_at = offset + 2 + index * 12 + 8
        if total > 4:
            value_at = struct.unpack_from("<I", data, value_at)[0]
        if value_at + total > len(data):
            tags[tag] = "<вне первого мегабайта>"
            continue
        raw = data[value_at:value_at + total]
        if type_code == 2:
            tags[tag] = raw.rstrip(b"\0").decode("ascii", "replace")
        elif type_code in (5, 10):
            pair = "II" if type_code == 5 else "ii"
            flat = struct.unpack("<" + pair[0] * (2 * n), raw)
            tags[tag] = [round(flat[i] / flat[i + 1], 6) if flat[i + 1] else float("nan") for i in range(0, 2 * n, 2)]
        elif type_code == 12:
            tags[tag] = [float(x) for x in struct.unpack("<" + "d" * n, raw)]
        elif type_code == 7 and n > 64:
            tags[tag] = f"<{n} байт>"
        else:
            tags[tag] = list(struct.unpack("<" + fmt * n, raw))
    return tags


def close(a, b, tolerance=2e-6):
    if isinstance(a, list) and isinstance(b, list) and len(a) == len(b):
        return all(isinstance(x, (int, float)) and isinstance(y, (int, float)) and abs(x - y) <= tolerance for x, y in zip(a, b))
    return a == b


def print_tag_differences(reference_path, our_path):
    reference, ours = read_ifd0_tags(reference_path), read_ifd0_tags(our_path)
    print("  Различия тегов IFD0 (эталон DngCreator → наш):")
    rows = 0
    for tag in sorted(set(reference) | set(ours)):
        if tag in EXPECTED_TAG_DIFFERENCES:
            continue
        if tag in reference and tag in ours and close(reference[tag], ours[tag]):
            continue
        name = TAG_NAMES.get(tag, str(tag))
        left = reference.get(tag, "— нет —")
        right = ours.get(tag, "— нет —")
        left = (str(left)[:60] + "…") if len(str(left)) > 60 else left
        right = (str(right)[:60] + "…") if len(str(right)) > 60 else right
        print(f"    {name:<22} {left}  →  {right}")
        rows += 1
    if rows == 0:
        print("    (нет)")


def render(path, user_wb):
    """Линейный 16-битный рендер: одинаковые настройки для всех DNG."""
    with rawpy.imread(path) as raw:
        return raw.postprocess(
            user_wb=list(user_wb),
            no_auto_bright=True,
            output_bps=16,
            gamma=(1, 1),
            output_color=rawpy.ColorSpace.sRGB,
            demosaic_algorithm=rawpy.DemosaicAlgorithm.AHD,
            use_camera_wb=False,
        )


def center_crop(image, height, width):
    top, left = (image.shape[0] - height) // 2, (image.shape[1] - width) // 2
    return image[top:top + height, left:left + width]


def linear_srgb_to_lab(rgb16):
    """Линейный sRGB (0..65535) → CIE Lab (D65)."""
    rgb = rgb16.astype(np.float64) / 65535.0
    matrix = np.array([[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750], [0.0193339, 0.1191920, 0.9503041]])
    xyz = rgb @ matrix.T / np.array([0.95047, 1.0, 1.08883])
    f = np.where(xyz > 216 / 24389, np.cbrt(xyz), (24389 / 27 * xyz + 16) / 116)
    return np.stack([116 * f[..., 1] - 16, 500 * (f[..., 0] - f[..., 1]), 200 * (f[..., 1] - f[..., 2])], axis=-1)


def compare(reference_rgb, other_rgb):
    height = min(reference_rgb.shape[0], other_rgb.shape[0])
    width = min(reference_rgb.shape[1], other_rgb.shape[1])
    a, b = center_crop(reference_rgb, height, width), center_crop(other_rgb, height, width)
    mse = np.mean((a.astype(np.float64) - b.astype(np.float64)) ** 2)
    psnr = float("inf") if mse == 0 else 10 * np.log10(65535.0 ** 2 / mse)
    tiles_y, tiles_x = height // TILE, width // TILE
    delta = []
    for ty in range(tiles_y):
        for tx in range(tiles_x):
            sl = (slice(ty * TILE, (ty + 1) * TILE), slice(tx * TILE, (tx + 1) * TILE))
            lab_a = linear_srgb_to_lab(a[sl].reshape(-1, 3).mean(axis=0))
            lab_b = linear_srgb_to_lab(b[sl].reshape(-1, 3).mean(axis=0))
            delta.append(float(np.linalg.norm(lab_a - lab_b)))
    return psnr, float(np.mean(delta)), float(np.max(delta)), (height, width)


def build_our_dng(run_dir, output, section, raw16_override=None):
    """Собирает наш DNG примером `dng_from_capture`; `raw16_override` подменяет отсчёты кадра."""
    source = run_dir
    if raw16_override is not None:
        # Временный каталог: паспорт прогона + подложенные отсчёты под именем `*.stream.raw16`.
        source = tempfile.mkdtemp(prefix="dng-check-")
        passport = glob.glob(os.path.join(run_dir, "*.passport.json"))[0]
        stem = os.path.basename(passport)[: -len(".passport.json")]
        os.symlink(passport, os.path.join(source, stem + ".passport.json"))
        os.symlink(raw16_override, os.path.join(source, stem + ".stream.raw16"))
    subprocess.run(
        ["cargo", "run", "-q", "-p", "photo-output", "--example", "dng_from_capture", "--", source, output, "--section", section],
        check=True, cwd=REPO_ROOT, stdout=subprocess.DEVNULL,
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("runs", nargs="+", help="каталоги прогонов эталона (с *.dng, *.stream.raw16, *.passport.json)")
    parser.add_argument("--work-dir", default=None, help="куда класть наши DNG (по умолчанию временный каталог)")
    args = parser.parse_args()
    work = args.work_dir or tempfile.mkdtemp(prefix="dng-check-out-")
    os.makedirs(work, exist_ok=True)

    print(f"{'прогон':<14}{'вариант':<26}{'PSNR, дБ':>10}{'ΔE ср.':>9}{'ΔE макс.':>10}  размер сравнения")
    for run_dir in args.runs:
        run_dir = os.path.expanduser(run_dir.rstrip("/"))
        run = os.path.basename(run_dir)
        reference_dng = glob.glob(os.path.join(run_dir, "*.dng"))[0]
        stream_raw16 = glob.glob(os.path.join(run_dir, "*.stream.raw16"))[0]
        with rawpy.imread(reference_dng) as reference:
            user_wb = reference.camera_whitebalance  # множители из AsShotNeutral эталона
            reference_pixels = np.array(reference.raw_image, copy=True)  # копия: буфер rawpy умирает с контекстом
        reference_rgb = render(reference_dng, user_wb)

        pixels_file = os.path.join(work, f"{run}.reference_pixels.raw16")
        reference_pixels.astype("<u2").tofile(pixels_file)
        same_pixels_dng = os.path.join(work, f"{run}.ours_same_pixels.dng")
        stream_dng = os.path.join(work, f"{run}.ours_stream_frame.dng")
        build_our_dng(run_dir, same_pixels_dng, "still_frame", raw16_override=pixels_file)
        build_our_dng(run_dir, stream_dng, "stream_frame")

        for label, path in (("те же пиксели (теги)", same_pixels_dng), ("кадр потока (+сцена)", stream_dng)):
            psnr, mean_de, max_de, shape = compare(reference_rgb, render(path, user_wb))
            print(f"{run:<14}{label:<26}{psnr:>10.2f}{mean_de:>9.3f}{max_de:>10.3f}  {shape[1]}×{shape[0]}")
        print_tag_differences(reference_dng, same_pixels_dng)
    print(f"\nНаши DNG лежат в {work}")


if __name__ == "__main__":
    sys.exit(main())
