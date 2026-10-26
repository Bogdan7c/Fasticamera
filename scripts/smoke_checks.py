"""Ожидания smoke-сценариев сессий 02 и 03: lifecycle, permission, motion, platform,
render, render-lifecycle, ahb-import, gpu-fence.

Каждая проверка — чистая функция «JSON результата → список проблем» (пустой список — прошёл).
Контракт полей отчёта фиксирован планом сессии 02 и реализуется в Rust
(``crates/fasticamera-app/src/scenario/``). Сюда же вынесены пороги — именованные константы.
"""

from __future__ import annotations

MIN_GYRO_RATE_HZ = 380.0
"""Порог частоты гироскопа из плана: просили 400 Гц, меньше 380 Гц — сенсор не тянет требование."""

EXPECTED_PERMISSION_ANSWERS = ["denied", "denied_permanently", "denied_permanently"]
"""Три запроса подряд: первый отказ обычный, дальше Android не показывает диалог — отказ навсегда."""

LIFECYCLE_EVENTS = frozenset({"paused", "resumed", "surface_created", "surface_changed", "surface_destroyed"})
"""События шва, образующие цепочку жизненного цикла. Остальные (``touch*``, ориентация, термо…) не проверяем."""


def _status_problem(result: dict) -> list[str]:
    """Общая первая проверка: сценарий обязан дойти до ``passed``."""
    if result.get("status") == "passed":
        return []
    return [f"status={result.get('status')!r}, ожидался 'passed' ({result.get('error', '')})"]


def _is_number(value: object) -> bool:
    """Число, но не ``bool`` (в Python ``True`` — это int, а в отчёте такое значение ошибочно)."""
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def lifecycle_order_problems(events: list[str]) -> list[str]:
    """Проверяет порядок событий жизненного цикла при сворачивании и возврате.

    * Сворачивание: ``paused`` → ``surface_destroyed``.
    * Возврат: ``resumed`` и ``surface_created`` в любом порядке. Android его не гарантирует,
      и на OnePlus 11 ``resumed`` приходит первым (факт сессии 02). ``surface_changed`` — только
      после ``surface_created``.
    """
    chain = [name for name in events if name in LIFECYCLE_EVENTS]
    if not chain:
        return ["нет событий жизненного цикла (paused/surface_destroyed/…): приложение не сворачивалось?"]
    problems = []
    if chain[0] != "paused":
        problems.append(f"первое событие жизненного цикла {chain[0]!r}, ожидалось 'paused'")
    if "surface_destroyed" not in chain:
        problems.append("нет 'surface_destroyed': поверхность не уничтожалась при сворачивании")
        return problems
    destroyed_at = chain.index("surface_destroyed")
    before_destroyed = chain[:destroyed_at]
    if "surface_created" in before_destroyed:
        problems.append("'surface_created' раньше 'surface_destroyed'")
    if "resumed" in before_destroyed:
        problems.append("'resumed' раньше 'surface_destroyed': приложение не ушло в фон")
    returned = chain[destroyed_at + 1 :]
    if "surface_created" not in returned:
        problems.append("после 'surface_destroyed' нет 'surface_created': поверхность не пересоздана")
    elif "surface_changed" in returned[: returned.index("surface_created")]:
        problems.append("'surface_changed' раньше пересоздания поверхности ('surface_created')")
    if "resumed" not in returned:
        problems.append("после 'surface_destroyed' нет 'resumed': приложение не вернулось на передний план")
    return problems


