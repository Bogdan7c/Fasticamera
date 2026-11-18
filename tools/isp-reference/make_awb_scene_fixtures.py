"""Фикстуры сцен для оценки света сцены (AWB, сессия 08): статистика плиток 32×24 из ColorChecker.

Источник правды независим от Rust-кода:
- отражения 24 плиток ColorChecker — спектры `colour.SDS_COLOURCHECKERS['ColorChecker N Ohta']`;
- источники света — спектры Планка (`colour.sd_blackbody`) и стандартные CIE (D55, D65, F2, F11);
- XYZ плитки под источником — `colour.sd_to_XYZ` (Y идеального отражателя = 1);
- камера — OnePlus 11 (те же числа, что `crates/isp-model/src/test_support.rs`), матрица XYZ → камера
  для цветности света — `colour_hdri.matrix_XYZ_to_camera_space` (спецификация DNG);
- ожидаемый свет — цветность, CCT и Duv по Робертсону (`colour.temperature.uv_to_CCT_Robertson1968`).

Затем каждая сцена раскладывается на сетку плиток статистики AWB и квантуется ТАК ЖЕ, как
`crates/isp-model/src/statistics.rs`: `q = floor(clamp(v, 0, 1) · 4095 + 0.5)`, суммы G — по двум
зелёным отсчётам, ячейка пересвечена — только `saturated_cells`.

Раскладка (сетка 32×24): 6 × 4 плитки ColorChecker по 5 × 6 плиток статистики (столбцы 0–29);
столбец 30 — тёмные плитки (почти чёрные, ниже порога освещённости), столбец 31 — пересвеченные.
Сцены:
- `checker_<свет>` — все 24 плитки поровну (нейтральных больше всего — 6 из 24);
- `dominant_green_<свет>` — ≥ 70 % плиток из зелёных образцов (foliage, yellow green, green,
  bluish green) и немного нейтральных;
- `no_neutrals_<свет>` — только цветные образцы (серых нет);
- `single_colour_<свет>` — вся сцена одного цвета (зелёный образец).

Rust-сторона: `crates/scene-estimation/tests/scene_reference.rs`.

Запуск (из корня репозитория):
    ~/.cache/fasticamera/isp-reference-venv/bin/python -I tools/isp-reference/make_awb_scene_fixtures.py

Выход: test-assets/scene-estimation/awb-scenes.{json,bin}. Числа лежат в `extra` манифеста.
"""

from __future__ import annotations

import sys
import warnings
from pathlib import Path

import numpy as np

warnings.filterwarnings("ignore")  # colour ругается на отсутствие matplotlib — для расчётов не нужен

import colour  # noqa: E402
from colour.models import UCS_uv_to_xy, XYZ_to_xy  # noqa: E402
from colour.temperature import uv_to_CCT_Robertson1968  # noqa: E402
from colour_hdri import matrix_XYZ_to_camera_space  # noqa: E402

sys.path.insert(0, str(Path(__file__).resolve().parent))
from asset_io import reference_versions, write_asset  # noqa: E402

EXIT_OK = 0

ASSET_NAME = "awb-scenes"
GENERATOR_COMMAND = "~/.cache/fasticamera/isp-reference-venv/bin/python -I tools/isp-reference/make_awb_scene_fixtures.py"

# Калибровка основной камеры OnePlus 11 — те же числа, что `crates/isp-model/src/test_support.rs`
# (ColorMatrix1 D65, ColorMatrix2 StdA, CameraCalibration1/2); AnalogBalance = 1 (у Camera2 его нет).
CCT_STD_A = 2850.0
CCT_D65 = 6500.0
COLOR_MATRIX_D65 = np.array([[0.9848, -0.3684, -0.1011], [-0.2682, 1.1295, 0.1152], [0.0588, 0.0403, 0.5348]])
COLOR_MATRIX_STD_A = np.array([[1.9084, -1.3277, 0.1113], [-0.2221, 1.1645, 0.2236], [0.141, -0.0884, 0.8508]])
CAMERA_CALIBRATION_D65 = np.diag([1.0078125, 1.0, 1.0078125])
CAMERA_CALIBRATION_STD_A = np.diag([0.9921875, 1.0, 0.9765625])
ANALOG_BALANCE = np.ones(3)

