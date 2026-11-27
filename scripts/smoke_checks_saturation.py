"""Ожидания smoke-сценария ``gpu-saturation-bench`` (сессия 16a, P4b): цена работ GPU при насыщении.

Вердикт судит Rust (`scenario/gpu_saturation.rs`: работа без цифры или лишняя продуктовая работа потока рендера — провал).
Здесь независимая грубая перепроверка сводки строки результата, чтобы пустая или усечённая таблица не прошла как успех:
* каждая ЗАПРОШЕННАЯ работа есть в таблице ровно один раз и с вердиктом `ok`. С 14o сводка не повторяет имена
  (они уже в таблице): `requested_count` — сколько просили, `requested_missing` — кому не досталось строки; число строк
  таблицы плюс пропавшие обязано равняться числу запрошенных. Старый формат (`requested` — список имён) читается как раньше;
* у каждой строки есть цифра: к/с и мс/кадр положительны и согласованы (`к/с × мс/кадр ≈ 1000`, цифры округлены до
  трёх значащих);
* `measured` равно числу запрошенных работ, прогон не отменён;
* паритет на устройстве (14o, `look_parity_*`): строк паритета столько, сколько запрошено работ паритета (число
  `parity_expected` Rust берёт из каталога по списку запрошенных), у каждой ошибка математики не больше допуска и
  вердикт `pass`; нет числа (`unavailable`) — провал;
* размер картинки приборов (14o, пункт 5): у каждой работы `scopes_*` в `scope_pictures` названо `ШxВ` (положительные числа);
* камера на время замера была закрыта сценарием (или движка нет вовсе), а поток рендера за замер выполнил продуктовой
  работы GPU не больше допуска (`render_tolerance_frames`).
Строка таблицы — `имя|к/с|мс/кадр|вердикт` (контракт с `saturation_bench/summary.rs`); подробности по работам лежат в
`FASTICAMERA_DETAIL` (стадия `detail_rows`) и печатаются скриптом.

Каждая проверка — чистая функция «JSON результата → список проблем» (пустой список — прошёл).

Отдельно — сравнение двух сохранённых прогонов (`compare_runs`, CLI `python3 smoke_checks_saturation.py compare A B`):
основной инструмент цикла «база → правка → новое». Читает вывод `device-smoke.sh` (строки `FASTICAMERA_DETAIL` со
стадией `detail_rows` и/или `FASTICAMERA_RESULT`) и печатает по каждой работе мс/кадр A → B, изменение в % и
дайджест выхода: совпал / различается / недоступен.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

from smoke_checks import _is_number, _report_problem

SCOPE_SIZE = re.compile(r"^(\d+)x(\d+)$")
"""Размер картинки прибора в строке `scope_pictures`: `ШxВ`."""

TABLE_COLUMNS = 4
"""Столбцов в строке таблицы: имя, к/с, мс/кадр, вердикт."""

OK_VERDICT = "ok"
"""Вердикт строки с цифрой (`WorkloadRow::verdict_word`)."""

COST_CONSISTENCY_TOLERANCE = 0.02
"""Допуск согласованности `к/с × мс/кадр = 1000`: обе цифры округлены до трёх значащих (до ±0,5 % каждая)."""

PARITY_COLUMNS = 5
"""Столбцов в строке паритета: имя, ошибка математики, max |Δ|, допуск, вердикт (`saturation_bench/parity_report.rs`)."""

PARITY_PASS_VERDICT = "pass"
"""Вердикт строки паритета, когда допуск держится."""

PARITY_WORK_PREFIX = "look_parity_"
"""Имя работы паритета в каталоге: подстраховка на случай, когда прогон вообще не прислал поля паритета."""

SCOPE_WORK_PREFIX = "scopes_"
"""Имя работы цветового прибора в каталоге: подстраховка на случай, когда прогон не прислал размеры картинок."""

ACCEPTED_CAMERA_STATES = frozenset({"closed_by_scenario", "engine_unavailable"})
"""Что сценарий вправе сказать о камере на время замера: закрыта им или движка нет (стримить было нечему)."""


def check_gpu_saturation_bench(result: dict) -> list[str]:
    """``gpu-saturation-bench``: все запрошенные работы с цифрой, камера тиха, рендер не делил GPU со стендом."""
    problems, report = _report_problem(result)
    if problems:
        return problems
    requested, requested_problems = _requested_names(report)
    if requested is None:
        return requested_problems
    problems.extend(requested_problems)
    problems.extend(_table_problems(report, requested))
    if report.get("measured") != len(requested):
        problems.append(f"с цифрой {report.get('measured')!r} работ из {len(requested)} запрошенных")
    if report.get("cancelled") is not False:
        problems.append(f"прогон отменён или флаг отмены не передан (cancelled={report.get('cancelled')!r})")
    if report.get("camera") not in ACCEPTED_CAMERA_STATES:
        problems.append(f"камера на время замера: {report.get('camera')!r}, ожидалось одно из {sorted(ACCEPTED_CAMERA_STATES)}")
    problems.extend(_render_problems(report))
    problems.extend(_digest_problems(report, requested))
    problems.extend(_parity_problems(report, requested))
    problems.extend(_scope_picture_problems(report, requested))
    return problems


def _requested_names(report: dict) -> tuple[list[str] | None, list[str]]:
    """Имена запрошенных работ и проблемы с ними. Новый формат (14o): имена таблицы по порядку плюс
    `requested_missing`, а `requested_count` сверяет, что ни одна работа не пропала молча."""
    legacy = report.get("requested")
    if legacy is not None:
        if not isinstance(legacy, list) or not legacy or not all(isinstance(name, str) for name in legacy):
            return None, [f"нет списка запрошенных работ (report.requested={legacy!r})"]
        return legacy, []
    count = report.get("requested_count")
    missing = report.get("requested_missing")
    if isinstance(count, bool) or not isinstance(count, int) or count <= 0:
        return None, [f"нет числа запрошенных работ (report.requested_count={count!r})"]
    if not isinstance(missing, list) or not all(isinstance(name, str) for name in missing):
        return None, [f"report.requested_missing не список имён: {missing!r}"]
    table = report.get("table")
    rows = table if isinstance(table, list) else []
    names = [parsed[0] for parsed in map(_parse_row, rows) if parsed is not None]
    problems = [f"запрошенной работы {name} нет в таблице" for name in missing]
    if len(names) + len(missing) != count:
        problems.append(
            f"в таблице {len(names)} работ и {len(missing)} пропавших, а запрошено {count}: строка потерялась"
        )
    return names + missing, problems


def _expected_count(report: dict, key: str) -> tuple[int | None, list[str]]:
    """Сколько строк обязано прийти (`parity_expected`, `scope_pictures_expected`): число Rust берёт из каталога по
    списку ЗАПРОШЕННЫХ работ, а не по пришедшим строкам, поэтому пропавшая строка не сходит за успех."""
    expected = report.get(key)
    if expected is None:
        return None, []
    if isinstance(expected, bool) or not isinstance(expected, int) or expected <= 0:
        return None, [f"report.{key} не положительное целое: {expected!r}"]
    return expected, []


def _rows_of(report: dict, key: str) -> tuple[list | None, list[str]]:
    rows = report.get(key)
    if not isinstance(rows, list):
        return None, [f"report.{key} не список или его нет: {rows!r}"]
    return rows, []


def _parity_problems(report: dict, requested: list[str]) -> list[str]:
    """Паритет GPU с CPU-эталоном на устройстве (`look_parity_*`): строка на КАЖДУЮ запрошенную работу паритета, у
    каждой ошибка математики ≤ допуска и вердикт `pass`. Нет числа (`unavailable`, строка пропала, не разобралась) — провал."""
    expected, problems = _expected_count(report, "parity_expected")
    if expected is None:
        if not problems and any(name.startswith(PARITY_WORK_PREFIX) for name in requested):
            problems.append("запрошены работы паритета, а прогон не прислал parity_expected: измерения паритета нет")
        return problems
    rows, row_problems = _rows_of(report, "parity")
    if rows is None:
        return problems + row_problems
    if len(rows) != expected:
        problems.append(f"строк паритета {len(rows)} из {expected} запрошенных работ паритета")
    seen: set[str] = set()
    for raw in rows:
        problems.extend(_parity_row_problems(raw, requested, seen))
    return problems


def _parity_row_problems(raw: object, requested: list[str], seen: set[str]) -> list[str]:
    parts = raw.split("|") if isinstance(raw, str) else []
    if len(parts) != PARITY_COLUMNS or not parts[0]:
        return [f"строка паритета {raw!r} не в формате имя|ошибка|max_abs|допуск|вердикт"]
    name, excess_text, max_abs_text, tolerance_text, verdict = parts
    problems: list[str] = []
    if name not in requested:
        problems.append(f"паритет работы {name!r}, которую не просили")
    if name in seen:
        problems.append(f"паритет работы {name} дважды")
    seen.add(name)
    try:
        excess, max_abs, tolerance = float(excess_text), float(max_abs_text), float(tolerance_text)
    except ValueError:
        return problems + [f"{name}: паритет не измерен (вердикт {verdict!r}, числа {excess_text!r}/{tolerance_text!r})"]
    if not (tolerance > 0 and excess >= 0 and max_abs >= 0 and excess == excess and max_abs == max_abs):
        return problems + [f"{name}: числа паритета не годятся (ошибка {excess}, max |Δ| {max_abs}, допуск {tolerance})"]
    if excess > tolerance:
        problems.append(f"{name}: ошибка математики {excess:.2e} больше допуска {tolerance:.0e}")
    if verdict != PARITY_PASS_VERDICT:
        problems.append(f"{name}: вердикт паритета {verdict!r}, ожидался {PARITY_PASS_VERDICT!r}")
    return problems


def _scope_picture_problems(report: dict, requested: list[str]) -> list[str]:
    """Размер картинки каждой работы `scopes_*` (пункт 5 сессии 14o): отчёт обязан называть настоящий размер раздела.
    Строка `имя|ШxВ`, оба числа положительные; `имя|-` (размер не назван) — провал."""
    expected, problems = _expected_count(report, "scope_pictures_expected")
    if expected is None:
        if not problems and any(name.startswith(SCOPE_WORK_PREFIX) for name in requested):
            problems.append("запрошены приборы scopes_*, а прогон не прислал scope_pictures_expected: размер картинки не назван")
        return problems
    rows, row_problems = _rows_of(report, "scope_pictures")
    if rows is None:
        return problems + row_problems
    if len(rows) != expected:
        problems.append(f"размеров картинок приборов {len(rows)} из {expected} запрошенных работ scopes_*")
    seen: set[str] = set()
    for raw in rows:
        name, _, size = str(raw).partition("|")
        match = SCOPE_SIZE.match(size)
        if not name or match is None or int(match.group(1)) == 0 or int(match.group(2)) == 0:
            problems.append(f"размер картинки {raw!r} не в формате имя|ШxВ с положительными числами")
            continue
        if name not in requested:
            problems.append(f"размер картинки работы {name!r}, которую не просили")
        if name in seen:
            problems.append(f"размер картинки работы {name} дважды")
        seen.add(name)
    return problems


def _digest_problems(report: dict, requested: list[str]) -> list[str]:
    """Список `digests` (`имя|16 hex` или `имя|-`) необязателен (старые прогоны, большие списки работ), но если он
    есть, то разобран, про запрошенные работы и без повторов."""
    digests = report.get("digests")
    if digests is None:
        return []
    if not isinstance(digests, list):
        return [f"report.digests не список: {digests!r}"]
    problems: list[str] = []
    seen: set[str] = set()
    for raw in digests:
        name, _, value = str(raw).partition("|")
        if name not in requested:
            problems.append(f"дайджест работы {name!r}, которую не просили: {raw!r}")
        elif name in seen:
            problems.append(f"дайджест работы {name} дважды")
        if value != "-" and not DIGEST_HEX.match(value):
            problems.append(f"дайджест {raw!r} не в формате имя|16 hex-цифр или имя|-")
        seen.add(name)
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


def _parity_notes(report: dict) -> list[str]:
    """Заметка о паритете: худшая ошибка математики среди измеренных работ и её допуск."""
    rows = report.get("parity")
    if not isinstance(rows, list) or not rows:
        return []
    worst: tuple[float, str, str] | None = None
    for raw in rows:
        parts = str(raw).split("|")
        if len(parts) != PARITY_COLUMNS:
            continue
        try:
            excess = float(parts[1])
        except ValueError:
            continue
        if worst is None or excess > worst[0]:
            worst = (excess, parts[0], parts[3])
    if worst is None:
        return [f"паритет: {len(rows)} работ без числа"]
    return [f"паритет на устройстве: худшая ошибка математики {worst[0]:.2e} у {worst[1]} (допуск {worst[2]}), работ {len(rows)}"]


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
    digests = report.get("digests")
    if isinstance(digests, list):
        missing = sum(1 for entry in digests if str(entry).endswith("|-"))
        if missing:
            notes.append(f"дайджест выхода недоступен у {missing} работ из {len(digests)} (причины — в FASTICAMERA_DETAIL)")
    elif report.get("digests_in_detail_only"):
        notes.append("дайджесты не влезли в строку результата: смотрите FASTICAMERA_DETAIL (detail_rows)")
    notes.extend(_parity_notes(report))
    pictures = report.get("scope_pictures")
    if isinstance(pictures, list) and pictures:
        notes.append("размер картинки приборов: " + ", ".join(str(item).replace("|", " ") for item in pictures))
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


# ---------------------------------------------------------------------------------------------------------------------
# Сравнение двух сохранённых прогонов: «база → новое».
# ---------------------------------------------------------------------------------------------------------------------

DETAIL_MARKER = "FASTICAMERA_DETAIL "
"""Префикс строки подробностей в выводе `device-smoke.sh` (контракт с `smoke_wait.DETAIL_LINE_MARKER`)."""

RESULT_MARKER = "FASTICAMERA_RESULT "
"""Префикс строки результата в logcat (контракт с `smoke_wait.RESULT_MARKER`)."""

DETAIL_ROWS_STAGE = "detail_rows"
"""Стадия, которой сценарий пишет строки работ (`scenario/gpu_saturation.rs`)."""

DIGEST_HEX = re.compile(r"^[0-9a-f]{16}$")
"""Дайджест в таблице и подробностях: 16 шестнадцатеричных цифр (`OutputDigest`)."""

@dataclass(frozen=True)
class SaturationRow:
    """Одна работа из сохранённого прогона."""

    name: str
    verdict: str
    frame_cost_ms: float | None
    digest: str | None
    """Общий дайджест; `None` — недоступен или его нет в прогоне (старый формат)."""
    digest_parts: dict[str, str]
    digest_unavailable_reason: str | None = None
    frame_note: str | None = None


@dataclass(frozen=True)
class RowComparison:
    """Сравнение одной работы."""

    name: str
    cost_a_ms: float | None
    cost_b_ms: float | None
    change_percent: float | None
    digest_status: str
    """`совпал`, `различается`, `недоступен` или `нет в A` / `нет в B` (работа есть только в одном прогоне)."""
    detail: str = ""


DIGEST_SAME = "совпал"
DIGEST_DIFFERENT = "различается"
DIGEST_UNAVAILABLE = "недоступен"


def parse_saturation_rows(text: str) -> dict[str, SaturationRow]:
    """Строки работ из сохранённого вывода `device-smoke.sh` (или сырого logcat) по порядку прогона.

    Источники: подробности `detail_rows` (полные: мс/кадр, дайджест и его части) и таблица результата
    `FASTICAMERA_RESULT` (`имя|к/с|мс/кадр|вердикт`, дайджесты в `digests`). Подробности главнее: они точнее
    (мс/кадр без округления), результат дополняет тем, чего в подробностях нет. Пустой словарь — в тексте нет
    ни того ни другого.
    """
    from_details: dict[str, SaturationRow] = {}
    from_results: dict[str, SaturationRow] = {}
    for line in text.splitlines():
        payload = _json_after(line, DETAIL_MARKER)
        if isinstance(payload, dict) and payload.get("stage") == DETAIL_ROWS_STAGE:
            for item in payload.get("items", []):
                row = _row_from_detail(item)
                if row is not None:
                    from_details.setdefault(row.name, row)
            continue
        payload = _json_after(line, RESULT_MARKER)
        if isinstance(payload, dict) and isinstance(payload.get("report"), dict):
            for row in _rows_from_result(payload["report"]):
                from_results.setdefault(row.name, row)
    merged = dict(from_results)
    merged.update(from_details)
    return _in_run_order(merged, from_details, from_results)


def _in_run_order(
    merged: dict[str, SaturationRow], from_details: dict[str, SaturationRow], from_results: dict[str, SaturationRow]
) -> dict[str, SaturationRow]:
    """Порядок прогона: как в подробностях, а при их отсутствии — как в таблице результата."""
    order = list(from_details) or list(from_results)
    return {name: merged[name] for name in order if name in merged}


def _json_after(line: str, marker: str) -> object:
    index = line.find(marker)
    if index < 0:
        return None
    try:
        return json.loads(line[index + len(marker) :])
    except json.JSONDecodeError:
        return None


def _row_from_detail(item: object) -> SaturationRow | None:
    if not isinstance(item, dict) or not isinstance(item.get("name"), str):
        return None
    outcome = item.get("outcome") if isinstance(item.get("outcome"), dict) else {}
    measurement = outcome.get("measurement") if isinstance(outcome.get("measurement"), dict) else {}
    cost = measurement.get("frame_cost_ms")
    digest = item.get("digest") if isinstance(item.get("digest"), dict) else {}
    parts = {
        part["label"]: part["digest"]
        for part in digest.get("parts", [])
        if isinstance(part, dict) and isinstance(part.get("label"), str) and isinstance(part.get("digest"), str)
    }
    computed = digest.get("state") == "computed" and isinstance(digest.get("combined"), str)
    return SaturationRow(
        name=item["name"],
        verdict=str(outcome.get("state", "unknown")),
        frame_cost_ms=float(cost) if _is_number(cost) else None,
        digest=digest["combined"] if computed else None,
        digest_parts=parts if computed else {},
        digest_unavailable_reason=digest.get("reason") if digest.get("state") == "unavailable" else None,
        frame_note=digest.get("frame_note") if computed else None,
    )


def _rows_from_result(report: dict) -> list[SaturationRow]:
    digests: dict[str, str | None] = {}
    for entry in report.get("digests", []) if isinstance(report.get("digests"), list) else []:
        name, _, value = str(entry).partition("|")
        digests[name] = value if DIGEST_HEX.match(value) else None
    rows: list[SaturationRow] = []
    for raw in report.get("table", []) if isinstance(report.get("table"), list) else []:
        parsed = _parse_row(raw)
        if parsed is None:
            continue
        name, _fps, cost, verdict = parsed
        try:
            frame_cost = float(cost)
        except ValueError:
            frame_cost = None
        rows.append(SaturationRow(name, verdict, frame_cost, digests.get(name), {}))
    return rows


def compare_runs(base: dict[str, SaturationRow], new: dict[str, SaturationRow]) -> list[RowComparison]:
    """Сравнение по работам: сначала в порядке прогона A, затем работы, которые есть только в B."""
    names = [*base, *(name for name in new if name not in base)]
    return [_compare_row(name, base.get(name), new.get(name)) for name in names]


def _compare_row(name: str, base: SaturationRow | None, new: SaturationRow | None) -> RowComparison:
    if base is None or new is None:
        side = "A" if base is None else "B"
        only = new if base is None else base
        return RowComparison(
            name,
            base.frame_cost_ms if base else None,
            new.frame_cost_ms if new else None,
            None,
            f"нет в {side}",
            f"работа есть только в {'B' if base is None else 'A'}" + (f" ({only.verdict})" if only else ""),
        )
    change = None
    if base.frame_cost_ms and new.frame_cost_ms is not None:
        change = (new.frame_cost_ms - base.frame_cost_ms) / base.frame_cost_ms * 100.0
    status, detail = _digest_status(base, new)
    return RowComparison(name, base.frame_cost_ms, new.frame_cost_ms, change, status, detail)


def _digest_status(base: SaturationRow, new: SaturationRow) -> tuple[str, str]:
    if base.digest is None or new.digest is None:
        missing = [
            f"{label}: {row.digest_unavailable_reason or 'дайджеста нет в прогоне'}"
            for label, row in (("A", base), ("B", new))
            if row.digest is None
        ]
        return DIGEST_UNAVAILABLE, "; ".join(missing)
    if base.digest == new.digest:
        return DIGEST_SAME, _frame_note_difference(base, new)
    changed = sorted(
        label
        for label in {*base.digest_parts, *new.digest_parts}
        if base.digest_parts.get(label) != new.digest_parts.get(label)
    )
    return DIGEST_DIFFERENT, ("части: " + ", ".join(changed)) if changed else ""


def _frame_note_difference(base: SaturationRow, new: SaturationRow) -> str:
    """Дайджесты равны, но читали кадры разного вида (со статистикой AWB и без): равенство слабее."""
    if base.frame_note and new.frame_note and base.frame_note != new.frame_note:
        return f"читали разные кадры: A {base.frame_note}, B {new.frame_note}"
    return ""


def format_comparison(comparisons: list[RowComparison]) -> str:
    """Таблица «работа | мс/кадр A | мс/кадр B | изменение | дайджест» для печати."""
    header = ("работа", "мс/кадр A", "мс/кадр B", "изменение", "дайджест")
    lines = [header]
    for row in comparisons:
        lines.append(
            (
                row.name,
                _ms(row.cost_a_ms),
                _ms(row.cost_b_ms),
                "-" if row.change_percent is None else f"{row.change_percent:+.1f}%",
                row.digest_status + (f" ({row.detail})" if row.detail else ""),
            )
        )
    widths = [max(len(line[column]) for line in lines) for column in range(len(header))]
    return "\n".join(
        "  ".join(cell.ljust(width) for cell, width in zip(line, widths, strict=True)).rstrip() for line in lines
    )


def _ms(value: float | None) -> str:
    return "-" if value is None else f"{value:.3g}" if value < 100 else f"{value:.0f}"


def compare_files(path_a: Path, path_b: Path) -> tuple[int, str]:
    """Читает два сохранённых прогона и возвращает (код выхода, текст): 0 — таблица напечатана, 2 — файл пуст."""
    runs = []
    for path in (path_a, path_b):
        rows = parse_saturation_rows(path.read_text(encoding="utf-8", errors="replace"))
        if not rows:
            return 2, f"{path}: нет строк FASTICAMERA_DETAIL (detail_rows) и FASTICAMERA_RESULT сценария gpu-saturation-bench"
        runs.append(rows)
    return 0, format_comparison(compare_runs(runs[0], runs[1]))


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Сравнение двух сохранённых прогонов gpu-saturation-bench")
    commands = parser.add_subparsers(dest="command", required=True)
    compare = commands.add_parser("compare", help="таблица мс/кадр A → B и дайджестов по работам")
    compare.add_argument("base", type=Path, help="сохранённый вывод device-smoke.sh: база (A)")
    compare.add_argument("new", type=Path, help="сохранённый вывод device-smoke.sh: новое (B)")
    arguments = parser.parse_args(argv)
    code, text = compare_files(arguments.base, arguments.new)
    print(text, file=sys.stdout if code == 0 else sys.stderr)
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
