"""Ожидания smoke-сценария ``zoom`` (сессия 13): переключение объективов и зум на настоящем экране камеры.

Точные правила и пороги судит Rust (конфиг ``camera.scenarios.zoom``, ``zoom_verdict.rs``); здесь — независимая
грубая перепроверка сводки строки результата: все шаги плана выполнены, чёрных кадров там, где ждали картинку, РОВНО
ноль (решение 113), неудачных смен нет, все переходы раскрылись по условию готовности (раскрытие по сроку и честное чёрное — провал), времена переключения измерены, экран оставлен на 1× основной камеры.
Подробности по шагам (время «действие → смена объектива» и «действие → первый живой кадр», удержанные кадры, частота,
зеркало, вибрации колеса) лежат в строках ``detail_zoom`` и печатаются как ``FASTICAMERA_DETAIL``.

Каждая проверка — чистая функция «JSON результата → список проблем» (пустой список — прошёл).
"""

from __future__ import annotations

from smoke_checks import _is_number, _report_problem

EXPECTED_FINAL_FACING = "back"
"""Сценарий оставляет заднюю камеру (контракт с ``zoom_verdict::final_state_violations``)."""

EXPECTED_FINAL_ROLE = "wide"
"""…основную (роль ``wide``)."""

EXPECTED_FINAL_RATIO_MILLI = 1000
"""…на 1× (в тысячных)."""

FINAL_RATIO_TOLERANCE_MILLI = 20
"""Допуск на 1×: 2 % (как в Rust)."""

REVEAL_FAULT_FIELDS = (
    "revealed_by_timeout",
    "ended_without_stream",
    "ended_without_presentable_frame",
    "black_camera_unavailable",
    "black_picture_unavailable",
)
"""Плохие исходы держания переходного кадра (``RevealFaults`` в ``zoom_reveal.rs``): все обязаны быть нулями."""


def check_zoom(result: dict) -> list[str]:
    """``zoom``: шаги прошли, чёрных кадров 0, неудачных смен 0, времена измерены, экран оставлен на 1× основной камеры."""
    problems, report = _report_problem(result)
    if problems:
        return problems
    if report.get("failure") is not None:
        problems.append(f"прогон провален вне шагов: {report.get('failure')!r}")
    problems.extend(_steps_problems(report))
    problems.extend(_counter_problems(report))
    problems.extend(_timing_problems(report))
    problems.extend(_final_problems(report))
    return problems


def _steps_problems(report: dict) -> list[str]:
    planned = report.get("steps_planned")
    done = report.get("steps_done")
    if not _is_number(planned) or planned <= 0:
        return [f"в сводке нет числа шагов плана (steps_planned={planned!r})"]
    problems: list[str] = []
    if done != planned:
        problems.append(f"выполнено шагов {done!r} из {planned!r}")
    if report.get("unfinished") != 0:
        problems.append(f"не выполнено шагов: {report.get('unfinished')!r}")
    return problems


def _counter_problems(report: dict) -> list[str]:
    problems: list[str] = []
    black = report.get("black_frames_while_picture_expected")
    if black != 0:
        problems.append(f"чёрных кадров там, где ждали картинку: {black!r}, ожидалось ровно 0")
    if report.get("switches_failed") != 0:
        problems.append(f"неудачных смен объектива: {report.get('switches_failed')!r}, ожидалось 0")
    problems.extend(_reveal_problems(report))
    published = report.get("lens_intents_published")
    if not _is_number(published) or published <= 0:
        problems.append(f"экран не публиковал намерений объектива (lens_intents_published={published!r})")
    return problems


def _reveal_problems(report: dict) -> list[str]:
    """Каждая смена объектива раскрылась по условию готовности цвета и яркости (ядро заявки фичи, решение 113).

    Раскрытие по сроку (``max_hold_ms``) и держание, закончившееся честным чёрным, не дают чёрных кадров «там, где
    ждали картинку», поэтому счётчик чёрных их не ловит; сводка Rust несёт их отдельно в ``reveal_faults``.
    """
    faults = report.get("reveal_faults")
    if not isinstance(faults, dict):
        return [f"в сводке нет reveal_faults (значение {faults!r}): раскрытие переходов не проверено"]
    problems: list[str] = []
    for name in REVEAL_FAULT_FIELDS:
        value = faults.get(name)
        if value != 0:
            problems.append(f"переход не раскрылся по условию готовности: reveal_faults.{name}={value!r}, ожидалось 0")
    return problems


def _timing_problems(report: dict) -> list[str]:
    """Хотя бы одна смена объектива измерена обоими временами (иначе сценарий ничего не доказал)."""
    problems: list[str] = []
    observed = report.get("switches_observed")
    if not _is_number(observed) or observed <= 0:
        problems.append(f"ни одной смены объектива не наблюдалось (switches_observed={observed!r})")
        return problems
    for name in ("max_switch_ms", "max_first_live_frame_ms"):
        value = report.get(name)
        if not _is_number(value) or value < 0:
            problems.append(f"время {name} не измерено (значение {value!r})")
    return problems


def _final_problems(report: dict) -> list[str]:
    final = report.get("final")
    if not isinstance(final, dict):
        return [f"в сводке нет итогового состояния экрана (final={final!r})"]
    problems: list[str] = []
    ratio = final.get("ratio_milli")
    if not _is_number(ratio) or abs(ratio - EXPECTED_FINAL_RATIO_MILLI) > FINAL_RATIO_TOLERANCE_MILLI:
        problems.append(f"в конце экран на {ratio!r} тыс., ожидалось {EXPECTED_FINAL_RATIO_MILLI} (1×)")
    if final.get("facing") != EXPECTED_FINAL_FACING:
        problems.append(f"в конце направление {final.get('facing')!r}, ожидалось {EXPECTED_FINAL_FACING!r}")
    if final.get("role") != EXPECTED_FINAL_ROLE:
        problems.append(f"в конце объектив {final.get('role')!r}, ожидался {EXPECTED_FINAL_ROLE!r}")
    return problems


def zoom_notes(result: dict) -> list[str]:
    """Заметки прошедшего ``zoom`` (не провал): цифры, которые 13a сверит с порогами конфига."""
    report = result.get("report")
    if not isinstance(report, dict):
        return []
    notes: list[str] = []
    switch = report.get("max_switch_ms")
    live = report.get("max_first_live_frame_ms")
    if _is_number(switch) and _is_number(live):
        notes.append(
            f"zoom: самое долгое переключение — событие движка через {switch} мс, первый живой кадр через {live} мс; "
            "пороги в camera.scenarios.zoom (estimate), цифры телефона — основание их уточнить"
        )
    held = report.get("held_frames")
    if _is_number(held):
        notes.append(f"zoom: удержанных (размытых) кадров за прогон: {held}")
    return notes
