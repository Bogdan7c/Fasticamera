"""Фикстура эталонов белой точки: «CCT + Duv ↔ цветность» и «белая точка → нейтраль камеры».

Эталоны — colour-science 0.4.7 и colour-hdri 0.2.6 (BSD-3-Clause), независимые от Rust-кода:
- `colour.temperature.CCT_to_uv_Robertson1968` / `uv_to_CCT_Robertson1968` — метод Робертсона 1968
  с той же таблицей изотемпературных линий; знак Duv по CIE (плюс — выше кривой Планка, к зелёному);
- `colour.temperature.CCT_to_uv_Ohno2013` / `uv_to_CCT_Ohno2013` — другой метод (Ohno 2014), только
  для сведения: показывает, насколько Робертсон отличается от «точного» расстояния до кривой;
- `colour_hdri.xy_to_camera_neutral` — спецификация DNG, «Translating White Balance xy Coordinates
  to Camera Neutral Coordinates» (нормировка нейтрали на G = 1).

Rust-сторона: `crates/isp-model/src/color/white_point.rs` и `dng.rs` (`neutral_for_white`),
тест — `crates/isp-model/tests/white_point_reference.rs`.

Запуск (из корня репозитория):
    ~/.cache/fasticamera/isp-reference-venv/bin/python -I tools/isp-reference/make_white_point_fixtures.py

Выход: test-assets/isp-model/white-point-reference.{json,bin}. Числа лежат в `extra` манифеста
(JSON хранит f64 без потерь, а `.bin` формата отсчётов здесь не нужен — он пустой).
"""

from __future__ import annotations

import sys
import warnings
from dataclasses import dataclass
from pathlib import Path

import numpy as np

warnings.filterwarnings("ignore")  # colour ругается на отсутствие matplotlib — для расчётов не нужен

import colour  # noqa: E402
from colour.models import UCS_uv_to_xy  # noqa: E402
from colour.temperature import (  # noqa: E402
    CCT_to_uv_Ohno2013,
    CCT_to_uv_Robertson1968,
    uv_to_CCT_Ohno2013,
    uv_to_CCT_Robertson1968,
)
from colour_hdri import xy_to_camera_neutral  # noqa: E402

# Модуль записи лежит рядом; запуск с -I не добавляет каталог скрипта в sys.path.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from asset_io import reference_versions, write_asset  # noqa: E402

EXIT_OK = 0
EXIT_BAD_OUTPUT_DIR = 2

ASSET_NAME = "white-point-reference"
GENERATOR_COMMAND = "~/.cache/fasticamera/isp-reference-venv/bin/python -I tools/isp-reference/make_white_point_fixtures.py"

# Сетка прямой задачи: температуры из задания сессии 08 × Duv.
GRID_TEMPERATURES_KELVIN = (2500.0, 2856.0, 3200.0, 4000.0, 5000.0, 5500.0, 6500.0, 7500.0, 10000.0)
GRID_DUV = (-0.02, 0.0, 0.02)

# Точки обратной задачи (xy): D65, D50, точка A, равноэнергетический белый, зелёный/пурпурный сдвиг от D65.
INVERSE_XY = {
    "D65": (0.3127, 0.3290),
    "D50": (0.3457, 0.3585),
    "A": (0.44757, 0.40745),
    "equal_energy": (1 / 3, 1 / 3),
    "greenish_5000K": (0.3400, 0.3800),
    "magenta_5000K": (0.3500, 0.3300),
    "cool_shade": (0.2900, 0.3100),
    "warm_lamp": (0.4900, 0.4100),
}

# Белые для нейтрали камеры: точки кривой Планка (Duv = 0) по Робертсону + D65 + два смещения по Duv
# + точка холоднее калибровки A (2000 К: интерполяция прижимается к калибровке A).
NEUTRAL_PLANCKIAN_KELVIN = (2000.0, 2800.0, 4000.0, 5500.0, 6500.0)
NEUTRAL_OFF_LOCUS = ((4000.0, 0.01), (4000.0, -0.01))
D65_XY = (0.3127, 0.3290)

# Калибровка основной камеры OnePlus 11 — те же числа, что `crates/isp-model/src/test_support.rs`
# (ColorMatrix1 D65, ColorMatrix2 StdA, CameraCalibration1/2); AnalogBalance = 1 (у Camera2 его нет).
# Источник чисел — паспорт C7 «лампа» (сессия 00), матрицы как их отдаёт HAL.
CCT_STD_A = 2850.0
CCT_D65 = 6500.0
COLOR_MATRIX_D65 = np.array([[0.9848, -0.3684, -0.1011], [-0.2682, 1.1295, 0.1152], [0.0588, 0.0403, 0.5348]])
COLOR_MATRIX_STD_A = np.array([[1.9084, -1.3277, 0.1113], [-0.2221, 1.1645, 0.2236], [0.141, -0.0884, 0.8508]])
CAMERA_CALIBRATION_D65 = np.diag([1.0078125, 1.0, 1.0078125])
CAMERA_CALIBRATION_STD_A = np.diag([0.9921875, 1.0, 0.9765625])
ANALOG_BALANCE = np.ones(3)


@dataclass(frozen=True)
class ForwardCase:
    """Прямая задача: (CCT, Duv) → uv/xy."""

    cct: float
    duv: float
    robertson_uv: tuple[float, float]
    robertson_xy: tuple[float, float]
    ohno_uv: tuple[float, float]


