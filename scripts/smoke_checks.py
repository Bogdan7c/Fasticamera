"""Ожидания smoke-сценариев сессии 02: lifecycle, permission, motion, platform.

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
