"""Запись фикстур isp-model: манифест <имя>.json + отсчёты <имя>.bin.

Формат описан в test-assets/isp-model/README.md. Отсчёты — little-endian, порядок строк,
каналы подряд. Читатель на стороне Rust: crates/isp-model/tests/support/mod.rs.
"""

from __future__ import annotations

import json
from importlib import metadata
from pathlib import Path

import numpy as np

# Версия формата манифеста: меняется, только если меняется смысл полей.
ASSET_FORMAT_VERSION = 1

# Поддержанные типы отсчётов: numpy dtype → имя в манифесте.
_SAMPLE_TYPES = {np.dtype("<u2"): "u16le", np.dtype("<f4"): "f32le"}


def write_asset(
    out_dir: Path,
    name: str,
    samples: np.ndarray,
    description: str,
    generator: str,
    extra: dict | None = None,
) -> Path:
    """Пишет фикстуру; samples — (H, W) или (H, W, C) массив u16 или f32."""
    if samples.ndim == 2:
        samples = samples[:, :, None]
    if samples.ndim != 3:
        raise ValueError(f"{name}: ожидался массив (H, W[, C]), пришёл {samples.shape}")
    # Явно приводим порядок байт к little-endian, не меняя тип.
    little_endian = samples.astype(samples.dtype.newbyteorder("<"), copy=False)
    sample_type = _SAMPLE_TYPES.get(little_endian.dtype)
    if sample_type is None:
        raise ValueError(f"{name}: тип {samples.dtype} не поддержан (только u16 и f32)")
    height, width, channels = little_endian.shape
    data_file = f"{name}.bin"
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / data_file).write_bytes(np.ascontiguousarray(little_endian).tobytes())
    manifest = {
        "format_version": ASSET_FORMAT_VERSION,
        "width": width,
        "height": height,
        "channels": channels,
        "sample_type": sample_type,
        "data_file": data_file,
        "description": description,
        "generator": generator,
        "extra": extra or {},
    }
    manifest_path = out_dir / f"{name}.json"
    manifest_path.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return manifest_path


def reference_versions(packages: list[str]) -> dict[str, str]:
    """Версии пакетов-эталонов для манифеста: другая версия может дать другие числа."""
    return {package: metadata.version(package) for package in packages}