def forward_cases() -> list[dict]:
    cases = []
    for cct in GRID_TEMPERATURES_KELVIN:
        for duv in GRID_DUV:
            robertson_uv = CCT_to_uv_Robertson1968(np.array([cct, duv]))
            ohno_uv = CCT_to_uv_Ohno2013(np.array([cct, duv]))
            case = ForwardCase(
                cct=cct,
                duv=duv,
                robertson_uv=tuple(float(value) for value in robertson_uv),
                robertson_xy=tuple(float(value) for value in UCS_uv_to_xy(robertson_uv)),
                ohno_uv=tuple(float(value) for value in ohno_uv),
            )
            cases.append(case.__dict__)
    return cases


def inverse_cases() -> list[dict]:
    cases = []
    for label, (x, y) in INVERSE_XY.items():
        uv = xy_to_uv(x, y)
        robertson_cct, robertson_duv = uv_to_CCT_Robertson1968(uv)
        ohno_cct, ohno_duv = uv_to_CCT_Ohno2013(uv)
        cases.append(
            {
                "label": label,
                "xy": [x, y],
                "uv": list(uv),
                "robertson": {"cct": float(robertson_cct), "duv": float(robertson_duv)},
                "ohno": {"cct": float(ohno_cct), "duv": float(ohno_duv)},
            }
        )
    return cases


def xy_to_uv(x: float, y: float) -> tuple[float, float]:
    """CIE 1960 UCS: u = 4x / (−2x + 12y + 3), v = 6y / (−2x + 12y + 3)."""
    denominator = -2.0 * x + 12.0 * y + 3.0
    return 4.0 * x / denominator, 6.0 * y / denominator


def camera_neutral(x: float, y: float) -> list[float]:
    """Нейтраль камеры по colour-hdri; калибровка A — «1» (colour-hdri требует CCT1 < CCT2)."""
    neutral = xy_to_camera_neutral(
        np.array([x, y]),
        CCT_STD_A,
        CCT_D65,
        COLOR_MATRIX_STD_A,
        COLOR_MATRIX_D65,
        CAMERA_CALIBRATION_STD_A,
        CAMERA_CALIBRATION_D65,
        ANALOG_BALANCE,
    )
    return [float(value) for value in neutral]


def neutral_cases() -> list[dict]:
    cases = []
    white_points = [(f"planckian_{int(t)}K", t, 0.0) for t in NEUTRAL_PLANCKIAN_KELVIN]
    white_points += [(f"{int(t)}K_duv_{duv:+.2f}", t, duv) for t, duv in NEUTRAL_OFF_LOCUS]
    for label, cct, duv in white_points:
        x, y = (float(value) for value in UCS_uv_to_xy(CCT_to_uv_Robertson1968(np.array([cct, duv]))))
        cases.append({"label": label, "xy": [x, y], "neutral": camera_neutral(x, y)})
    cases.append({"label": "D65", "xy": list(D65_XY), "neutral": camera_neutral(*D65_XY)})
    return cases


def conventions() -> dict:
    """Различия соглашений, которые объясняют допуски в Rust-тесте."""
    return {
        "duv_sign": "CIE: плюс — выше кривой Планка (к зелёному); у Робертсона colour — смещение вдоль "
        "изотермы на -Duv",
        "robertson_vs_ohno": "Ohno 2014 считает расстояние до кривой точнее; Робертсон интерполирует линии "
        "таблицы, поэтому их uv различаются (поле ohno_uv — только для сведения)",
        "neutral_normalization": "colour_hdri нормирует нейтраль на G = 1; Rust — так же",
        "neutral_interpolation": "обе стороны: линейно по майредам между CCT калибровок с прижимом к крайним; "
        "температура белого — Робертсон 1968",
        "neutral_calibration_order": "в colour_hdri калибровка с меньшей CCT (StdA) идёт первой",
    }


def write_all(out_dir: Path) -> None:
    extra = {
        "references": {
            "robertson": "colour.temperature.CCT_to_uv_Robertson1968 / uv_to_CCT_Robertson1968",
            "ohno": "colour.temperature.CCT_to_uv_Ohno2013 / uv_to_CCT_Ohno2013 (для сведения)",
            "neutral": "colour_hdri.xy_to_camera_neutral",
        },
        "reference_versions": reference_versions(["colour-science", "colour-hdri", "numpy"]),
        "conventions": conventions(),
        "calibration": {
            "source": "crates/isp-model/src/test_support.rs (OnePlus 11, основная камера)",
            "cct_standard_a": CCT_STD_A,
            "cct_d65": CCT_D65,
            "color_matrix_d65": COLOR_MATRIX_D65.tolist(),
            "color_matrix_standard_a": COLOR_MATRIX_STD_A.tolist(),
            "camera_calibration_d65_diagonal": [1.0078125, 1.0, 1.0078125],
            "camera_calibration_standard_a_diagonal": [0.9921875, 1.0, 0.9765625],
        },
        "forward": forward_cases(),
        "inverse": inverse_cases(),
        "neutral": neutral_cases(),
    }
    # Числа лежат в `extra`; массив отсчётов пустой (0x0, 1 канал, u16) — формат менять незачем.
    write_asset(
        out_dir,
        ASSET_NAME,
        np.zeros((0, 0, 1), dtype="<u2"),
        "Эталоны белой точки: CCT+Duv <-> uv/xy (Робертсон, Ohno) и нейтраль камеры по xy (colour-hdri); данные в extra.",
        GENERATOR_COMMAND,
        extra,
    )


def main() -> int:
    out_dir = Path(__file__).resolve().parents[2] / "test-assets" / "isp-model"
    if not out_dir.is_dir():
        print(f"нет каталога {out_dir}", file=sys.stderr)
        return EXIT_BAD_OUTPUT_DIR
    write_all(out_dir)
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main())
