"""Ожидания smoke-сценария ``3a`` (сессия 08): автоматика 3A и ручные параметры на живом видоискателе.

Точные правила и пороги судит Rust (конфиг ``camera.scenarios.three_a``, вердикт ``three_a_verdict.rs``); здесь —
независимая грубая перепроверка сводки строки результата: сценарий прошёл, и в отчёте есть числа, ради которых
его запускали (решение 62 в продукте: AF/AE живы; ручная выдержка в результате; EV удваивает энергию кадра;
блокировка держит; брекетинг снимает 4 кадра подряд). Допуски здесь вдвое-втрое грубее Rust. Справка, не провал:
``awb_vs_hal``, ``region_effect_diopters``, ``ev_unverifiable_at_limit``.

Сессия 08a: экспозицию может вести наш AE (``live.ae_source == "own"``) или вендорский (по умолчанию в старых
отчётах). У нашего AE вендорский автомат выключен: ``AE_STATE`` — справка, маркеры «сошлось»/«заблокировано» —
состояние ``Settled``/``Holding`` нашего исполнителя, а секция ``own_ae`` доказывает, что ни один результат
повторяющегося запроса не пришёл с активным автоматом HAL.
"""

from __future__ import annotations

from smoke_checks import _is_number, _report_problem

FOCUS_SETTLED = frozenset({"focused", "not_focused"})
"""Итоги центрального тапа, при которых HAL отработал тап (контракт с ``TapResult`` в ``three_a_report.rs``)."""

ROUGH_EXPOSURE_TOLERANCE = 0.05
"""Грубый допуск ручной выдержки/ISO в результате (Rust судит строже: 1 %; зонд 08 видел 0,4 %)."""

ROUGH_EV_TOLERANCE = 0.5
"""Грубый допуск отношения «выдержка × ISO» к 2^EV (Rust: 0,35)."""

ROUGH_LOCK_TOLERANCE = 0.05
"""Грубый допуск постоянства «выдержка × ISO» под блокировкой нашего AE (Rust: 1 %)."""

MIN_EV_POINTS = 3
"""Точки EV: опорная (0), плюс и минус."""

STATIC_SCENE_RANGE_EV = 1.0
"""Размах замера сцены под блокировкой меньше этого — сцену в окне удержания не меняли (объектив не закрывали).
Это НЕ провал (автоматические прогоны идут без человека), но в вердикте об этом сказано явно."""


def _dict(value: object) -> dict:
    return value if isinstance(value, dict) else {}


def _rel_gap(value: object, target: object) -> float | None:
    if not _is_number(value) or not _is_number(target) or target == 0:
        return None
    return abs(value - target) / abs(target)


def _ae_source_problems(report: dict) -> list[str]:
    """Движок должен вести тот AE, который поставила композиция (``expected_ae_source``)."""
    expected = report.get("expected_ae_source")
    actual = _dict(report.get("live")).get("ae_source", "vendor")
    if expected is None or expected == actual:
        return []
    reason = report.get("ae_fallback_reason")
    suffix = f" (замена: {reason})" if reason else ""
    return [f"экспозицию ведёт {actual!r}, а композиция поставила {expected!r}{suffix}"]


def _is_own_ae(report: dict) -> bool:
    """Экспозицию вёл наш AE (отчёты до 08a поля не имеют — это вендорский)."""
    return _dict(report.get("live")).get("ae_source") == "own"


