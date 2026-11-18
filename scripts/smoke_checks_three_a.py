"""Ожидания smoke-сценария ``3a`` (сессия 08): автоматика 3A и ручные параметры на живом видоискателе.

Точные правила и пороги судит Rust (конфиг ``camera.scenarios.three_a``, вердикт ``three_a_verdict.rs``); здесь —
независимая грубая перепроверка сводки строки результата: сценарий прошёл, и в отчёте есть числа, ради которых
его запускали (решение 62 в продукте: AF/AE живы; ручная выдержка в результате; EV удваивает энергию кадра;
блокировка держит; брекетинг снимает 4 кадра подряд). Допуски здесь вдвое-втрое грубее Rust. Справка, не провал:
``awb_vs_hal``, ``region_effect_diopters``, ``ev_unverifiable_at_limit``.
"""

from __future__ import annotations

from smoke_checks import _is_number, _report_problem

FOCUS_SETTLED = frozenset({"focused", "not_focused"})
"""Итоги центрального тапа, при которых HAL отработал тап (контракт с ``TapResult`` в ``three_a_report.rs``)."""

ROUGH_EXPOSURE_TOLERANCE = 0.05
"""Грубый допуск ручной выдержки/ISO в результате (Rust судит строже: 1 %; зонд 08 видел 0,4 %)."""

ROUGH_EV_TOLERANCE = 0.5
"""Грубый допуск отношения «выдержка × ISO» к 2^EV (Rust: 0,35)."""

MIN_EV_POINTS = 3
"""Точки EV: опорная (0), плюс и минус."""


def _dict(value: object) -> dict:
    return value if isinstance(value, dict) else {}


def _rel_gap(value: object, target: object) -> float | None:
    if not _is_number(value) or not _is_number(target) or target == 0:
        return None
    return abs(value - target) / abs(target)


def _live_problems(report: dict) -> list[str]:
    live = _dict(report.get("live"))
    problems = []
    if _dict(live.get("companion")).get("state") != "active":
        problems.append(f"вспомогательный поток 3A не активен: {live.get('companion')!r} (автоматика HAL мертва)")
    for key in ("af_state", "ae_state"):
        if live.get(key) is None:
            problems.append(f"в результатах HAL нет {key.upper()}")
    return problems


def _awb_problems(report: dict) -> list[str]:
    awb = _dict(report.get("awb"))
    problems = []
    if awb.get("ready") is not True:
        problems.append("AWB не готов: нет оценки света или холодный старт не кончился")
    hidden, limit = awb.get("hidden"), awb.get("max_hidden")
    if not _is_number(hidden) or not _is_number(limit):
        problems.append(f"нет числа скрытых кадров AWB (hidden={hidden!r}, max_hidden={limit!r})")
    elif hidden > limit:
        problems.append(f"холодный старт скрыл {hidden} кадров, предел {limit}")
    if awb.get("forced") != 0:
        problems.append(f"принудительных показов AWB: {awb.get('forced')!r}, ожидалось 0")
    if awb.get("awb_vs_hal") not in ("ok", "warn", "unknown"):
        problems.append(f"нет сравнения AWB с HAL (awb_vs_hal={awb.get('awb_vs_hal')!r})")
    return problems


def _focus_problems(report: dict) -> list[str]:
    focus = _dict(report.get("focus"))
    taps = focus.get("taps")
    if not isinstance(taps, list) or not taps:
        return [f"нет итогов тапов (focus.taps={taps!r})"]
    problems = []
    center = _dict(taps[0]).get("result")
    if center not in FOCUS_SETTLED:
        problems.append(f"тап в центр закончился {center!r}, ожидалось одно из {sorted(FOCUS_SETTLED)}")
    if any(_dict(tap).get("result") == "trigger_not_delivered" for tap in taps):
        problems.append("бэкенд отверг разовый запрос тапа (trigger_not_delivered)")
    if focus.get("results_without_request_context") != 0:
        problems.append(
            f"результатов без контекста запроса: {focus.get('results_without_request_context')!r}, ожидалось 0"
        )
    if focus.get("triggers_sent") != len(taps):
        problems.append(f"отправлено триггеров {focus.get('triggers_sent')!r}, тапов {len(taps)}")
    if "region_effect_diopters" not in focus:
        problems.append("в отчёте нет region_effect_diopters (вопрос о регионах фокуса)")
    return problems


