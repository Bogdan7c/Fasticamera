"""Ожидания smoke-сценария ``glass`` (сессия 11): стекло поверх живого видоискателя.

Точные правила и пороги судит Rust (конфиг ``camera.scenarios.glass`` и ``camera.scenarios.viewfinder``,
``glass_report.rs``); здесь — независимая грубая перепроверка сводки строки результата по критерию 11a:
источник UI стоял и рисовал без отказов, пирамида размытия строилась, GPU-время кадра UI в бюджете
(ловушка грубых ошибок, решение 90), видоискатель живой, источник снят. Кадры «позже задуманного vsync» —
только цифра отчёта (решение 91): это задержка, а не рывок; рывки (кадр выпал) судит Rust порогами
видоискателя с допуском на дрейф часов. Каждая проверка — чистая функция
«JSON результата → список проблем» (пустой список — прошёл).

Гравитация (свет стекла от наклона) — не провал: её недоступность и молчание датчика идут заметкой
(``glass_notes``), потому что блик от наклона 11a смотрит глазами.
"""

from __future__ import annotations

from smoke_checks import _is_number, _report_problem

MAX_ISP_TO_SENSOR_GAP = 0.10
"""Грубый допуск «кадров через ISP в секунду» к частоте сенсора (Rust судит строже, 3 %)."""

MIN_SHOWN_ONCE_RATIO = 0.95
"""Грубая нижняя граница доли кадров видоискателя, выданных рендеру ровно один раз."""

STREAMING_UI_COUNTERS = ("frames_failed", "source_failures", "limit_overflows")
"""Счётчики слоя UI, которые за окно обязаны остаться нулевыми (контракт с ``glass_report.rs``)."""

GRAVITY_STATES = frozenset({"active", "unavailable"})
"""Состояния гравитации в отчёте (контракт с ``GravityReport``)."""


def _dict(value: object) -> dict:
    return value if isinstance(value, dict) else {}


def check_glass(result: dict) -> list[str]:
    """``glass``: слой UI работал и уложился в бюджет, видоискатель жив, источник снят."""
    problems, report = _report_problem(result)
    if problems:
        return problems
    window_ms = report.get("window_ms")
    if not _is_number(window_ms) or window_ms <= 0:
        problems.append(f"нет длины окна наблюдения (window_ms={window_ms!r})")
    problems.extend(_overlay_problems(_dict(report.get("overlay"))))
    problems.extend(_gpu_budget_problems(_dict(report.get("overlay")), _dict(report.get("ui_gpu_budget"))))
    problems.extend(_late_frames_problems(report))
    removal = _dict(report.get("source_removal")).get("state")
    if removal != "delivered":
        problems.append(f"источник UI не снят (source_removal.state={removal!r}, ожидалось 'delivered')")
    problems.extend(_gravity_problems(report.get("gravity")))
    problems.extend(_viewfinder_problems(_dict(report.get("viewfinder"))))
    validation = _dict(report.get("validation"))
    if validation.get("mode") == "enabled" and validation.get("errors") != 0:
        problems.append(f"ошибки validation: {validation.get('errors')!r}, ожидалось 0")
    return problems


def _overlay_problems(overlay: dict) -> list[str]:
    """Источник стоял, кадры рисовались без отказов, пирамида размытия строилась."""
    if not overlay:
        return ["в сводке нет раздела overlay (счётчики слоя UI)"]
    problems: list[str] = []
    source = overlay.get("active_source")
    if not isinstance(source, str) or not source:
        problems.append(f"источник UI не установлен к концу окна (active_source={source!r})")
    drawn = overlay.get("frames_drawn")
    if not _is_number(drawn) or drawn <= 0:
        problems.append(f"слой UI не нарисовал ни одного кадра (frames_drawn={drawn!r})")
    for name in STREAMING_UI_COUNTERS:
        value = overlay.get(name)
        if value != 0:
            problems.append(f"overlay.{name}={value!r}, ожидалось ровно 0 (последняя ошибка: {overlay.get('last_error')!r})")
    rebuilds = overlay.get("pyramid_rebuilds")
    if not _is_number(rebuilds) or rebuilds <= 0:
        problems.append(f"пирамида размытия фона не строилась (pyramid_rebuilds={rebuilds!r}): стекло без фона")
    return problems


