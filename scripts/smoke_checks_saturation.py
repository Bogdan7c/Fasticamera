"""Ожидания smoke-сценария ``gpu-saturation-bench`` (сессия 16a, P4b): цена работ GPU при насыщении.

Вердикт судит Rust (`scenario/gpu_saturation.rs`: работа без цифры или лишняя продуктовая работа потока рендера — провал).
Здесь независимая грубая перепроверка сводки строки результата, чтобы пустая или усечённая таблица не прошла как успех:
* каждая ЗАПРОШЕННАЯ работа (`report.requested`) есть в таблице ровно один раз и с вердиктом `ok`;
* у каждой строки есть цифра: к/с и мс/кадр положительны и согласованы (`к/с × мс/кадр ≈ 1000`, цифры округлены до
  трёх значащих);
* `measured` равно числу запрошенных работ, прогон не отменён;
* камера на время замера была закрыта сценарием (или движка нет вовсе), а поток рендера за замер выполнил продуктовой
  работы GPU не больше допуска (`render_tolerance_frames`).
Строка таблицы — `имя|к/с|мс/кадр|вердикт` (контракт с `saturation_bench/summary.rs`); подробности по работам лежат в
`FASTICAMERA_DETAIL` (стадия `detail_rows`) и печатаются скриптом.

Каждая проверка — чистая функция «JSON результата → список проблем» (пустой список — прошёл).
"""

from __future__ import annotations

from smoke_checks import _is_number, _report_problem

TABLE_COLUMNS = 4
"""Столбцов в строке таблицы: имя, к/с, мс/кадр, вердикт."""

OK_VERDICT = "ok"
"""Вердикт строки с цифрой (`WorkloadRow::verdict_word`)."""

COST_CONSISTENCY_TOLERANCE = 0.02
"""Допуск согласованности `к/с × мс/кадр = 1000`: обе цифры округлены до трёх значащих (до ±0,5 % каждая)."""

ACCEPTED_CAMERA_STATES = frozenset({"closed_by_scenario", "engine_unavailable"})
"""Что сценарий вправе сказать о камере на время замера: закрыта им или движка нет (стримить было нечему)."""


def check_gpu_saturation_bench(result: dict) -> list[str]:
    """``gpu-saturation-bench``: все запрошенные работы с цифрой, камера тиха, рендер не делил GPU со стендом."""
    problems, report = _report_problem(result)
    if problems:
        return problems
    requested = report.get("requested")
    if not isinstance(requested, list) or not requested or not all(isinstance(name, str) for name in requested):
        return [f"нет списка запрошенных работ (report.requested={requested!r})"]
    problems.extend(_table_problems(report, requested))
    if report.get("measured") != len(requested):
        problems.append(f"с цифрой {report.get('measured')!r} работ из {len(requested)} запрошенных")
    if report.get("cancelled") is not False:
        problems.append(f"прогон отменён или флаг отмены не передан (cancelled={report.get('cancelled')!r})")
    if report.get("camera") not in ACCEPTED_CAMERA_STATES:
        problems.append(f"камера на время замера: {report.get('camera')!r}, ожидалось одно из {sorted(ACCEPTED_CAMERA_STATES)}")
    problems.extend(_render_problems(report))
    return problems


def _parse_row(row: object) -> tuple[str, str, str, str] | None:
    """Строка таблицы → (имя, к/с, мс/кадр, вердикт); `None`, если формат не тот."""
    if not isinstance(row, str):
        return None
    parts = row.split("|")
    if len(parts) != TABLE_COLUMNS or not parts[0]:
        return None
    return parts[0], parts[1], parts[2], parts[3]


