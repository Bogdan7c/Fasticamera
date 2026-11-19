"""Ожидания smoke-сценария ``ae-trace`` (сессия 08c, шаги 5–6): ряд нашего AE по ступенькам света.

Точные правила судит Rust (конфиг ``camera.scenarios.ae_trace``, вердикт ``ae_trace_verdict.rs``). Здесь —
независимая грубая перепроверка структуры сводки в строке результата и человекочитаемая таблица, чтобы владелец
у телефона сразу увидел цифры. Таблица строится и для ПРОВАЛЬНОГО прогона (по ``report`` в строке результата):
именно тогда числа нужнее всего.

Сводка несёт ступеньки короткими строками-массивами в порядке ``step_cols`` (контракт с ``ae_trace_report.rs``).
Ряд (подробности ``detail_ae_trace``) проверке не нужен: он печатается скриптом отдельно.

Что считается проблемой здесь, сверх вердикта Rust:
* ни одной настоящей ступеньки — владелец не переключал лампу, приёмка не проведена (Rust такой прогон «пропускает»,
  потому что пропущенные подсказки в вердикт не идут, но принять AE по нему нельзя);
* поломка структуры отчёта: нет фазы, колонок, строки ступеньки неверной длины.
"""

from __future__ import annotations

import sys

from smoke_checks import _is_number, _status_problem

STEP_COLUMNS = [
    "cue",
    "kind",
    "step_ev",
    "onset_ms",
    "sensor_settle_ms",
    "screen_settle_ms",
    "overshoot_ev",
    "sensor_overshoot_ev",
    "max_abs_gain_ev",
    "gain_at_end_ev",
    "proposals",
    "generations",
]
"""Колонки строки ступеньки (контракт с ``STEP_COLUMNS`` в ``ae_trace_report.rs``)."""

PHASES = ("photo", "video", "static")
"""Фазы прогона по порядку."""

CUED_PHASES = ("photo", "video")
"""Фазы с подсказками владельцу."""

STEP_KINDS = frozenset({"measured", "no_step", "insufficient_data"})
FLICKER_READINGS = ("absent", "none", "mains_50hz", "mains_60hz")
"""Ответы детектора мерцания HAL (контракт с ``FlickerReading::name``)."""

ROUGH_MIN_STEP_EV = 0.4
"""Грубый нижний порог настоящей ступеньки (Rust: 0,5)."""

ROUGH_MAX_OVERSHOOT_EV = 0.3
"""Грубый верхний порог перерегулирования (Rust: 0,15 по умолчанию): вдвое мягче."""

ROUGH_GAIN_MARGIN_EV = 0.1
"""Запас над пределом подтяжки из отчёта (Rust: допуск 0,02)."""


def _dict(value: object) -> dict:
    return value if isinstance(value, dict) else {}


def _list(value: object) -> list:
    return value if isinstance(value, list) else []


def _row(columns: list[str], row: list) -> dict:
    """Строка ступеньки как словарь «колонка -> значение»."""
    return dict(zip(columns, row))


def _phase_by_name(report: dict) -> dict[str, dict]:
    return {phase.get("phase"): phase for phase in _list(report.get("phases")) if isinstance(phase, dict)}


def _structure_problems(report: dict) -> list[str]:
    """Форма сводки: фазы, колонки, строки ступенек."""
    problems = []
    if report.get("step_cols") != STEP_COLUMNS:
        problems.append(f"колонки ступенек {report.get('step_cols')!r}, ожидались {STEP_COLUMNS!r}")
        return problems
    names = [phase.get("phase") for phase in _list(report.get("phases")) if isinstance(phase, dict)]
    if names != list(PHASES):
        problems.append(f"фазы отчёта {names!r}, ожидались {list(PHASES)!r}")
        return problems
    phases = _phase_by_name(report)
    for name in CUED_PHASES:
        for index, row in enumerate(_list(phases[name].get("steps"))):
            if not isinstance(row, list) or len(row) != len(STEP_COLUMNS):
                problems.append(f"{name}: строка ступеньки {index + 1} не из {len(STEP_COLUMNS)} колонок: {row!r}")
            elif _row(STEP_COLUMNS, row)["kind"] not in STEP_KINDS:
                problems.append(f"{name}: строка ступеньки {index + 1}: неизвестный исход {row[1]!r}")
    if not _is_number(_dict(phases["static"].get("quiet")).get("analyzed_ms")):
        problems.append("static: нет сводки «дыхания» (quiet.analyzed_ms)")
    for name in PHASES:
        readings = _dict(_dict(phases[name].get("flicker")).get("readings"))
        missing = [key for key in FLICKER_READINGS if not _is_number(readings.get(key))]
        if missing:
            problems.append(f"{name}: в flicker.readings нет {missing}")
    return problems


