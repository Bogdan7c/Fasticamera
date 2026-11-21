"""Ожидания smoke-сценария ``standby`` (13b): тёплая вторая камера на телефоне.

Точные правила и пороги судит Rust (конфиг ``scenarios.standby``, ``standby_verdict.rs``); здесь — независимая грубая
перепроверка сводки строки результата: два обмена ролей, оба исполнены повышением тёплой (``promoted_standby``), ни одного
отказа тёплой и отказа бэкенда в обмене, ``leaked_leases`` РОВНО ноль, кадры тёплой возвращались камере, обе задние камеры
держат не меньше 55 к/с, пока стримят обе, а после отпуска все тёплые закрыты и объектив вернулся «домой».
Подробности (окна замера, обмены) лежат в строке ``detail_standby`` и печатаются как ``FASTICAMERA_DETAIL``.

Каждая проверка — чистая функция «JSON результата → список проблем» (пустой список — прошёл).
"""

from __future__ import annotations

from smoke_checks import _is_number, _report_problem

EXPECTED_SWAPS = 2
"""Обменов за прогон: туда и обратно (контракт с ``standby_verdict::EXPECTED_SWAPS``)."""

PROMOTED_METHOD = "promoted_standby"
"""Метод смены объектива, который обязан быть у обоих обменов (``LensSwitchMethod::PromotedStandby``)."""

MIN_BACK_FPS = 55.0
"""Порог частоты каждой задней камеры, пока стримят обе (решение владельца; Rust читает его из конфига)."""


def check_standby(result: dict) -> list[str]:
    """``standby``: обмены повышением, без отказов и утечек, частота ≥ 55 к/с у обеих камер, одна камера в конце."""
    problems, report = _report_problem(result)
    if problems:
        return problems
    if report.get("failure") is not None:
        problems.append(f"прогон провален: {report.get('failure')!r}")
    problems.extend(_swap_problems(report))
    problems.extend(_counter_problems(report))
    problems.extend(_fps_problems(report))
    problems.extend(_end_problems(report))
    return problems


def _section(report: dict, name: str) -> dict:
    value = report.get(name)
    return value if isinstance(value, dict) else {}


def _swap_problems(report: dict) -> list[str]:
    swaps = report.get("swaps")
    if not isinstance(swaps, list) or len(swaps) < EXPECTED_SWAPS:
        return [f"обменов ролей {len(swaps) if isinstance(swaps, list) else swaps!r}, ожидалось {EXPECTED_SWAPS}"]
    problems: list[str] = []
    for swap in swaps:
        if not isinstance(swap, dict) or swap.get("method") != PROMOTED_METHOD:
            leg = swap.get("leg") if isinstance(swap, dict) else swap
            method = swap.get("method") if isinstance(swap, dict) else None
            problems.append(f"обмен {leg!r} исполнен как {method!r}, ожидалось {PROMOTED_METHOD!r}")
        elif not _is_number(swap.get("observed_ms")) or swap["observed_ms"] < 0:
            problems.append(f"время обмена {swap.get('leg')!r} не измерено: {swap.get('observed_ms')!r}")
    if not _is_number(report.get("time_to_ready_ms")):
        problems.append(f"время «просьба → тёплая готова» не измерено: {report.get('time_to_ready_ms')!r}")
    return problems


def _counter_problems(report: dict) -> list[str]:
    problems: list[str] = []
    stream = _section(report, "stream")
    engine = _section(report, "engine")
    backend = report.get("backend")
    if stream.get("leaked_leases") != 0:
        problems.append(f"утекло лизингов кадров: {stream.get('leaked_leases')!r}, ожидалось ровно 0")
    returned = stream.get("standby_frames_returned")
    if not _is_number(returned) or returned <= 0:
        problems.append(f"кадры тёплой не возвращались камере (standby_frames_returned={returned!r})")
    promotions = stream.get("standby_promotions")
    if not _is_number(promotions) or promotions < EXPECTED_SWAPS:
        problems.append(f"поток кадров провёл обменов {promotions!r}, ожидалось не меньше {EXPECTED_SWAPS}")
    if stream.get("standby_begins_refused") != 0:
        problems.append(f"поток отказал в тёплой эпохе: {stream.get('standby_begins_refused')!r} раз")
    if engine.get("standby_failures") != 0:
        problems.append(f"тёплая отказывала: {engine.get('standby_failures')!r} раз, ожидалось 0")
    if engine.get("promotion_fallbacks") != 0:
        problems.append(f"бэкенд отверг обмен: {engine.get('promotion_fallbacks')!r} раз, ожидалось 0")
    if engine.get("switches_failed") != 0:
        problems.append(f"неудачных смен объектива: {engine.get('switches_failed')!r}, ожидалось 0")
    promoted = engine.get("promotions")
    if not _is_number(promoted) or promoted < EXPECTED_SWAPS:
        problems.append(f"движок повысил тёплую {promoted!r} раз, ожидалось не меньше {EXPECTED_SWAPS}")
    offered = sum(
        slot.get("results_offered", 0)
        for slot in backend
        if isinstance(slot, dict) and _is_number(slot.get("results_offered"))
    ) if isinstance(backend, list) else 0
    if offered <= 0:
        problems.append("бэкенд не предложил потоку ни одного результата тёплой (backend[].results_offered)")
    return problems


def _fps_problems(report: dict) -> list[str]:
    windows = report.get("windows")
    if not isinstance(windows, list):
        return [f"в сводке нет окон замера частоты (windows={windows!r})"]
    both = [window for window in windows if isinstance(window, dict) and window.get("both") is True]
    if not both:
        return ["не замерена частота, пока стримят обе камеры"]
    problems: list[str] = []
    for window in both:
        for key, label in (("primary_fps", "показываемая"), ("standby_fps", "тёплая")):
            value = window.get(key)
            if not _is_number(value) or value < MIN_BACK_FPS:
                problems.append(f"окно {window.get('name')!r}: {label} камера {value!r} к/с, ниже порога {MIN_BACK_FPS:.0f}")
    return problems


def _end_problems(report: dict) -> list[str]:
    problems: list[str] = []
    if not _is_number(report.get("release_ms")):
        problems.append(f"тёплые не закрылись после «не нужно» (release_ms={report.get('release_ms')!r})")
    if report.get("stage") != "done":
        problems.append(f"прогон остановился на шаге {report.get('stage')!r}, а не дошёл до конца")
    final_role = report.get("final_role")
    if not isinstance(final_role, str) or not final_role:
        problems.append(f"нет итоговой роли объектива (final_role={final_role!r})")
    return problems


def standby_notes(result: dict) -> list[str]:
    """Заметки прошедшего ``standby`` (не провал): цифры, на которые смотрит владелец."""
    report = result.get("report")
    if not isinstance(report, dict):
        return []
    notes: list[str] = []
    ready = report.get("time_to_ready_ms")
    swaps = report.get("swaps")
    if _is_number(ready) and isinstance(swaps, list) and swaps:
        times = ", ".join(
            f"{swap.get('leg')}: {swap.get('observed_ms')} мс"
            for swap in swaps
            if isinstance(swap, dict) and _is_number(swap.get("observed_ms"))
        )
        notes.append(f"standby: тёплая готова через {ready} мс; обмены — {times}")
    minimum = report.get("min_fps_while_both_run")
    if _is_number(minimum):
        notes.append(f"standby: наименьшая частота, пока стримят обе камеры: {minimum} к/с (порог {MIN_BACK_FPS:.0f})")
    release = report.get("release_ms")
    if _is_number(release):
        notes.append(f"standby: тёплые закрылись через {release} мс после «не нужно»")
    return notes