def _table_problems(report: dict, requested: list[str]) -> list[str]:
    table = report.get("table")
    if not isinstance(table, list):
        return [f"нет таблицы результата (report.table={table!r})"]
    problems: list[str] = []
    rows: dict[str, tuple[str, str, str]] = {}
    for raw in table:
        parsed = _parse_row(raw)
        if parsed is None:
            problems.append(f"строка таблицы {raw!r} не в формате имя|к/с|мс/кадр|вердикт")
            continue
        name, fps, cost, verdict = parsed
        if name in rows:
            problems.append(f"работа {name} в таблице дважды")
        rows[name] = (fps, cost, verdict)
    for name in requested:
        if name not in rows:
            problems.append(f"запрошенной работы {name} нет в таблице")
            continue
        problems.extend(_row_problems(name, *rows[name]))
    for name in rows:
        if name not in requested:
            problems.append(f"в таблице работа {name}, которую не просили")
    return problems


def _row_problems(name: str, fps_text: str, cost_text: str, verdict: str) -> list[str]:
    if verdict != OK_VERDICT:
        return [f"{name}: вердикт {verdict!r}, ожидался {OK_VERDICT!r} (цифры нет или работа не удалась)"]
    try:
        fps, cost = float(fps_text), float(cost_text)
    except ValueError:
        return [f"{name}: к/с {fps_text!r} и мс/кадр {cost_text!r} не числа при вердикте ok"]
    if not (fps > 0 and cost > 0):
        return [f"{name}: к/с {fps} и мс/кадр {cost} должны быть больше нуля"]
    product = fps * cost
    if abs(product - 1000.0) > 1000.0 * COST_CONSISTENCY_TOLERANCE:
        return [f"{name}: к/с {fps} и мс/кадр {cost} не согласованы (произведение {product:.1f}, ожидалось ≈ 1000)"]
    return []


def _render_problems(report: dict) -> list[str]:
    render = report.get("render")
    if not isinstance(render, dict):
        return [f"нет наблюдения за потоком рендера (report.render={render!r})"]
    if render.get("state") != "observed":
        # Рендер не поднялся: делить GPU со стендом некому. Причина печатается заметкой.
        return []
    tolerance = report.get("render_tolerance_frames")
    delta = render.get("delta")
    if not _is_number(tolerance) or not isinstance(delta, dict):
        return [f"нет допуска или прироста счётчиков рендера (tolerance={tolerance!r}, delta={delta!r})"]
    product_frames = sum(delta.get(key, 0) for key in ("isp_frames", "recomposed_frames", "ui_frames_drawn"))
    if product_frames > tolerance:
        return [f"поток рендера выполнил {product_frames} кадров продуктовой работы GPU за замер при допуске {tolerance}"]
    return []


def saturation_notes(result: dict) -> list[str]:
    """Пояснения к прошедшему прогону: откуда сенсор, какой вид, validation, что делал рендер."""
    report = result.get("report")
    if not isinstance(report, dict):
        return []
    notes: list[str] = []
    sensor = report.get("sensor") if isinstance(report.get("sensor"), dict) else {}
    if sensor.get("source") == "fallback":
        notes.append(f"сенсор: ЗАПАСНОЙ набор ({sensor.get('reason')}): цифры не от калибровки этого телефона")
    elif sensor.get("source") == "engine":
        notes.append(
            f"сенсор: камера {sensor.get('camera')}, карта теней "
            f"{'есть' if sensor.get('lens_shading_calibrated') else 'ОТСУТСТВУЕТ'}"
        )
    look = report.get("look") if isinstance(report.get("look"), dict) else {}
    notes.append(f"вид: {look.get('preset')!r}, основа тона {look.get('tone_base')!r}")
    if report.get("validation") == "enabled":
        notes.append("validation включён (debug): цифры завышены, для цен берите release")
    render = report.get("render") if isinstance(report.get("render"), dict) else {}
    if render.get("state") == "observed" and isinstance(render.get("delta"), dict):
        notes.append(f"поток рендера за замер: {render['delta']}")
    elif render.get("state") == "render_unavailable":
        notes.append(f"рендер недоступен ({render.get('reason')}): делить GPU со стендом было некому")
    return notes