def _manual_problems(report: dict) -> list[str]:
    manual = _dict(report.get("manual"))
    applied = _dict(manual.get("applied"))
    problems = []
    for key, requested in (("exposure_ns", "requested_exposure_ns"), ("iso", "requested_iso")):
        gap = _rel_gap(applied.get(key), manual.get(requested))
        if gap is None or gap > ROUGH_EXPOSURE_TOLERANCE:
            problems.append(f"ручное {key} в результате {applied.get(key)!r}, просили {manual.get(requested)!r}")
    if manual.get("adjustments_for_in_range") != 0:
        problems.append(f"ControlsAdjusted на значения в диапазоне: {manual.get('adjustments_for_in_range')!r}")
    long_shutter = _dict(manual.get("long_shutter"))
    if long_shutter.get("limited_event_seen") is not True:
        problems.append("слишком длинная выдержка не дала ControlsAdjusted(LimitedByFrameDuration)")
    budget, got = long_shutter.get("frame_budget_ns"), long_shutter.get("applied_ns")
    if not _is_number(budget) or not _is_number(got) or got > budget * (1 + ROUGH_EXPOSURE_TOLERANCE):
        problems.append(f"длинная выдержка в результате {got!r} нс при длительности кадра {budget!r} нс")
    return problems


def _ev_problems(report: dict) -> list[str]:
    ev = _dict(report.get("ev"))
    points = ev.get("points")
    if not isinstance(points, list) or len(points) < MIN_EV_POINTS:
        return [f"точек EV меньше {MIN_EV_POINTS} (ev.points={points!r})"]
    problems = []
    if not isinstance(ev.get("ev_unverifiable_at_limit"), bool):
        problems.append("нет признака ev_unverifiable_at_limit")
    for point in map(_dict, points):
        bias = point.get("bias_ev")
        if not point.get("verifiable") or not _is_number(bias) or bias == 0:
            continue
        gap = _rel_gap(point.get("ratio_to_baseline"), 2.0**bias)
        if gap is None or gap > ROUGH_EV_TOLERANCE:
            problems.append(f"EV {bias:+}: отношение {point.get('ratio_to_baseline')!r}, ожидалось {2.0**bias:.2f}")
    return problems


def _lock_problems(report: dict) -> list[str]:
    lock = _dict(report.get("lock"))
    problems = []
    if lock.get("locked") is not True:
        problems.append("AE_STATE не стал Locked после блокировки")
    elif lock.get("non_locked_after_lock") != 0:
        problems.append(f"под блокировкой результатов не Locked: {lock.get('non_locked_after_lock')!r}")
    if lock.get("back_to_continuous") is not True:
        problems.append("после сброса точки AF не вернулся в непрерывный режим")
    return problems


def _bracket_problems(report: dict) -> list[str]:
    bracket = _dict(report.get("bracket"))
    frames = bracket.get("frames")
    problems = []
    if bracket.get("captured") is not True:
        problems.append(f"серия не снята (captured={bracket.get('captured')!r}, failure={bracket.get('failure')!r})")
    if not isinstance(frames, list) or not frames:
        return [*problems, "в серии нет кадров"]
    indexes = [_dict(frame).get("index") for frame in frames]
    if indexes != list(range(len(frames))):
        problems.append(f"кадры серии не в порядке индексов: {indexes!r}")
    if bracket.get("frame_gaps") != 0:
        problems.append(f"номера кадров серии не подряд: пропущено {bracket.get('frame_gaps')!r}")
    skipped = bracket.get("skipped_delta")
    if not _is_number(skipped) or not 0 < skipped <= len(frames):
        problems.append(f"рендер пропустил кадров серии {skipped!r} из {len(frames)}")
    if not _is_number(bracket.get("presented_during")) or bracket["presented_during"] <= 0:
        problems.append("за время серии видоискатель не показал кадров")
    return problems


def check_three_a(result: dict) -> list[str]:
    """``3a``: прошёл; автоматика HAL жива, AWB готов, тап сходится, ручная экспозиция точна, EV удваивает
    энергию кадра, блокировка держит, серия снята подряд, автоматика возвращена."""
    problems, report = _report_problem(result)
    if problems:
        return problems
    for section in (
        _live_problems,
        _awb_problems,
        _focus_problems,
        _manual_problems,
        _ev_problems,
        _lock_problems,
        _bracket_problems,
    ):
        problems.extend(section(report))
    if report.get("restore_failure"):
        problems.append(f"автоматические параметры не возвращены: {report.get('restore_failure')}")
    if report.get("state_at_end") != "streaming":
        problems.append(f"к концу прогона состояние камеры {report.get('state_at_end')!r}, ожидалось 'streaming'")
    return problems