# Сетка статистики и раскладка образцов.
GRID_COLUMNS, GRID_ROWS = 32, 24
PATCH_COLUMNS, PATCH_ROWS = 6, 4
PATCH_TILE_WIDTH, PATCH_TILE_HEIGHT = 5, 6
DARK_COLUMN, SATURATED_COLUMN = 30, 31
# 64×64 ячейки на плитку (RAW 4080×3060, сетка 32×24 — порядка 4100 ячеек).
CELLS_PER_TILE = 64 * 64
QUANT_MAX = 4095

# Яркость белого образца по зелёному (до баланса белого): так ни один канал не пересвечен.
WHITE_PATCH_GREEN_LEVEL = 0.75
# Уровень «тёмных» плиток (ниже любого порога освещённости) и пересвеченных (все ячейки).
DARK_TILE_LEVEL = 0.004

# Образцы Ohta по порядку ColorChecker (строки слева направо, сверху вниз).
PATCH_NAMES = [
    "dark skin", "light skin", "blue sky", "foliage", "blue flower", "bluish green",
    "orange", "purplish blue", "moderate red", "purple", "yellow green", "orange yellow",
    "blue", "green", "red", "yellow", "magenta", "cyan",
    "white 9.5 (.05 D)", "neutral 8 (.23 D)", "neutral 6.5 (.44 D)", "neutral 5 (.70 D)",
    "neutral 3.5 (1.05 D)", "black 2 (1.5 D)",
]
NEUTRAL_INDICES = [18, 19, 20, 21, 22, 23]
GREEN_NAMES = ["foliage", "yellow green", "green", "bluish green"]
COLOUR_INDICES = list(range(18))

# Источники света: имя → (спектр, описание).
SHAPE = colour.SpectralShape(380, 780, 5)


def illuminants() -> dict[str, colour.SpectralDistribution]:
    return {
        "bb2800": colour.sd_blackbody(2800, SHAPE),
        "bb4000": colour.sd_blackbody(4000, SHAPE),
        "d55": colour.SDS_ILLUMINANTS["D55"].copy().align(SHAPE),
        "d65": colour.SDS_ILLUMINANTS["D65"].copy().align(SHAPE),
        "fl2": colour.SDS_ILLUMINANTS["FL2"].copy().align(SHAPE),
        "fl11": colour.SDS_ILLUMINANTS["FL11"].copy().align(SHAPE),
    }


def patch_reflectances() -> list[colour.SpectralDistribution]:
    checker = colour.SDS_COLOURCHECKERS["ColorChecker N Ohta"]
    by_name = {name: sd.copy().align(SHAPE) for name, sd in checker.items()}
    return [by_name[name] for name in PATCH_NAMES]


def illuminant_white(illuminant: colour.SpectralDistribution) -> dict:
    """Цветность, CCT и Duv света (Робертсон, знак Duv по CIE)."""
    cmfs = colour.MSDS_CMFS["CIE 1931 2 Degree Standard Observer"].copy().align(SHAPE)
    # Цветность не зависит от масштаба, поэтому нормировка XYZ света не важна.
    xyz_white = colour.sd_to_XYZ(illuminant, cmfs) / 100.0
    xy = XYZ_to_xy(xyz_white)
    u = 4.0 * xy[0] / (-2.0 * xy[0] + 12.0 * xy[1] + 3.0)
    v = 6.0 * xy[1] / (-2.0 * xy[0] + 12.0 * xy[1] + 3.0)
    cct, duv = uv_to_CCT_Robertson1968(np.array([u, v]))
    return {"xy": [float(xy[0]), float(xy[1])], "cct": float(cct), "duv": float(duv)}