def _measured_rows(report: dict) -> list[tuple[str, dict]]:
    """Настоящие ступеньки всех фаз с подсказками: (фаза, строка как словарь)."""
    phases = _phase_by_name(report)
    rows = []
    for name in CUED_PHASES:
        for row in _list(_dict(phases.get(name)).get("steps")):
            if isinstance(row, list) and len(row) == len(STEP_COLUMNS):
                entry = _row(STEP_COLUMNS, row)
                if entry["kind"] == "measured":
                    rows.append((name, entry))
    return rows


def _step_problems(report: dict) -> list[str]:
    """Грубая перепроверка настоящих ступенек и общих правил: подтяжка, перерегулирование, профили."""
    problems = []
    measured = _measured_rows(report)
    if report.get("real_steps") != len(measured):
        problems.append(f"real_steps={report.get('real_steps')!r}, а настоящих ступенек в строках {len(measured)}")
    if not measured:
        problems.append("ни одной настоящей ступеньки: владелец не переключал лампу, приёмка AE не проведена")
    for name, entry in measured:
        label = f"{name}, подсказка {entry['cue']}"
        if not _is_number(entry["step_ev"]) or abs(entry["step_ev"]) < ROUGH_MIN_STEP_EV:
            problems.append(f"{label}: ступенька {entry['step_ev']!r} EV меньше {ROUGH_MIN_STEP_EV} у настоящей")
        if _is_number(entry["overshoot_ev"]) and entry["overshoot_ev"] > ROUGH_MAX_OVERSHOOT_EV:
            problems.append(f"{label}: перерегулирование {entry['overshoot_ev']} EV выше {ROUGH_MAX_OVERSHOOT_EV}")
    limit, gain = report.get("gain_limit_ev"), report.get("max_abs_gain_ev")
    if _is_number(limit) and _is_number(gain) and gain > limit + ROUGH_GAIN_MARGIN_EV:
        problems.append(f"подтяжка ISP {gain} EV выше предела {limit} EV")
    for name, phase in _phase_by_name(report).items():
        if phase.get("profile_confirmed") is not True:
            problems.append(f"{name}: профиль {phase.get('profile')!r} не подтверждён движком")
    return problems


def _harness_problems(report: dict) -> list[str]:
    """Поломки стенда в отчёте: подсказки, возврат профиля, переполнение ряда."""
    problems = []
    failed = _dict(_dict(report.get("prompts")).get("failed"))
    if failed.get("count") != 0:
        problems.append(f"владельцу не показан сигнал на экране: {failed.get('first')!r}")
    if report.get("restore_failure"):
        problems.append(f"профиль «Фото» не возвращён: {report.get('restore_failure')}")
    if report.get("dropped_samples") != 0:
        problems.append(f"ряд переполнен: потеряно {report.get('dropped_samples')!r} отсчётов")
    return problems


def _number(value: object, digits: int = 2) -> str:
    """Число для таблицы; нет данных — прочерк."""
    if not _is_number(value):
        return "-"
    text = f"{value:.{digits}f}"
    # Нули убираем только в дробной части: «3100» не должно стать «31».
    return text.rstrip("0").rstrip(".") if "." in text else text


def _median(values: list[float]) -> float | None:
    ordered = sorted(values)
    if not ordered:
        return None
    middle = len(ordered) // 2
    return ordered[middle] if len(ordered) % 2 else (ordered[middle - 1] + ordered[middle]) / 2