def check_lifecycle(result: dict) -> list[str]:
    """``lifecycle``: свернули и вернули приложение — порядок событий верный, ничего не утекло."""
    problems = _status_problem(result)
    if problems:
        return problems
    report = result.get("report", {})
    events = report.get("events")
    if not isinstance(events, list):
        problems.append("нет списка report.events")
    else:
        problems.extend(lifecycle_order_problems(events))
    refs_at_ready = report.get("live_global_refs_at_ready")
    refs_at_end = report.get("live_global_refs_at_end")
    if not _is_number(refs_at_ready) or not _is_number(refs_at_end):
        problems.append("нет счётчиков глобальных JNI-ссылок (live_global_refs_at_ready/at_end)")
    elif refs_at_ready != refs_at_end:
        problems.append(f"утечка глобальных JNI-ссылок: было {refs_at_ready}, стало {refs_at_end}")
    if report.get("surface_release_timeouts") != 0:
        problems.append(f"surface_release_timeouts={report.get('surface_release_timeouts')!r}, ожидалось 0")
    if report.get("activity_recreated") is not False:
        problems.append(f"activity_recreated={report.get('activity_recreated')!r}, ожидалось false (onCreate не должен повторяться)")
    return problems


def check_permission(result: dict) -> list[str]:
    """``permission``: ответы на три запроса подряд — отказ, затем отказ навсегда дважды."""
    problems = _status_problem(result)
    if problems:
        return problems
    answers = result.get("report", {}).get("answers")
    if answers != EXPECTED_PERMISSION_ANSWERS:
        problems.append(f"ответы на запросы разрешения {answers!r}, ожидалось {EXPECTED_PERMISSION_ANSWERS!r}")
    return problems


def check_motion(result: dict) -> list[str]:
    """``motion``: гироскоп тянет заявленную частоту, акселерометр тоже присылает данные."""
    problems = _status_problem(result)
    if problems:
        return problems
    report = result.get("report", {})
    gyro = report.get("gyro", {})
    rate_hz = gyro.get("rate_hz")
    if not _is_number(rate_hz):
        problems.append("нет частоты гироскопа (report.gyro.rate_hz)")
    elif rate_hz < MIN_GYRO_RATE_HZ:
        problems.append(f"частота гироскопа {rate_hz} Гц ниже порога {MIN_GYRO_RATE_HZ:g} Гц")
    gyro_samples = gyro.get("samples")
    if not _is_number(gyro_samples) or gyro_samples <= 0:
        problems.append(f"нет отсчётов гироскопа (gyro.samples={gyro_samples!r})")
    accelerometer_samples = report.get("accelerometer_samples")
    if not _is_number(accelerometer_samples) or accelerometer_samples <= 0:
        problems.append(f"нет отсчётов акселерометра (accelerometer_samples={accelerometer_samples!r})")
    return problems


def check_platform(result: dict) -> list[str]:
    """``platform``: термо-слушатель, вибромотор с реализацией каждого паттерна и частота кадров."""
    problems = _status_problem(result)
    if problems:
        return problems
    report = result.get("report", {})
    thermal = report.get("thermal", {})
    if not thermal.get("status"):
        problems.append("пустой thermal.status")
    if thermal.get("listener") != "registered":
        problems.append(f"thermal.listener={thermal.get('listener')!r}, ожидалось 'registered'")
    haptics = report.get("haptics", {})
    if haptics.get("vibrator") != "present":
        problems.append(f"haptics.vibrator={haptics.get('vibrator')!r}, ожидалось 'present'")
    unsupported = [
        str(pattern.get("pattern"))
        for pattern in haptics.get("patterns", [])
        if pattern.get("realization") == "unsupported"
    ]
    if unsupported:
        problems.append(f"паттерны вибрации без реализации (unsupported): {', '.join(unsupported)}")
    frame_rate_result = report.get("frame_rate", {}).get("result")
    if frame_rate_result != "applied":
        problems.append(f"frame_rate.result={frame_rate_result!r}, ожидалось 'applied'")
    return problems


# ---------------------------------------------------------------------------------------------
# Сессия 03: Vulkan-ядро. Rust сам судит сценарии (``failed`` с перечнем нарушений); здесь те же
# правила перепроверяются по отчёту, чтобы расхождение отчёта и вердикта не прошло молча.
# ---------------------------------------------------------------------------------------------

VALIDATION_ENABLED = "enabled"
"""Значение ``validation.mode`` отчёта, когда слой Vulkan включён. В release он ``disabled``, и нули ничего не доказывают."""

