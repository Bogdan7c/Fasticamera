#!/usr/bin/env python3
"""Собственная калибровка затенения объектива (виньетирования) по плоскому полю, сессия 14f.

Вход: прогоны сценария зонда `shading` (см. tools/device-probe/README.md и `shading_analyze.py`):
`<mode>.json` + `<mode>_base.blocks.f32` — среднее «сырое» DN по блокам 32x32 для 4 позиций CFA буфера.
Одна «укладка» (placement) — один прогон на плоском поле; несколько укладок усредняются, чтобы убрать
неравномерность подсветки конкретного стенда.

Метод:
  1. Из блоков вычитается динамический чёрный (по позициям CFA), результат делится на (белый - чёрный).
  2. Позиции буфера сопоставляются каналам Camera2 [R, G_even, G_odd, B] (G_even — зелёный в ЧЁТНЫХ строках).
  3. Каждая плоскость канала нормируется на свой центр (4x4 блока): абсолютная яркость стенда не важна.
  4. Плоскости усредняются по укладкам; на среднем подгоняется полином двух переменных (МНК, по каналу).
  5. Полином считается в узлах сетки gain-таблицы; gain = max(модель) / модель(узла), поэтому все gain >= 1,
     а наименее затенённая точка получает 1. Максимум берётся по ВСЕЙ рабочей области (центры блоков + узлы).

Почему степень 16 по умолчанию: полином степени 8 оставлял 9–28 % ошибки у самого края кадра, где
виньетка круче всего; степень 16 снижает остаток до долей процента. Базис — полиномы Лежандра (линейная
оболочка та же, что у обычных мономов u^i v^j с i+j <= 16, но МНК не вырождается из-за плохой обусловленности).
Сетка 33x25 — ровно 4:3 с шагом 128 px по ширине 4096: билинейная интерполяция таблицы между узлами
не вносит заметной ошибки на гладкой виньетке.

Конвенция сетки (интерфейс с Rust-кодом, менять нельзя): узел (c, r), c in [0, columns), r in [0, rows),
лежит в пикселе активного массива ((W-1)/(columns-1)*c, (H-1)/(rows-1)*r), строки таблицы идут подряд.
Полином вычисляется в центре этого пикселя: u = (px + 0.5)/W*2 - 1, НО координаты зажаты в прямоугольник
центров крайних блоков (без экстраполяции на последние 16 px, см. `clamp_to_measured`).

Запуск (numpy/scipy/rawpy есть в окружении ISP-референса):
  ~/.cache/fasticamera/isp-reference-venv/bin/python -I pc/shading_calibrate.py \\
      --camera 2 '<glob к main43.json>' ... --held-out 2 <main43.json> --output lens-shading.json ...
  ~/.cache/fasticamera/isp-reference-venv/bin/python -I pc/shading_calibrate.py --write-fixture <main43.json> ...
  ~/.cache/fasticamera/isp-reference-venv/bin/python -I pc/shading_calibrate.py --selftest
Полные команды — в tools/device-probe/README.md.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import glob
import json
import os
from pathlib import Path
import sys

import numpy as np

# Скрипты запускаются как `python -I` (изолированный режим): каталог скрипта НЕ попадает в sys.path,
# поэтому пакет-сосед `shading_calibration/` не нашёлся бы. Добавляем каталог скрипта явно.
sys.path.insert(0, str(Path(__file__).resolve().parent))

from shading_calibration.core import (  # noqa: E402 — путь добавлен строкой выше
    BLOCK,
    CalibrationError,
    DEFAULT_DEGREE,
    DEFAULT_GRID_COLUMNS,
    DEFAULT_GRID_ROWS,
    GainTable,
    GridSpec,
    RawRun,
    SCHEMA,
    SensorGeometry,
)
from shading_calibration.loading import (  # noqa: E402 — путь добавлен строкой выше
    load_raw_run,
    to_placement,
)
from shading_calibration.fitting import (  # noqa: E402 — путь добавлен строкой выше
    calibrate,
)
from shading_calibration.validation import (  # noqa: E402 — путь добавлен строкой выше
    cross_bench_report,
    dng_to_placement,
    leave_one_out,
    validate_on_placement,
)
from shading_calibration.output import (  # noqa: E402 — путь добавлен строкой выше
    max_corner_gain,
    render_document,
    table_points,
)
from shading_calibration.fixture import (  # noqa: E402 — путь добавлен строкой выше
    write_fixture,
)
from shading_calibration.selftest import (  # noqa: E402 — путь добавлен строкой выше
    run_selftest,
)


# ---------------------------------------------------------------------------------------------
# Командная строка
# ---------------------------------------------------------------------------------------------

@dataclass(frozen=True)
class CameraRequest:
    """Что калибруем для одной камеры."""

    camera_id: str
    json_paths: list[Path]
    held_out: list[Path]
    cross_bench: list[Path]


def expand_globs(patterns: list[str]) -> list[Path]:
    """Раскрывает ~ и маски; пустой результат — ошибка (опечатка в маске не должна дать пустую подгонку)."""
    paths: list[Path] = []
    for pattern in patterns:
        matches = sorted(glob.glob(os.path.expanduser(pattern)))
        if not matches:
            raise CalibrationError(f"маска {pattern!r} ничего не нашла")
        paths.extend(Path(m).resolve() for m in matches)
    return paths


def build_requests(args: argparse.Namespace) -> list[CameraRequest]:
    """Собирает запросы по `--camera ID МАСКА...`, `--held-out ID json`, `--cross-bench ID dng`."""
    held_by_camera: dict[str, list[Path]] = {}
    for camera_id, path in args.held_out or []:
        held_by_camera.setdefault(camera_id, []).append(Path(os.path.expanduser(path)).resolve())
    bench_by_camera: dict[str, list[Path]] = {}
    for camera_id, path in args.cross_bench or []:
        bench_by_camera.setdefault(camera_id, []).append(Path(os.path.expanduser(path)).resolve())
    requests = []
    for camera_args in args.camera:
        camera_id, patterns = camera_args[0], camera_args[1:]
        if not patterns:
            raise CalibrationError(f"--camera {camera_id}: не указана ни одна маска прогонов")
        held = held_by_camera.get(camera_id, [])
        # Отложенный прогон НЕ участвует в подгонке, даже если попал под маску.
        fit_paths = [p for p in expand_globs(patterns) if p not in held]
        requests.append(CameraRequest(camera_id, fit_paths, held, bench_by_camera.get(camera_id, [])))
    return requests


def load_placements(paths: list[Path]) -> tuple[list[RawRun], SensorGeometry]:
    """Читает прогоны и проверяет, что они согласованы (один режим, CFA, геометрия, сетка блоков)."""
    runs = [load_raw_run(p) for p in paths]
    first = runs[0]
    for run in runs[1:]:
        same = (run.mode, run.cfa, run.width, run.height, run.block_means.shape) == (
            first.mode, first.cfa, first.width, first.height, first.block_means.shape)
        if not same:
            raise CalibrationError(f"{run.label}: параметры не совпадают с {first.label} (режим/CFA/геометрия)")
    geometry = SensorGeometry(first.width, first.height, first.block_means.shape[2], first.block_means.shape[1])
    return runs, geometry


def calibrate_camera(request: CameraRequest, grid: GridSpec, degree: int) -> tuple[GainTable, dict]:
    """Калибровка одной камеры + все проверки; возвращает таблицу и блок provenance (без служебных полей стенда)."""
    runs, geometry = load_placements(request.json_paths)
    placements = [to_placement(r) for r in runs]
    table = calibrate(placements, geometry, grid, degree)
    validation: dict = {"leave_one_out": leave_one_out(placements, geometry, grid, degree)}
    held_runs = [load_raw_run(p) for p in request.held_out]
    for run in held_runs:
        if (run.mode, run.cfa, run.width, run.height) != (runs[0].mode, runs[0].cfa, runs[0].width, runs[0].height):
            raise CalibrationError(f"отложенный {run.label} не совпадает по режиму/CFA/геометрии с обучающими")
    if held_runs:
        validation["held_out_runs"] = [validate_on_placement(to_placement(r), table) for r in held_runs]
    provenance = {"placements": len(placements), "held_out_validation": validation}
    cross = [cross_bench_report(dng_to_placement(p, geometry), table) for p in request.cross_bench]
    if cross:
        provenance["cross_bench"] = cross
    return table, provenance


def print_camera_summary(camera_id: str, table: GainTable, provenance: dict) -> None:
    """Короткая сводка в stdout."""
    print(f"== камера {camera_id}: укладок {provenance['placements']}, макс. gain {max_corner_gain(table):.3f}, "
          f"мин. gain по каналам {np.round(table.gains.min(axis=(0, 1)), 4).tolist()}")
    validation = provenance["held_out_validation"]
    for item in validation["leave_one_out"]:
        print(f"   LOO {item['held_out']}: углы {item['corners_tl_tr_bl_br']} EV {item['mean_ev']:+.3f} макс.откл {item['max_abs_deviation']:.3f}")
    for item in validation.get("held_out_runs", []):
        print(f"   отложенный {item['run']}: углы {item['corners_tl_tr_bl_br']} EV {item['mean_ev']:+.3f} макс.откл {item['max_abs_deviation']:.3f}")
    for item in provenance.get("cross_bench", []):
        print(f"   кросс-стенд {item['source']}: углы {item['corners_tl_tr_bl_br']} EV {item['mean_ev']:+.3f} макс.откл {item['max_abs_deviation']:.3f}")
        print(f"      радиально {item['radial_profile_10_bins']}")


def run_calibration(args: argparse.Namespace) -> None:
    """Режим по умолчанию: калибровка всех камер и запись JSON."""
    grid = GridSpec(args.grid_columns, args.grid_rows)
    cameras = []
    for request in build_requests(args):
        table, provenance = calibrate_camera(request, grid, args.degree)
        print_camera_summary(request.camera_id, table, provenance)
        provenance = {
            "session": args.session,
            "date": args.date,
            "bench": args.bench,
            "placements": provenance.pop("placements"),
            "fit": f"poly2d degree {args.degree} (Legendre basis) on {BLOCK}x{BLOCK} blocks, gains = max/model per channel",
            "tool": "tools/device-probe/pc/shading_calibrate.py " + _command_line_for_record(),
            **provenance,
        }
        cameras.append({
            "camera_id": request.camera_id,
            "active_array": {"width": table.geometry.width, "height": table.geometry.height},
            "grid": {"columns": grid.columns, "rows": grid.rows, "points": table_points(table)},
            "provenance": provenance,
        })
    document = {
        "schema": SCHEMA,
        "device": {"manufacturer": args.device_manufacturer, "model": args.device_model},
        "cameras": cameras,
    }
    Path(args.output).parent.mkdir(parents=True, exist_ok=True)
    Path(args.output).write_text(render_document(document), encoding="utf-8")
    print(f"записано {args.output}")


def _command_line_for_record() -> str:
    """Аргументы запуска для provenance; домашний каталог заменён на ~ (в репозиторий личные пути не пишем)."""
    home = str(Path.home())
    return " ".join(arg.replace(home, "~") for arg in sys.argv[1:])



# ---------------------------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------------------------

def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0], formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--selftest", action="store_true", help="самотест на синтетике")
    parser.add_argument("--camera", nargs="+", action="append", metavar=("ID", "MASK"),
                        help="камера: ID и маски путей к <mode>.json укладок (можно несколько --camera, порядок сохраняется)")
    parser.add_argument("--held-out", nargs=2, action="append", metavar=("ID", "JSON"),
                        help="отложенный прогон камеры: исключается из подгонки, по нему проверяется таблица")
    parser.add_argument("--cross-bench", nargs=2, action="append", metavar=("ID", "DNG"),
                        help="DNG другого стенда для проверки таблицы камеры")
    parser.add_argument("--output", help="куда писать lens-shading.json")
    parser.add_argument("--device-manufacturer")
    parser.add_argument("--device-model")
    parser.add_argument("--session", default="")
    parser.add_argument("--date", default="")
    parser.add_argument("--bench", default="")
    parser.add_argument("--degree", type=int, default=DEFAULT_DEGREE)
    parser.add_argument("--grid-columns", type=int, default=DEFAULT_GRID_COLUMNS)
    parser.add_argument("--grid-rows", type=int, default=DEFAULT_GRID_ROWS)
    parser.add_argument("--write-fixture", metavar="JSON", help="записать фикстуру плоского поля из прогона")
    parser.add_argument("--fixture-dir", default="test-assets/isp-model")
    parser.add_argument("--fixture-name", default="flat-field-main-heldout")
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        if args.selftest:
            return run_selftest()
        if args.write_fixture:
            manifest = write_fixture(Path(os.path.expanduser(args.write_fixture)), Path(args.fixture_dir), args.fixture_name)
            print(f"записано {manifest}")
            return 0
        missing = [name for name in ("camera", "output", "device_manufacturer", "device_model") if not getattr(args, name)]
        if missing:
            parser.error("обязательны для калибровки: " + ", ".join("--" + m.replace("_", "-") for m in missing))
        run_calibration(args)
        return 0
    except CalibrationError as error:
        print(f"ОШИБКА: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
