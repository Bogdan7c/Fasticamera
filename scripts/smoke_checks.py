"""Ожидания smoke-сценариев сессий 02, 03 и 04: lifecycle, permission, motion, platform,
render, render-lifecycle, ahb-import, gpu-fence, camera-catalog, camera-stream, camera-evict.

Каждая проверка — чистая функция «JSON результата → список проблем» (пустой список — прошёл).
Контракт полей отчёта фиксирован планом сессии 02 и реализуется в Rust
(``crates/fasticamera-app/src/scenario/``). Сюда же вынесены пороги — именованные константы.
"""

from __future__ import annotations

import re

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


# ---- Сессия 04: камера ----

HIDDEN_CAMERA_IDS = frozenset({"4", "5"})
"""Камеры OnePlus 11 с ``SYSTEM_CAMERA`` (телевик 4 и логическая 5): обычным приложениям закрыты, в
каталоге их быть не должно (инвариант 6 плана, факты 00 C1). Полную сверку делает ``smoke_dumpsys.py``,
здесь — дешёвая страховка, работающая и без ``adb``."""

REQUIRED_CATALOG_ROLES = ("wide", "ultra_wide", "front")
"""Роли, которые обязан найти движок на OnePlus 11 (телевика без ``SYSTEM_CAMERA`` нет)."""

ACCEPTED_EVICTION_REASONS = frozenset({"evicted", "in_use_by_another_client", "too_many_cameras_open"})
"""Причины недоступности, допустимые при запуске системной камеры (зеркало ``ACCEPTED_EVICTION_REASONS`` в Rust)."""

MIN_FRACTION_OF_TARGET_FPS = 0.9
"""Грубый независимый порог `camera-stream`: измеренная частота не ниже 90 % цели шага. Точные пороги
(57 из 60, 28 из 30) судит Rust по конфигу сценария; скрипт ловит только грубые расхождения."""

TARGET_RATE_PATTERN = re.compile(r"@(\d+)$")
"""Цель шага в подписи `2/raw10/16x9@60`: частота после `@`."""

CAPTURE_RESULTS_REQUIRED = "required"
CAPTURE_RESULTS_NOT_SENT = "not_sent_by_hal"
"""Режимы ожиданий результатов шага (зеркало `CaptureResultsExpectation` в Rust): `required` — обычный и в сводке
не пишется; `not_sent_by_hal` — логическая камера 0, HAL которой шлёт только частичные результаты."""

STREAMING_STATE = "streaming"
"""Имя состояния движка, в котором идёт поток."""


def _report_problem(result: dict) -> tuple[list[str], dict]:
    """Общий вход проверок камеры: сценарий обязан дойти до ``passed`` и принести объект ``report``.

    Провал сценария с отчётом (``failed`` + ``report``) разбирается отдельно: нарушения лежат в ``error``,
    их печатает сама строка результата, а здесь возвращается только статус. Проверки не гадают по
    частичному отчёту.
    """
    problems = _status_problem(result)
    if problems:
        return problems, {}
    report = result.get("report")
    if not isinstance(report, dict):
        return ["нет report в результате"], {}
    return [], report


def check_camera_catalog(result: dict) -> list[str]:
    """``camera-catalog``: видимые камеры с RAW-потоками, роли привязаны, скрытых камер нет, колбэки не паниковали.

    Строка результата вмещает 4000 байт, поэтому здесь краткая запись камеры (``id``, ``facing``, топология,
    роль, уровень, RAW-потоки, угол обзора); полные характеристики идут строками стадий ``detail_cameras``.
    """
    problems, report = _report_problem(result)
    if problems:
        return problems
    cameras = report.get("cameras")
    if not isinstance(cameras, list) or not cameras:
        return ["каталог пуст или отсутствует (report.cameras)"]
    ids = [str(camera.get("id")) for camera in cameras if isinstance(camera, dict)]
    for hidden in sorted(HIDDEN_CAMERA_IDS.intersection(ids)):
        problems.append(f"скрытая камера {hidden} (SYSTEM_CAMERA) попала в каталог: инвариант 6 нарушен")
    for camera in cameras:
        if not isinstance(camera, dict):
            problems.append(f"запись камеры не объект: {camera!r}")
            continue
        for field in ("id", "facing", "topology", "hardware_level", "raw_streams"):
            if camera.get(field) is None:
                problems.append(f"камера {camera.get('id')!r}: нет поля {field}")
        if not camera.get("raw_streams"):
            problems.append(f"камера {camera.get('id')!r}: нет ни одного RAW-потока")
    roles = {entry.get("role"): entry.get("camera") for entry in report.get("roles", []) if isinstance(entry, dict)}
    for role in REQUIRED_CATALOG_ROLES:
        if role not in roles:
            problems.append(f"роль {role} не привязана ни к одной камере (привязаны: {sorted(roles)})")
    backend = report.get("backend")
    if not isinstance(backend, dict):
        problems.append("нет сводки диагностики бэкенда (report.backend)")
    elif backend.get("callback_panics") != 0:
        problems.append(f"паники колбэков NDK: {backend.get('callback_panics')!r}, ожидалось 0")
    return problems