REQUIRED_DISPLAY_FIELDS = ("format", "color_space", "width_px", "height_px", "transform", "image_count")
"""Параметры swapchain, которые отчёт ``render`` обязан содержать."""

AHB_PASSING_OUTCOMES = {"raw16": {"matched"}, "raw10": {"matched", "unsupported"}}
"""Допустимые исходы ``ahb-import``: RAW16 обязан совпасть, RAW10 может честно оказаться ``unsupported``."""


def _validation_problems(validation: object, label: str = "validation") -> list[str]:
    """Ошибки validation считаются, только если слой был включён."""
    if not isinstance(validation, dict):
        return [f"нет блока {label}"]
    mode = validation.get("mode")
    if mode not in (VALIDATION_ENABLED, "disabled"):
        return [f"{label}.mode={mode!r}, ожидалось 'enabled' или 'disabled'"]
    if mode == VALIDATION_ENABLED and validation.get("errors") != 0:
        return [f"ошибки validation: {validation.get('errors')!r}, ожидалось 0"]
    # Счётчики известных отходов — справка и провалом не считаются; но если поле есть, оно обязано
    # быть словарём VUID -> число (иначе отчёт испорчен).
    deviations = validation.get("known_deviations")
    if deviations is not None and not (
        isinstance(deviations, dict) and all(_is_number(count) and count >= 0 for count in deviations.values())
    ):
        return [f"{label}.known_deviations={deviations!r}, ожидался словарь VUID -> число"]
    return []


def _pacing_coverage_problems(pacing: dict) -> list[str]:
    """Полнота данных пейсинга: ``intervals / frames_covered`` не ниже порога из конфига.

    Доля пропусков по горстке интервалов (остальное — потерянные записи драйвера) ничего не
    доказывает; Rust уже судит это вердиктом ``SparseData``, здесь правило перепроверяется по
    самому отчёту.
    """
    intervals, frames_covered = pacing.get("intervals"), pacing.get("frames_covered")
    minimum = pacing.get("min_coverage_ratio")
    if not _is_number(minimum) or not 0 < minimum <= 1:
        return [f"нет pacing.min_coverage_ratio в (0, 1] (получено {minimum!r})"]
    if not (_is_number(intervals) and _is_number(frames_covered)) or frames_covered <= 0:
        return []  # отсутствие самих чисел уже отмечено выше
    coverage = intervals / frames_covered
    if coverage < minimum:
        return [f"данные пейсинга разрежены: интервалов {intervals} из {frames_covered} кадров ({coverage:.3f} < {minimum})"]
    return []


