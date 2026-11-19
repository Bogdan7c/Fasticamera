"""Ожидания smoke-сценария ``screen`` (сессия 12): экран камеры iOS поверх видоискателя, касания делает скрипт.

Точные правила и пороги судит Rust (конфиг ``camera.scenarios.screen`` и ``camera.scenarios.viewfinder``,
``screen_report.rs``); здесь — независимая грубая перепроверка сводки строки результата: все шаги плана прошли,
счётчики экрана нулевые, слой UI в бюджете GPU (решение 90), видоискатель живой и ровный (решение 91). Каждая
проверка — чистая функция «JSON результата → список проблем» (пустой список — прошёл).

Состояние экрана (панель, плашка AE/AF LOCK, шаги EV, режим, затвор) Rust читает прямо из снимка отчёта экрана,
поэтому косвенных шагов (``steps_inferred``) обычно 0. Если они появятся (новый шаг без прямой проверки), это не
провал: число идёт заметкой (``screen_notes``), чтобы владелец на приёмке 12a знал, что смотреть глазами.
"""

from __future__ import annotations

from smoke_checks import _is_number, _report_problem
from smoke_checks_glass import _dict, _gpu_budget_problems, _late_frames_problems, _viewfinder_problems

EXPECTED_SOURCE = "camera_screen"
"""Имя источника UI экрана камеры (контракт с ``screen_report::EXPECTED_SOURCE``)."""

REQUIRED_STEPS = (
    "focus_tap_center",
    "lock_long_press",
    "unlock_tap",
    "menu_open_swipe",
    "ev_slider_swipe",
    "volume_up_shutter",
)
"""Шаги, без которых прогон не считается проверкой экрана (контракт с ``screen_steps.rs``)."""

ZERO_SCREEN_COUNTERS = (
    "frame_failures",
    "effects_overflowed",
    "unhandled_gestures",
    "input_overflowed",
    "input_receiver_gone",
    "input_dropped_without_screen",
    "executor_refusals",
    "results_with_unknown_origin",
)
"""Счётчики экрана, которые за окно обязаны остаться нулевыми (контракт с ``ScreenCounters``)."""

ZERO_UI_COUNTERS = ("frames_failed", "source_failures", "limit_overflows")
"""Счётчики слоя UI, которые за окно обязаны остаться нулевыми (как у ``glass``)."""


def check_screen(result: dict) -> list[str]:
    """``screen``: шаги плана прошли, экран без отказов, слой UI в бюджете, видоискатель жив."""
    problems, report = _report_problem(result)
    if problems:
        return problems
    window_ms = report.get("window_ms")
    if not _is_number(window_ms) or window_ms <= 0:
        problems.append(f"нет длины окна наблюдения (window_ms={window_ms!r})")
    problems.extend(_steps_problems(report))
    problems.extend(_screen_problems(_dict(report.get("screen"))))
    problems.extend(_overlay_problems(_dict(report.get("overlay"))))
    problems.extend(_gpu_budget_problems(_dict(report.get("overlay")), _dict(report.get("ui_gpu_budget"))))
    problems.extend(_late_frames_problems(report))
    problems.extend(_viewfinder_problems(_dict(report.get("viewfinder"))))
    validation = _dict(report.get("validation"))
    if validation.get("mode") == "enabled" and validation.get("errors") != 0:
        problems.append(f"ошибки validation: {validation.get('errors')!r}, ожидалось 0")
    return problems


def _steps_problems(report: dict) -> list[str]:
    """Каждый шаг плана выполнен и подтверждён следствиями; обязательные шаги на месте."""
    steps = report.get("steps")
    if not isinstance(steps, list) or not steps:
        return [f"в сводке нет списка шагов плана (steps={steps!r})"]
    problems: list[str] = []
    done = {step.get("name") for step in steps if isinstance(step, dict) and step.get("ok") is True}
    for step in steps:
        if not isinstance(step, dict) or step.get("ok") is not True:
            name = step.get("name") if isinstance(step, dict) else step
            problems.append(f"шаг {name!r} не подтверждён на экране и в камере")
    for required in REQUIRED_STEPS:
        if required not in done:
            problems.append(f"обязательный шаг {required!r} не выполнен")
    unfinished = report.get("unfinished")
    if unfinished != []:
        problems.append(f"не выполнены шаги плана: {unfinished!r}")
    if report.get("plan_failure") is not None:
        problems.append(f"план провален: {report.get('plan_failure')!r}")
    return problems


def _screen_problems(screen: dict) -> list[str]:
    """Счётчики экрана за окно: кадры рисовались, отказов и потерь ввода нет."""
    if not screen:
        return ["в сводке нет раздела screen (счётчики экрана)"]
    problems: list[str] = []
    frames = screen.get("frames_produced")
    if not _is_number(frames) or frames <= 0:
        problems.append(f"экран не нарисовал ни одного кадра (screen.frames_produced={frames!r})")
    for name in ZERO_SCREEN_COUNTERS:
        if screen.get(name) != 0:
            problems.append(f"screen.{name}={screen.get(name)!r}, ожидалось ровно 0")
    return problems


def _overlay_problems(overlay: dict) -> list[str]:
    """Источник UI — экран камеры, кадры рисовались без отказов."""
    if not overlay:
        return ["в сводке нет раздела overlay (счётчики слоя UI)"]
    problems: list[str] = []
    source = overlay.get("active_source")
    if source != EXPECTED_SOURCE:
        problems.append(f"источник UI в рендере {source!r}, ожидался {EXPECTED_SOURCE!r}")
    drawn = overlay.get("frames_drawn")
    if not _is_number(drawn) or drawn <= 0:
        problems.append(f"слой UI не нарисовал ни одного кадра (overlay.frames_drawn={drawn!r})")
    for name in ZERO_UI_COUNTERS:
        value = overlay.get(name)
        if value != 0:
            problems.append(f"overlay.{name}={value!r}, ожидалось ровно 0 (последняя ошибка: {overlay.get('last_error')!r})")
    return problems


EV_PROOF_ENGINE_DESIRED = "engine_desired_value"
"""Значение ``ev_proof`` сводки: EV доказан до желаемого состояния движка (контракт с ``screen_report.rs``)."""


def screen_notes(result: dict) -> list[str]:
    """Пояснения к прошедшему ``screen`` (не провал): косвенные шаги и что именно доказывают шаги EV."""
    report = _dict(result.get("report"))
    notes: list[str] = []
    inferred = report.get("steps_inferred")
    if _is_number(inferred) and inferred > 0:
        notes.append(
            f"{int(inferred)} шагов доказаны косвенно (по следствиям жеста, а не по снимку состояния экрана): "
            "эти шаги на приёмке 12a смотрите глазами"
        )
    if report.get("ev_proof") == EV_PROOF_ENGINE_DESIRED:
        notes.append(
            "шаги EV доказывают, что желаемое EV дошло до движка камеры; применил ли его HAL, статус не сообщает: "
            "смотрите яркость кадра глазами на приёмке 12a"
        )
    return notes