def _rate_floor_problems(label: str, source: str, measured: float, rate_match: re.Match[str] | None) -> list[str]:
    """Частота ``measured`` (оценка ``source``) не ниже доли цели шага; цель берётся из подписи ``@N``."""
    if rate_match is None:
        return [f"{label}: в подписи цели нет частоты (@N)"]
    if measured < MIN_FRACTION_OF_TARGET_FPS * int(rate_match.group(1)):
        return [f"{label}: частота {source} {measured:.2f} к/с меньше {MIN_FRACTION_OF_TARGET_FPS:.0%} цели"]
    return []


def _partial_only_rate_problems(label: str, step: dict, rate_match: re.Match[str] | None) -> list[str]:
    """Шаг без финальных результатов (логическая камера 0): частота по RAW-кадрам и сторож факта."""
    problems: list[str] = []
    raw_fps = step.get("raw_fps")
    if not _is_number(raw_fps):
        problems.append(f"{label}: частота по RAW-кадрам не измерена ({raw_fps!r})")
    else:
        problems.extend(_rate_floor_problems(label, "по RAW-кадрам", raw_fps, rate_match))
    results = step.get("results_completed")
    if _is_number(results) and results != 0:
        problems.append(
            f"{label}: факт изменился (прошивка?) — пересмотреть ожидание и отчёт 04: "
            f"финальных результатов {results}, ожидалось 0"
        )
    return problems


def _stream_step_problems(index: int, step: object) -> list[str]:
    """Проверка одного шага ``camera-stream`` по его сводке (грубая перепроверка независимо от вердикта Rust)."""
    if not isinstance(step, dict):
        return [f"шаг {index + 1}: запись не объект"]
    label = f"шаг {index + 1} ({step.get('target')})"
    problems: list[str] = []
    if step.get("failure") is not None:
        problems.append(f"{label}: {step.get('failure')!r}")
    rate_match = TARGET_RATE_PATTERN.search(str(step.get("target")))
    if step.get("capture_results", CAPTURE_RESULTS_REQUIRED) == CAPTURE_RESULTS_NOT_SENT:
        problems.extend(_partial_only_rate_problems(label, step, rate_match))
    else:
        measured = step.get("measured_fps")
        if not _is_number(measured):
            problems.append(f"{label}: частота не измерена ({measured!r})")
        else:
            problems.extend(_rate_floor_problems(label, "по меткам сенсора", measured, rate_match))
    raw_sink = step.get("raw_sink") if isinstance(step.get("raw_sink"), dict) else {}
    acquired = raw_sink.get("frames_acquired")
    if not _is_number(acquired) or acquired <= 0:
        problems.append(f"{label}: RAW-приёмник не получил кадров ({acquired!r})")
    if step.get("state_at_end") != STREAMING_STATE:
        problems.append(f"{label}: к концу замера состояние {step.get('state_at_end')!r}, ожидалось {STREAMING_STATE!r}")
    return problems


def check_camera_stream(result: dict) -> list[str]:
    """``camera-stream``: каждый шаг дошёл до ``Streaming``, частота выше порога, рычаг тот, RAW-кадры идут.

    Подробные отчёты шагов (статистика целиком, журнал событий) идут строками стадий ``detail_steps``.
    """
    problems, report = _report_problem(result)
    if problems:
        return problems
    steps = report.get("steps")
    if not isinstance(steps, list) or not steps:
        return ["нет шагов в отчёте (report.steps)"]
    for index, step in enumerate(steps):
        problems.extend(_stream_step_problems(index, step))
    backend = report.get("backend")
    if not isinstance(backend, dict):
        problems.append("нет сводки диагностики бэкенда (report.backend)")
    elif backend.get("callback_panics") != 0:
        problems.append(f"паники колбэков NDK: {backend.get('callback_panics')!r}, ожидалось 0")
    return problems


def check_camera_evict(result: dict) -> list[str]:
    """``camera-evict``: вытеснение замечено с допустимой причиной, движок сам вернул поток."""
    problems, report = _report_problem(result)
    if problems:
        return problems
    unavailable = report.get("unavailable")
    reason = unavailable.get("reason") if isinstance(unavailable, dict) else None
    if reason not in ACCEPTED_EVICTION_REASONS:
        problems.append(f"причина недоступности {reason!r}, допустимо {sorted(ACCEPTED_EVICTION_REASONS)}")
    recovered = report.get("recovered")
    if not isinstance(recovered, dict):
        problems.append("нет события Recovered (report.recovered): самовосстановления не было")
    elif not _is_number(recovered.get("recovery_ms")):
        problems.append(f"нет времени восстановления ({recovered.get('recovery_ms')!r})")
    if report.get("final_state") != STREAMING_STATE:
        problems.append(f"final_state={report.get('final_state')!r}, ожидалось {STREAMING_STATE!r}")
    counters = report.get("counters") if isinstance(report.get("counters"), dict) else {}
    if not _is_number(counters.get("recoveries")) or counters.get("recoveries") < 1:
        problems.append(f"counters.recoveries={counters.get('recoveries')!r}, ожидалось не меньше 1")
    if report.get("failure") is not None:
        problems.append(f"сценарий записал отказ: {report.get('failure')!r}")
    return problems