def _live_problems(report: dict) -> list[str]:
    live = _dict(report.get("live"))
    problems = []
    if _dict(live.get("companion")).get("state") != "active":
        problems.append(f"вспомогательный поток 3A не активен: {live.get('companion')!r} (автоматика HAL мертва)")
    # У нашего AE вендорский автомат выключен (AE_MODE_OFF): AE_STATE там справка, а не правило.
    required = ("af_state",) if _is_own_ae(report) else ("af_state", "ae_state")
    for key in required:
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
    own = _is_own_ae(report)
    shifted = [_dict(point) for point in points if _is_number(_dict(point).get("bias_ev")) and point["bias_ev"] != 0]
    if own and shifted and all(point.get("ae_limit_reached") is True for point in shifted):
        problems.append("EV не проверен: наш AE во всех точках с компенсацией упёрся в предел камеры")
    for point in map(_dict, points):
        bias = point.get("bias_ev")
        # Наш AE: точка либо сошлась (Settled/Holding), либо честно упёрлась в предел (LimitReached).
        if own and not point.get("ae_converged") and not point.get("ae_limit_reached"):
            problems.append(f"EV {bias:+}: наш AE не сошёлся (ни Settled/Holding, ни LimitReached)")
        if not point.get("verifiable") or not _is_number(bias) or bias == 0:
            continue
        gap = _rel_gap(point.get("ratio_to_baseline"), 2.0**bias)
        if gap is None or gap > ROUGH_EV_TOLERANCE:
            problems.append(f"EV {bias:+}: отношение {point.get('ratio_to_baseline')!r}, ожидалось {2.0**bias:.2f}")
    return problems


def _lock_problems(report: dict) -> list[str]:
    lock = _dict(report.get("lock"))
    problems = []
    if _is_own_ae(report):
        if lock.get("locked") is not True:
            problems.append("наш AE не дошёл до Holding (и не упёрся в предел) после блокировки")
        elif lock.get("non_locked_after_lock") != 0:
            problems.append(f"под блокировкой наш AE вышел из Holding на тиках: {lock.get('non_locked_after_lock')!r}")
        if _is_number(lock.get("max_product_deviation")) and lock["max_product_deviation"] > ROUGH_LOCK_TOLERANCE:
            problems.append(f"под блокировкой «выдержка × ISO» плавала на {lock['max_product_deviation']:.3f}")
    elif lock.get("locked") is not True:
        problems.append("AE_STATE не стал Locked после блокировки")
    elif lock.get("non_locked_after_lock") != 0:
        problems.append(f"под блокировкой результатов не Locked: {lock.get('non_locked_after_lock')!r}")
    if lock.get("back_to_continuous") is not True:
        problems.append("после сброса точки AF не вернулся в непрерывный режим")
    if _is_own_ae(report) and lock.get("locked") is True:
        problems.extend(_lock_evidence_problems(lock))
    return problems


def _lock_evidence_problems(lock: dict) -> list[str]:
    """Свидетельства окна удержания нашего AE (08c): поля есть и правильного типа. Значения не судим."""
    problems = []
    for key in ("observed_scene_range_ev", "product_frozen_after_frames", "haptic_hold_start", "haptic_hold_end"):
        if key not in lock:
            problems.append(f"в отчёте блокировки нет поля {key}")
    scene_range = lock.get("observed_scene_range_ev")
    if scene_range is not None and not (_is_number(scene_range) and scene_range >= 0):
        problems.append(f"observed_scene_range_ev {scene_range!r}: ожидалось неотрицательное число или null")
    frozen = lock.get("product_frozen_after_frames")
    if frozen is not None and not (_is_number(frozen) and float(frozen).is_integer() and frozen >= 0):
        problems.append(f"product_frozen_after_frames {frozen!r}: ожидалось неотрицательное целое или null")
    for key in ("haptic_hold_start", "haptic_hold_end"):
        cue = lock.get(key)
        if key in lock and not (isinstance(cue, dict) and isinstance(cue.get("outcome"), str)):
            problems.append(f"{key} {cue!r}: ожидался итог вибрации с полем outcome")
    return problems


def _own_ae_problems(report: dict) -> list[str]:
    """Секция ``own_ae``: при нашем AE — ни одного результата с вендорским AE и признак работы AE."""
    own_ae = report.get("own_ae")
    if not _is_own_ae(report):
        return [] if own_ae is None else [f"own_ae есть, а экспозицию вёл вендорский AE: {own_ae!r}"]
    section = _dict(own_ae)
    problems = []
    if section.get("vendor_ae_results") != 0:
        problems.append(
            f"наш AE: результатов с активным AE_STATE вендора {section.get('vendor_ae_results')!r} "
            f"(первый кадр {section.get('first_vendor_ae_frame')!r}), ожидалось 0"
        )
    problems.extend(_results_checked_problems(section.get("results_checked")))
    problems.extend(_engine_count_problems(section.get("engine")))
    if not any(section.get(key) is True for key in ("settled_seen", "holding_seen", "limit_reached_seen")):
        problems.append(f"наш AE ни разу не дошёл до Settled/Holding/LimitReached (в конце: {section.get('progress_at_end')!r})")
    return problems