def _flicker_cell(phase: dict) -> str:
    flicker = _dict(phase.get("flicker"))
    readings = _dict(flicker.get("readings"))
    counts = " ".join(f"{key}={readings.get(key, 0)}" for key in FLICKER_READINGS)
    share = flicker.get("multiple_share")
    return f"{counts}; кратных 10 мс: {_number(share)} из {flicker.get('long_frames', 0)} длинных; макс {_number(flicker.get('max_exposure_ms'))} мс"


def format_summary_table(report: dict) -> str:
    """Человекочитаемая таблица по сводке: фото и видео рядом, статика, мерцание."""
    lines = [
        "",
        "ae-trace: итоги",
        f"  настоящих ступенек: {report.get('real_steps')}; отсчётов ряда: {report.get('samples')}; "
        f"подсказок показано: {_dict(report.get('prompts')).get('queued')}",
        f"  подтяжка ISP: максимум {_number(report.get('max_abs_gain_ev'), 3)} EV при пределе {_number(report.get('gain_limit_ev'), 3)} EV",
        "",
        f"  {'фаза':<6} {'профиль':<7} {'настоящих':>9} {'пропущено':>9} {'экран, мс':>10} {'сенсор, мс':>10} "
        f"{'перерег., EV':>12} {'макс |gain|':>11} {'gain в конце':>12}",
    ]
    phases = _phase_by_name(report)
    for name in CUED_PHASES:
        phase = _dict(phases.get(name))
        rows = [_row(STEP_COLUMNS, row) for row in _list(phase.get("steps")) if isinstance(row, list) and len(row) == len(STEP_COLUMNS)]
        real = [row for row in rows if row["kind"] == "measured"]
        screen = _median([row["screen_settle_ms"] for row in real if _is_number(row["screen_settle_ms"])])
        sensor = _median([row["sensor_settle_ms"] for row in real if _is_number(row["sensor_settle_ms"])])
        overshoot = max((row["overshoot_ev"] for row in real if _is_number(row["overshoot_ev"])), default=None)
        gains = [row["gain_at_end_ev"] for row in real if _is_number(row["gain_at_end_ev"])]
        worst_end = max(gains, key=abs, default=None)
        lines.append(
            f"  {name:<6} {str(phase.get('profile', '-')):<7} {len(real):>9} {len(rows) - len(real):>9} "
            f"{_number(screen, 0):>10} {_number(sensor, 0):>10} {_number(overshoot, 3):>12} "
            f"{_number(phase.get('max_abs_gain_ev'), 3):>11} {_number(worst_end, 3):>12}"
        )
    quiet = _dict(_dict(phases.get("static")).get("quiet"))
    lines += [
        "",
        f"  статика ({_number(quiet.get('analyzed_ms'), 0)} мс): размах цели {_number(quiet.get('target_range_ev'), 3)} EV, "
        f"сенсора {_number(quiet.get('sensor_range_ev'), 3)} EV, экрана {_number(quiet.get('screen_range_ev'), 3)} EV; "
        f"предложений {_number(quiet.get('proposals'))} ({_number(quiet.get('proposals_per_s'))}/с), "
        f"перестроек запроса {_number(quiet.get('generations'))} ({_number(quiet.get('generations_per_s'))}/с)",
        "",
        "  мерцание (ответы детектора HAL по кадрам):",
    ]
    lines += [f"    {name:<6} {_flicker_cell(_dict(phases.get(name)))}" for name in PHASES]
    return "\n".join(lines)


def check_ae_trace(result: dict) -> list[str]:
    """``ae-trace``: прошёл; структура сводки цела; есть настоящие ступеньки; подтяжка, перерегулирование и
    профили в порядке; подсказки доставлены. Печатает таблицу итогов (при проблемах — в stderr)."""
    problems = _status_problem(result)
    report = result.get("report")
    if not isinstance(report, dict):
        return [*problems, "нет report в результате"]
    if problems and "phases" not in report:
        # Прогон оборван до разбора (нет камеры, нет диагностики AE): структура не нужна, причина — в статусе.
        return problems
    structure = _structure_problems(report)
    problems += structure
    if not structure:
        problems += _step_problems(report)
        problems += _harness_problems(report)
        print(format_summary_table(report), file=sys.stderr if problems else sys.stdout)
    return problems