def check_render(result: dict) -> list[str]:
    """``render``: ритм, период экрана, параметры swapchain, кадры и чистая validation."""
    problems = _status_problem(result)
    if problems:
        return problems
    report = result.get("report", {})
    pacing = report.get("pacing", {})
    miss_ratio, max_miss_ratio = pacing.get("miss_ratio"), pacing.get("max_miss_ratio")
    if not _is_number(miss_ratio) or not _is_number(max_miss_ratio):
        problems.append("нет pacing.miss_ratio / pacing.max_miss_ratio")
    elif miss_ratio > max_miss_ratio:
        problems.append(f"доля пропусков {miss_ratio} выше допустимой {max_miss_ratio}")
    if not _is_number(pacing.get("intervals")) or pacing.get("intervals", 0) <= 0:
        problems.append(f"в окне пейсинга нет интервалов (pacing.intervals={pacing.get('intervals')!r})")
    if not isinstance(pacing.get("histogram"), list):
        problems.append("нет гистограммы интервалов (pacing.histogram)")
    # Дыры измерения (потерянные записи времён показа) — не пропуски, но число обязано быть в отчёте:
    # без него нельзя отличить «ровно» от «почти ничего не измерили».
    timing_gaps, frames_covered = pacing.get("timing_gaps"), pacing.get("frames_covered")
    if not _is_number(timing_gaps) or timing_gaps < 0:
        problems.append(f"нет pacing.timing_gaps (получено {timing_gaps!r})")
    if not _is_number(frames_covered) or frames_covered <= 0:
        problems.append(f"нет pacing.frames_covered (получено {frames_covered!r})")
    problems.extend(_pacing_coverage_problems(pacing))
    rejected = pacing.get("rejected_out_of_order")
    if not _is_number(rejected) or rejected < 0:
        problems.append(f"нет pacing.rejected_out_of_order (получено {rejected!r})")
    refresh = report.get("refresh", {})
    measured, expected, tolerance = (
        refresh.get("measured_period_ms"),
        refresh.get("expected_period_ms"),
        refresh.get("tolerance_ratio"),
    )
    if not (_is_number(measured) and _is_number(expected) and _is_number(tolerance)) or expected <= 0:
        problems.append("период обновления экрана неизвестен или отчёт неполон (refresh.*)")
    elif abs(measured - expected) / expected > tolerance:
        problems.append(f"период обновления {measured} мс вне допуска ±{tolerance:.0%} от ожидаемых {expected} мс")
    frames = report.get("frames_presented")
    if not _is_number(frames) or frames <= 0:
        problems.append(f"нет показанных кадров (frames_presented={frames!r})")
    if report.get("backend_errors") != 0:
        problems.append(f"ошибки бэкенда рендера: {report.get('backend_errors')!r}, ожидалось 0")
    display = report.get("display")
    if not isinstance(display, dict):
        problems.append("нет параметров swapchain (report.display)")
    else:
        missing = [name for name in REQUIRED_DISPLAY_FIELDS if display.get(name) in (None, "")]
        if missing:
            problems.append(f"в report.display не хватает полей: {', '.join(missing)}")
    problems.extend(_validation_problems(report.get("validation")))
    return problems


def _idle_census_problems(baseline: object, cycles: list) -> list[str]:
    """Перепись живых GPU-объектов В ПОКОЕ (по видам) в каждом цикле равна базовой.

    Число объектов при показе (``presenting_live_objects``) шумит и вердикта не имеет.
    """
    if not isinstance(baseline, dict):
        return ["нет idle_baseline_live_objects"]
    problems = []
    for cycle in cycles:
        census = cycle.get("idle_live_objects")
        if not isinstance(census, dict):
            problems.append(f"цикл {cycle.get('cycle')}: живые GPU-объекты в покое не замерены")
            continue
        changes = [
            f"{kind}: было {baseline.get(kind, 0)}, стало {census.get(kind, 0)}"
            for kind in sorted(set(baseline) | set(census))
            if baseline.get(kind, 0) != census.get(kind, 0)
        ]
        if changes:
            problems.append(f"в цикле {cycle.get('cycle')} живые GPU-объекты в покое изменились ({'; '.join(changes)})")
    return problems


def check_render_lifecycle(result: dict) -> list[str]:
    """``render-lifecycle``: каждый цикл вернул показ, освобождений ровно столько же, объекты GPU не растут."""
    problems = _status_problem(result)
    if problems:
        return problems
    report = result.get("report", {})
    planned = report.get("planned_cycles")
    cycles = report.get("cycles")
    if not _is_number(planned) or planned <= 0 or not isinstance(cycles, list):
        return ["нет planned_cycles / cycles в отчёте"]
    if len(cycles) != planned:
        problems.append(f"выполнено циклов {len(cycles)} из {planned}")
    if report.get("releases_confirmed") != planned:
        problems.append(f"подтверждённых освобождений {report.get('releases_confirmed')!r}, ожидалось {planned}")
    problems.extend(_idle_census_problems(report.get("idle_baseline_live_objects"), cycles))
    if report.get("surface_release_timeouts") != 0:
        problems.append(f"surface_release_timeouts={report.get('surface_release_timeouts')!r}, ожидалось 0")
    if report.get("activity_recreated") is not False:
        problems.append(f"activity_recreated={report.get('activity_recreated')!r}, ожидалось false")
    if report.get("fault") is not None:
        problems.append(f"поток рендера неисправен: {report.get('fault')!r}")
    if report.get("backend_errors") != 0:
        problems.append(f"ошибки бэкенда рендера: {report.get('backend_errors')!r}, ожидалось 0")
    if report.get("detach_failures") != 0:
        problems.append(f"detach_failures={report.get('detach_failures')!r}, ожидалось 0")
    if not isinstance(report.get("release_latency"), dict):
        problems.append("нет сводки задержки освобождения (release_latency)")
    problems.extend(_validation_problems(report.get("validation")))
    return problems