def _gpu_budget_problems(overlay: dict, budget: dict) -> list[str]:
    """GPU-время кадра UI замерено и p95 не больше бюджета (период кадра × доля из конфига)."""
    p95 = _dict(overlay.get("gpu_time")).get("p95_ms")
    limit = budget.get("budget_ms")
    if not _is_number(p95):
        return ["нет GPU-времени кадра UI (overlay.gpu_time.p95_ms)"]
    if not _is_number(limit) or limit <= 0:
        return [f"нет бюджета GPU-времени UI (ui_gpu_budget.budget_ms={limit!r})"]
    if p95 > limit:
        return [f"GPU-время кадра UI p95 = {p95} мс больше бюджета {limit} мс"]
    return []


def _late_frames_problems(report: dict) -> list[str]:
    """Кадры позже задуманного vsync обязаны быть посчитаны; само число не провал (решение 91)."""
    late = report.get("late_frames")
    if not _is_number(late) or late < 0:
        return [f"нет счёта кадров позже задуманного vsync (late_frames={late!r})"]
    return []


def _gravity_problems(gravity: object) -> list[str]:
    """Гравитация описана по контракту. Молчание датчика и отказ — не провал (см. ``glass_notes``)."""
    state = _dict(gravity).get("state")
    if state not in GRAVITY_STATES:
        return [f"gravity.state={state!r}, ожидалось одно из {sorted(GRAVITY_STATES)}"]
    if state == "active" and not _is_number(_dict(gravity).get("samples_in_window")):
        return ["gravity активна, но нет числа показаний (gravity.samples_in_window)"]
    return []


def _viewfinder_problems(viewfinder: dict) -> list[str]:
    """Видоискатель под стеклом жив: ISP успевает за сенсором, кадры показаны один раз, копий нет."""
    if not viewfinder:
        return ["в сводке нет раздела viewfinder"]
    problems: list[str] = []
    sensor_fps, isp_fps = viewfinder.get("sensor_fps"), viewfinder.get("isp_fps")
    if not _is_number(sensor_fps) or not _is_number(isp_fps) or sensor_fps <= 0:
        problems.append(f"нет частот сенсора/ISP (sensor_fps={sensor_fps!r}, isp_fps={isp_fps!r})")
    elif abs(isp_fps - sensor_fps) / sensor_fps > MAX_ISP_TO_SENSOR_GAP:
        problems.append(f"ISP {isp_fps} кадров/с при сенсоре {sensor_fps} к/с")
    presented = viewfinder.get("frames_presented")
    if not _is_number(presented) or presented <= 0:
        problems.append(f"видоискатель не показал кадров (frames_presented={presented!r})")
    if viewfinder.get("cpu_copies") != 0:
        problems.append(f"cpu_copies={viewfinder.get('cpu_copies')!r}, ожидалось ровно 0")
    ratio = viewfinder.get("shown_once_ratio")
    if not _is_number(ratio):
        problems.append(f"viewfinder.shown_once_ratio={ratio!r}, ожидалось число")
    elif ratio < MIN_SHOWN_ONCE_RATIO:
        problems.append(f"показано ровно один раз {ratio:.3f} кадров, меньше {MIN_SHOWN_ONCE_RATIO}")
    return problems


def glass_notes(result: dict) -> list[str]:
    """Пояснения к прошедшему ``glass`` (не провал): заметки Rust (гравитация) из сводки, как есть."""
    notes = _dict(result.get("report")).get("notes")
    if not isinstance(notes, list):
        return []
    return [note for note in notes if isinstance(note, str)]