def camera_rgb_of_patches(
    illuminant: colour.SpectralDistribution, reflectances: list[colour.SpectralDistribution], xy: list[float]
) -> np.ndarray:
    """Камера (до баланса белого) для каждого образца под этим светом, форма (24, 3).

    XYZ образца — относительно света (идеальный отражатель имеет Y = 1), как в формуле DNG
    «нейтраль = XYZtoCamera · XYZ(белая, Y = 1)»; матрицу XYZtoCamera даёт colour-hdri для цветности света.
    """
    cmfs = colour.MSDS_CMFS["CIE 1931 2 Degree Standard Observer"].copy().align(SHAPE)
    xyz_to_camera = matrix_XYZ_to_camera_space(
        np.array(xy),
        CCT_STD_A,
        CCT_D65,
        COLOR_MATRIX_STD_A,
        COLOR_MATRIX_D65,
        CAMERA_CALIBRATION_STD_A,
        CAMERA_CALIBRATION_D65,
        ANALOG_BALANCE,
    )
    # `sd_to_XYZ(отражение, cmfs, свет)` сам перемножает отражение и свет и нормирует Y идеального отражателя в 100.
    patches = [colour.sd_to_XYZ(reflectance, cmfs, illuminant=illuminant) / 100.0 for reflectance in reflectances]
    return np.array([xyz_to_camera @ xyz for xyz in patches])


def quantize(value: float) -> int:
    """Квантование отсчёта как `isp_model::statistics::quantize_sample` (формула в f32)."""
    clipped = min(max(float(np.float32(value)), 0.0), 1.0)
    return int(np.floor(np.float32(clipped) * np.float32(QUANT_MAX) + np.float32(0.5)))


# Маркер полностью пересвеченной плитки в `.bin` (настоящие значения — 0…4095).
SATURATED_TILE_MARKER = 0xFFFF


def tile_for_colour(rgb: np.ndarray | None) -> list[int]:
    """Плитка `[q_r, q_g, q_b]` — квантованные отсчёты КАЖДОЙ ячейки (все ячейки плитки одинаковы).

    Суммы статистики Rust-тест восстанавливает по формуле контракта: `sum_r = q_r · ячеек`,
    `sum_g = 2 · q_g · ячеек` (два зелёных отсчёта на ячейку), `used = ячеек`. `None` — плитка
    пересвечена целиком (`saturated_cells = ячеек`, суммы нули): три маркера `SATURATED_TILE_MARKER`.
    """
    if rgb is None:
        return [SATURATED_TILE_MARKER] * 3
    return [quantize(channel) for channel in rgb]