def _results_checked_problems(checked: object) -> list[str]:
    """Выборка сценария по происхождению результатов. Сценарий видит лишь последний результат на тик (~каждый
    третий кадр), поэтому тапа и серии в выборке может не оказаться — это не ошибка: полный учёт каждого
    результата даёт движок (``own_ae.engine``). Обязателен только повторяющийся запрос."""
    if not isinstance(checked, dict):
        return [f"наш AE: нет results_checked по происхождению результатов ({checked!r})"]
    count = checked.get("repeating")
    if not _is_number(count) or count <= 0:
        return [f"наш AE: не проверено ни одного результата повторяющегося запроса на вендорский AE (repeating={count!r})"]
    return []


def _engine_count_problems(engine: object) -> list[str]:
    """Учёт движка по каждому результату за прогон: ни одного с активным AE HAL, и результаты вообще были."""
    if not isinstance(engine, dict):
        return [f"наш AE: нет учёта движка own_ae.engine ({engine!r}) — «каждый результат» не проверен"]
    observed = engine.get("results_observed")
    active = engine.get("results_with_hal_ae_active")
    if not _is_number(observed) or observed <= 0:
        return [f"наш AE: движок не насчитал результатов за прогон (results_observed={observed!r}, счёт с запуска движка)"]
    if active != 0:
        return [f"наш AE: движок с запуска насчитал {active!r} результатов с активным AE HAL из {observed!r}, ожидалось 0"]
    return []


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
        _ae_source_problems,
        _live_problems,
        _awb_problems,
        _focus_problems,
        _manual_problems,
        _ev_problems,
        _lock_problems,
        _own_ae_problems,
        _bracket_problems,
    ):
        problems.extend(section(report))
    if report.get("restore_failure"):
        problems.append(f"автоматические параметры не возвращены: {report.get('restore_failure')}")
    if report.get("state_at_end") != "streaming":
        problems.append(f"к концу прогона состояние камеры {report.get('state_at_end')!r}, ожидалось 'streaming'")
    return problems


def three_a_notes(result: dict) -> list[str]:
    """Пояснения к вердикту ``3a``: не провал, но читать обязательно. Вызывается после успешной проверки.

    Главное: блокировка AE проверена на неподвижной сцене или нет. В автоматических прогонах руки над объективом
    нет, и ``max_product_deviation = 0`` тогда ничего не доказывает — об этом вердикт говорит прямо.
    """
    _, report = _report_problem(result)
    lock = _dict(report.get("lock"))
    if not _is_own_ae(report) or lock.get("locked") is not True:
        return []
    notes = []
    scene_range = lock.get("observed_scene_range_ev")
    if not _is_number(scene_range):
        notes.append("3a: размах замера сцены в окне блокировки не получен, смену сцены под блокировкой проверить нечем")
    elif scene_range < STATIC_SCENE_RANGE_EV:
        notes.append(
            f"3a: блокировка проверена на СТАТИЧНОЙ сцене (размах замера {scene_range:.2f} EV < "
            f"{STATIC_SCENE_RANGE_EV:g}): объектив в окне удержания не закрывали, отсутствие дрейфа "
            "выдержки × ISO под блокировкой здесь ничего не доказывает"
        )
    else:
        notes.append(f"3a: сцена в окне блокировки менялась (размах замера {scene_range:.2f} EV), дрейфа нет")
    frozen = lock.get("product_frozen_after_frames")
    if _is_number(frozen):
        notes.append(f"3a: выдержка × ISO замерла через {frozen:g} кадров после команды блокировки")
    else:
        notes.append("3a: выдержка × ISO не замерла до конца окна удержания (product_frozen_after_frames = null)")
    for key, moment in (("haptic_hold_start", "начала"), ("haptic_hold_end", "конца")):
        cue = _dict(lock.get(key))
        if cue.get("outcome") != "queued":
            notes.append(f"3a: вибрация {moment} окна не сыграна ({cue.get('outcome')!r}): владелец мог не заметить сигнал")
    return notes