def check_ahb_import(result: dict) -> list[str]:
    """``ahb-import``: RAW16 совпал, RAW10 совпал или честно ``unsupported``; кэш и учёт объектов в порядке."""
    problems = _status_problem(result)
    if problems:
        return problems
    report = result.get("report", {})
    for format_name, allowed in AHB_PASSING_OUTCOMES.items():
        entry = report.get(format_name)
        if not isinstance(entry, dict):
            problems.append(f"нет отчёта по {format_name}")
            continue
        outcome = entry.get("outcome")
        if outcome == "matched_with_validation_errors":
            # Данные дошли верно, но validation нашёл ошибки сверх известных отходов: это провал,
            # и известные отходы печатаем, чтобы было видно, что уже учтено.
            summary = entry.get("summary") if isinstance(entry.get("summary"), dict) else {}
            problems.append(
                f"{format_name}: данные совпали, но ошибки validation: {summary.get('validation_errors')!r} "
                f"(известные отходы: {summary.get('known_deviations')!r})"
            )
            continue
        if outcome not in allowed:
            problems.append(f"{format_name}: outcome={outcome!r}, допустимо {sorted(allowed)} ({entry.get('details', '')})")
            continue
        summary = entry.get("summary")
        if outcome == "matched":
            if not isinstance(summary, dict):
                problems.append(f"{format_name}: нет summary при outcome=matched")
                continue
            for field in ("matched", "second_import_hit", "live_objects_restored"):
                if summary.get(field) is not True:
                    problems.append(f"{format_name}: summary.{field}={summary.get(field)!r}, ожидалось true")
            problems.extend(_validation_problems(
                {"mode": summary.get("validation"), "errors": summary.get("validation_errors")},
                f"{format_name}.validation",
            ))
    return problems


def check_gpu_fence(result: dict) -> list[str]:
    """``gpu-fence``: B увидела итог A, стартовала после конца A, fence был незавершённым при экспорте."""
    problems = _status_problem(result)
    if problems:
        return problems
    report = result.get("report", {})
    expected = report.get("expected_value")
    if report.get("work_a_value") != expected or report.get("work_b_value") != expected or expected is None:
        problems.append(
            f"итоги работ A/B ({report.get('work_a_value')!r}/{report.get('work_b_value')!r}) не равны ожидаемому {expected!r}"
        )
    if report.get("b_started_after_a_finished") is not True:
        problems.append(f"b_started_after_a_finished={report.get('b_started_after_a_finished')!r}, ожидалось true")
    if report.get("release_fence_pending_at_export") is not True:
        problems.append(
            f"release_fence_pending_at_export={report.get('release_fence_pending_at_export')!r}, ожидалось true"
        )
    if report.get("already_signaled_wait_semaphore_created") is not False:
        problems.append("фаза «уже сигнален»: создан лишний ожидающий семафор")
    if report.get("already_signaled_work_completed") is not True:
        problems.append("фаза «уже сигнален»: работа не завершилась")
    if report.get("already_signaled_result_value") != expected:
        problems.append(f"фаза «уже сигнален»: итог {report.get('already_signaled_result_value')!r}, ожидалось {expected!r}")
    problems.extend(_validation_problems({"mode": report.get("validation"), "errors": report.get("validation_errors")}))
    return problems