def layout_tiles(patch_assignment: list[int | None], colours: np.ndarray, dark_colour: np.ndarray) -> list[list[int]]:
    """Сетка плиток по строкам. `patch_assignment` — 24 индекса образцов (по раскладке 6×4) или None.

    Образец `None` в позиции раскладки — тёмная плитка. Столбец 30 — тёмный, 31 — пересвеченный.
    """
    tiles: list[list[int]] = []
    for row in range(GRID_ROWS):
        for column in range(GRID_COLUMNS):
            if column == SATURATED_COLUMN:
                tiles.append(tile_for_colour(None))
            elif column == DARK_COLUMN:
                tiles.append(tile_for_colour(dark_colour))
            else:
                position = (row // PATCH_TILE_HEIGHT) * PATCH_COLUMNS + column // PATCH_TILE_WIDTH
                patch = patch_assignment[position]
                tiles.append(tile_for_colour(dark_colour if patch is None else colours[patch]))
    return tiles


def scenes_for_light(name: str, illuminant: colour.SpectralDistribution, reflectances) -> list[dict]:
    white = illuminant_white(illuminant)
    raw = camera_rgb_of_patches(illuminant, reflectances, white["xy"])
    exposure = WHITE_PATCH_GREEN_LEVEL / raw[PATCH_NAMES.index("white 9.5 (.05 D)")][1]
    colours = raw * exposure
    # Тёмная плитка — серая по цвету света (нейтраль света, умноженная на малый уровень).
    white_rgb = raw[PATCH_NAMES.index("white 9.5 (.05 D)")]
    dark = white_rgb / white_rgb[1] * DARK_TILE_LEVEL

    def scene(kind: str, assignment: list[int | None], note: str) -> dict:
        return {
            "name": f"{kind}_{name}",
            "kind": kind,
            "light": name,
            "expected": {"xy": white["xy"], "cct": white["cct"], "duv": white["duv"]},
            "note": note,
            "tiles": layout_tiles(assignment, colours, dark),  # уходит в .bin, не в манифест
        }

    all_patches: list[int | None] = list(range(24))
    greens = [PATCH_NAMES.index(green) for green in GREEN_NAMES]
    neutrals_few = [PATCH_NAMES.index(n) for n in ("white 9.5 (.05 D)", "neutral 8 (.23 D)", "neutral 6.5 (.44 D)")]
    # Зелёные: 4 столбца образцов из 6 в трёх рядах + нейтральные только в одном ряду.
    dominant: list[int | None] = []
    for position in range(24):
        row, column = divmod(position, PATCH_COLUMNS)
        if row == 3 and column >= 4:
            dominant.append(neutrals_few[column - 4])  # две плитки нейтральных (а третья — ниже)
        elif row == 3 and column == 3:
            dominant.append(neutrals_few[2])
        else:
            dominant.append(greens[position % len(greens)])
    no_neutrals: list[int | None] = [COLOUR_INDICES[position % len(COLOUR_INDICES)] for position in range(24)]
    single: list[int | None] = [PATCH_NAMES.index("foliage")] * 24
    return [
        scene("checker", all_patches, "все 24 образца поровну; 6 нейтральных из 24"),
        scene("dominant_green", dominant, "21 из 24 плиток раскладки — зелёные образцы, 3 — нейтральные"),
        scene("no_neutrals", no_neutrals, "только цветные образцы, серых нет"),
        scene("single_colour", single, "вся сцена одного цвета (foliage)"),
    ]


def write_all(out_dir: Path) -> None:
    reflectances = patch_reflectances()
    scenes: list[dict] = []
    lights: dict[str, dict] = {}
    for name, illuminant in illuminants().items():
        lights[name] = illuminant_white(illuminant)
        scenes.extend(scenes_for_light(name, illuminant, reflectances))
    tiles = np.array([scene.pop("tiles") for scene in scenes], dtype="<u2")  # (сцен, 768, 3)
    extra = {
        "references": {
            "reflectances": "colour.SDS_COLOURCHECKERS['ColorChecker N Ohta']",
            "illuminants": "colour.sd_blackbody, colour.SDS_ILLUMINANTS D55/D65/FL2/FL11",
            "camera": "colour_hdri.matrix_XYZ_to_camera_space (DNG, спецификация)",
            "light_duv": "colour.temperature.uv_to_CCT_Robertson1968 (знак Duv по CIE)",
        },
        "reference_versions": reference_versions(["colour-science", "colour-hdri", "numpy"]),
        "calibration": {
            "source": "crates/isp-model/src/test_support.rs (OnePlus 11, основная камера)",
            "cct_standard_a": CCT_STD_A,
            "cct_d65": CCT_D65,
            "color_matrix_d65": COLOR_MATRIX_D65.tolist(),
            "color_matrix_standard_a": COLOR_MATRIX_STD_A.tolist(),
            "camera_calibration_d65_diagonal": [1.0078125, 1.0, 1.0078125],
            "camera_calibration_standard_a_diagonal": [0.9921875, 1.0, 0.9765625],
        },
        "grid": {"columns": GRID_COLUMNS, "rows": GRID_ROWS},
        "cells_per_tile": CELLS_PER_TILE,
        "bin_layout": "u16le, высота = сцены по порядку `scenes`, ширина = 768 плиток по строкам, 3 канала "
        "= квантованные отсчёты ячейки (q_r, q_g, q_b); 65535 — плитка пересвечена целиком",
        "layout": "по строкам; образцы 6×4 по 5×6 плиток (столбцы 0–29), столбец 30 тёмный, 31 пересвечен",
        "white_patch_green_level": WHITE_PATCH_GREEN_LEVEL,
        "dark_tile_level": DARK_TILE_LEVEL,
        "patch_names": PATCH_NAMES,
        "lights": lights,
        "scenes": scenes,
    }
    write_asset(
        out_dir,
        ASSET_NAME,
        tiles,
        "Статистика плиток 32×24 сцен ColorChecker под разными светами (оценка света сцены, сессия 08); манифест — свет и ожидания",
        GENERATOR_COMMAND,
        extra,
    )


def main() -> int:
    out_dir = Path(__file__).resolve().parents[2] / "test-assets" / "scene-estimation"
    write_all(out_dir)
    print(f"записано: {out_dir / (ASSET_NAME + '.json')}")
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main())
