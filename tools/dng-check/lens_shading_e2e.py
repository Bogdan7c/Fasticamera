"""Сквозная проверка OpcodeList2/GainMap: Adobe `dng_validate` проявляет НАШ DNG плоского поля без списка и со списком.

Что делает:
1. Достаёт отсчёты (raw16, как лежат в файле) из DNG плоского поля (rawpy читает только пиксели).
2. Пример `dng_from_capture` собирает два DNG на этих пикселях: без карты виньетирования и с картой камеры из
   `device-profiles/<устройство>/lens-shading.json` (то есть `OpcodeList2` пишет наш писатель).
3. `dng_validate -2` пишет «стадию 2» — линеаризованную мозаику ПОСЛЕ OpcodeList2 (DNG 1.7: список 2 работает над
   отсчётами до демозаика), `-tif -16` — готовое изображение. По ним меряется яркость углов к центру.

ВАЖНО: rawpy/LibRaw опкод-списки ИГНОРИРУЕТ — сравнение через rawpy коррекции не покажет; эту проверку делает только
Adobe SDK (`dng_validate`).

Запуск (из корня репозитория):
    ~/.cache/fasticamera/isp-reference-venv/bin/python -I tools/dng-check/lens_shading_e2e.py \
        ~/fasticamera-captures/14f/app-anchor/17884.dng --camera 2 --run-dir <прогон c7-reference с камерой 2>
"""
import argparse
import pathlib
import struct
import subprocess
import sys
import tempfile

import numpy as np
import rawpy

REPO = pathlib.Path(__file__).resolve().parents[2]
VALIDATE = pathlib.Path.home() / ".cache/fasticamera/tools/dng_build/dng_validate"
PROFILE = REPO / "device-profiles/oneplus-cph2447/lens-shading.json"
PATCH = 0.1  # сторона квадрата замера, доля размера кадра


def read_tiff(path):
    """Минимальное чтение несжатого TIFF от dng_validate: массив (H, W, каналы) и число бит."""
    data = pathlib.Path(path).read_bytes()
    order = "<" if data[:2] == b"II" else ">"
    (ifd,) = struct.unpack(order + "I", data[4:8])
    (count,) = struct.unpack(order + "H", data[ifd:ifd + 2])
    tags = {}
    sizes = {1: 1, 3: 2, 4: 4}
    for index in range(count):
        entry = data[ifd + 2 + 12 * index: ifd + 14 + 12 * index]
        tag, kind, n = struct.unpack(order + "HHI", entry[:8])
        if kind not in sizes:
            continue
        size = sizes[kind] * n
        raw = entry[8:8 + size] if size <= 4 else data[struct.unpack(order + "I", entry[8:12])[0]:][:size]
        code = {1: "B", 3: "H", 4: "I"}[kind]
        tags[tag] = struct.unpack(order + code * n, raw)
    width, height = tags[256][0], tags[257][0]
    bits = tags[258][0]
    samples = tags[277][0]
    assert tags.get(259, (1,))[0] == 1, "ожидали TIFF без сжатия"
    assert tags.get(284, (1,))[0] == 1, "ожидали контигуальные плоскости"
    body = b"".join(data[o:o + c] for o, c in zip(tags[273], tags[279]))
    dtype = np.dtype(order + ("u2" if bits == 16 else "u1"))
    return np.frombuffer(body, dtype=dtype).reshape(height, width, samples), bits


def patch_mean(image, centre):
    """Среднее по квадрату вокруг (cy, cx) в долях кадра; все каналы."""
    height, width = image.shape[:2]
    half_h, half_w = int(height * PATCH / 2), int(width * PATCH / 2)
    cy, cx = int(centre[0] * height), int(centre[1] * width)
    cy, cx = min(max(cy, half_h), height - half_h), min(max(cx, half_w), width - half_w)
    return float(image[cy - half_h:cy + half_h, cx - half_w:cx + half_w].astype(np.float64).mean())


def corner_to_centre(image, black=0.0):
    """Яркость четырёх углов к центру (после вычитания чёрного)."""
    centre = patch_mean(image, (0.5, 0.5)) - black
    corners = [patch_mean(image, c) - black for c in ((0, 0), (0, 1), (1, 0), (1, 1))]
    return [c / centre for c in corners], centre


def run(command):
    subprocess.run([str(part) for part in command], check=True, stdout=subprocess.DEVNULL)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("flat_dng", type=pathlib.Path, help="наш DNG плоского поля (источник пикселей)")
    parser.add_argument("--camera", required=True, help="id камеры в профиле калибровки")
    parser.add_argument("--run-dir", type=pathlib.Path, required=True, help="прогон c7-reference той же камеры (паспорт)")
    args = parser.parse_args()
    if not VALIDATE.exists():
        sys.exit(f"нет {VALIDATE}: соберите tools/dng-check/build_dng_validate.sh")

    with tempfile.TemporaryDirectory() as tmp:
        tmp = pathlib.Path(tmp)
        with rawpy.imread(str(args.flat_dng)) as raw:
            mosaic = raw.raw_image.copy()
        raw16 = tmp / "flat.raw16"
        mosaic.astype("<u2").tofile(raw16)
        example = [
            "cargo", "run", "-q", "-p", "photo-output", "--example", "dng_from_capture", "--",
            args.run_dir,
        ]
        plain, corrected = tmp / "plain.dng", tmp / "corrected.dng"
        run(example + [plain, "--raw16", raw16])
        run(example + [corrected, "--raw16", raw16, "--lens-shading", PROFILE])
        for name, dng in (("без OpcodeList2", plain), ("с OpcodeList2", corrected)):
            stem = dng.with_suffix("")
            run([VALIDATE, "-2", f"{stem}_stage2", "-16", "-tif", f"{stem}_render", dng])
            stage2, _ = read_tiff(f"{stem}_stage2.tif")
            render, _ = read_tiff(f"{stem}_render.tif")
            for label, image, level in (("стадия 2 (линейная)", stage2, 0.0), ("итог sRGB 16 бит", render, 0.0)):
                # Стадия 2 у Adobe уже без чёрного и в линейном масштабе; итог sRGB — с гамма-кривой (поэтому углы светлее).
                ratios, centre = corner_to_centre(image if image.shape[2] == 1 else image.mean(axis=2, keepdims=True), level)
                text = "  ".join(f"{ratio:.3f}" for ratio in ratios)
                print(f"{name:16} {label:20} центр {centre:9.1f}  углы/центр (ЛВ ПВ ЛН ПН): {text}")


if __name__ == "__main__":
    main()
