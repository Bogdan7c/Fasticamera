"""Ожидания smoke-сценариев видоискателя сессии 07: ``viewfinder``, ``viewfinder-latency``,
``viewfinder-stage-budget``, ``viewfinder-endurance``, ``video-headroom-bench``.

Точные правила и пороги судит Rust (конфиг ``camera.scenarios.viewfinder``); здесь — независимая грубая
перепроверка сводки строки результата: что сценарий прошёл и что в отчёте есть числа, ради которых его
запускали (таблица решения 45, вывод о троттлинге, время стенда). Каждая проверка — чистая функция
«JSON результата → список проблем» (пустой список — прошёл).
"""

from __future__ import annotations

from smoke_checks import _is_number, _report_problem

MAX_ISP_TO_SENSOR_GAP = 0.10
"""Грубый допуск «кадров через ISP в секунду» к частоте сенсора (Rust судит строже, 3 %)."""

COLOR_SPACES = frozenset({"display_p3", "srgb"})
"""Честные ответы о пространстве экрана; ``unknown`` — ISP не дал ни одного кадра."""

PROFILE_VERDICTS = frozenset({"tuned_better", "no_clear_difference", "tuned_worse", "mixed", "not_comparable"})
"""Выводы ``viewfinder-latency`` (контракт с ``viewfinder_latency_verdict.rs``): A/B-исход — данные, не провал."""

ADPF_SESSIONS = frozenset({"disabled", "pending", "active", "unsupported", "failed"})
"""Состояния сессии ADPF в отчёте (контракт с ``viewfinder_tuning_report.rs``)."""

PROFILE_COUNT = 2
"""Профилей в сравнении ``viewfinder-latency``: база (A) и проверяемый (B)."""

TABLE_COLUMNS = 4
"""Столбцы таблицы решения 45: стадия | мс GPU (медиана / p95) | Δ мА | вывод."""

STREAMING_STATE = "streaming"

VIEWFINDER_PACINGS = frozenset({"latest_wins", "jitter_buffer"})
"""Политики подачи кадров видоискателя в профиле (контракт с ``raw_frame_stream::ViewfinderPacing``, snake_case)."""

DEPTH_REDUCTIONS = frozenset({"disabled", "skip_one_vsync"})
"""Режимы схода на ближнюю линию кадра в профиле (контракт с ``render_loop::config::DepthReductionMode``)."""

MAX_JITTER_OVERFLOW_PER_MINUTE = 8.0
"""Грубый предел переполнений джиттер-буфера в минуту (Rust судит строже, 4: дрейф 60,03 против 60,00 Гц
даёт ~1,8 в минуту). Больше удвоенного допуска — буфер явно теряет кадры."""

MIN_SHOWN_ONCE_RATIO = 0.95
"""Грубая нижняя граница доли кадров, выданных рендеру ровно один раз (``delivered / frames_published``)."""

MIN_AWB_PUBLISH_FRACTION = 0.25
"""Грубая нижняя граница публикаций статистики AWB от ожидаемого при прореживании (обработанные кадры / N);
Rust судит строже (0,5): исполнитель бывает занят, окно режет период на краях."""

MAX_ACCOUNTING_GAP = 0.10
"""Грубый допуск «обработано − скрыто на холодном старте = показано» (Rust судит строже, 3 %)."""

MS_PER_MINUTE = 60_000.0


def _dict(value: object) -> dict:
    return value if isinstance(value, dict) else {}


def _streaming_problem(report: dict) -> list[str]:
    state = report.get("state_at_end")
    return [] if state == STREAMING_STATE else [f"к концу прогона состояние камеры {state!r}, ожидалось 'streaming'"]


def check_viewfinder(result: dict) -> list[str]:
    """``viewfinder``: ISP успевает за сенсором, ноль копий, время прохода, AWB, задержка, пространство экрана."""
    problems, report = _report_problem(result)
    if problems:
        return problems
    sensor_fps, isp_fps = report.get("sensor_fps"), report.get("isp_fps")
    if not _is_number(sensor_fps) or not _is_number(isp_fps) or sensor_fps <= 0:
        problems.append(f"нет частот сенсора/ISP (sensor_fps={sensor_fps!r}, isp_fps={isp_fps!r})")
    elif abs(isp_fps - sensor_fps) / sensor_fps > MAX_ISP_TO_SENSOR_GAP:
        problems.append(f"ISP {isp_fps} кадров/с при сенсоре {sensor_fps} к/с")
    if report.get("cpu_copies") != 0:
        problems.append(f"cpu_copies={report.get('cpu_copies')!r}, ожидалось ровно 0")
    isp_dispatch = _dict(_dict(report.get("gpu_time")).get("isp_dispatch"))
    if not _is_number(isp_dispatch.get("p95_ms")):
        problems.append("нет времени прохода ISP (gpu_time.isp_dispatch.p95_ms)")
    look = _dict(report.get("look"))
    if look.get("painter") != "isp":
        problems.append(f"рисовальщик {look.get('painter')!r}, ожидался 'isp'")
    if look.get("display_color_space") not in COLOR_SPACES:
        problems.append(f"пространство экрана {look.get('display_color_space')!r}, ожидалось одно из {sorted(COLOR_SPACES)}")
    problems.extend(_awb_problems(report))
    problems.extend(_accounting_problems(report))
    companion = _dict(report.get("three_a_companion")).get("state")
    if companion != "active":
        problems.append(f"вспомогательный поток 3A {companion!r}, ожидалось 'active' (иначе автоматика HAL мертва)")
    latency = _dict(report.get("latency"))
    if not _is_number(latency.get("samples")) or latency.get("samples", 0) <= 0:
        problems.append("задержка «сенсор → экран» не замерена")
    validation = _dict(report.get("validation"))
    if validation.get("mode") == "enabled" and validation.get("errors") != 0:
        problems.append(f"ошибки validation: {validation.get('errors')!r}, ожидалось 0")
    problems.extend(_pacing_problems(report))
    problems.extend(_adpf_problems(report))
    problems.extend(_streaming_problem(report))
    return problems


def _awb_problems(report: dict) -> list[str]:
    """AWB в сводке ``viewfinder``: статистика идёт с прореживанием (не на каждом кадре), оценки света есть,
    холодный старт кончился. Прореживание: ожидается ``frames_processed / statistics_every_n_frames`` публикаций."""
    awb = report.get("awb_published")
    if not _is_number(awb) or awb <= 0:
        return [f"статистика AWB не публиковалась (awb_published={awb!r})"]
    white_balance = report.get("white_balance")
    if not isinstance(white_balance, dict):
        return ["в сводке нет раздела white_balance (наш AWB)"]
    problems: list[str] = []
    period, processed = white_balance.get("statistics_every_n_frames"), report.get("frames_processed")
    if not _is_number(period) or period < 1 or not _is_number(processed):
        problems.append(f"нет периода статистики или числа кадров (every_n={period!r}, frames_processed={processed!r})")
    elif awb < processed / period * MIN_AWB_PUBLISH_FRACTION:
        problems.append(f"статистика AWB опубликована {awb} раз, при {processed} кадрах и периоде {period} ожидалось больше")
    if not _is_number(white_balance.get("estimates")) or white_balance["estimates"] <= 0:
        problems.append("AWB не получил ни одной оценки света")
    if white_balance.get("cold_start_pending") is not False:
        problems.append("холодный старт AWB не кончился")
    return problems


def _accounting_problems(report: dict) -> list[str]:
    """«Ровно один раз» с учётом скрытых на холодном старте кадров: обработано − скрыто ≈ показано."""
    processed, hidden = report.get("frames_processed"), report.get("frames_hidden_cold_start")
    presented, skipped = report.get("frames_presented"), report.get("bracket_frames_skipped")
    if not all(_is_number(value) for value in (processed, hidden, presented, skipped)):
        return [
            "нет счётчиков учёта кадров (frames_processed, frames_hidden_cold_start, frames_presented, "
            "bracket_frames_skipped)"
        ]
    if presented <= 0 or abs((processed - hidden) - presented) / presented > MAX_ACCOUNTING_GAP:
        return [f"обработано {processed} (скрыто {hidden}), показано {presented}: кадры теряются"]
    return []


def _pacing_problems(report: dict) -> list[str]:
    """Подача кадров в сводке ``viewfinder``: потери посчитаны раздельно, переполнений не больше дрейфа.

    Вытеснение (``superseded``) и переполнение джиттер-буфера (``jitter_overflow``) обязаны быть в отчёте
    отдельными числами, а ``dropped`` — их суммой: потери не прячутся. Строгий допуск судит Rust.
    """
    pacing = report.get("pacing")
    if not isinstance(pacing, dict):
        return ["в сводке нет раздела pacing (подача кадров видоискателя)"]
    problems: list[str] = []
    for name in ("frames_published", "delivered", "dropped", "superseded", "jitter_overflow", "repeats"):
        if not _is_number(pacing.get(name)) or pacing[name] < 0:
            problems.append(f"pacing.{name}={pacing.get(name)!r}, ожидалось неотрицательное число")
    if problems:
        return problems
    if pacing["dropped"] != pacing["superseded"] + pacing["jitter_overflow"]:
        problems.append(
            f"pacing.dropped={pacing['dropped']} не равно superseded {pacing['superseded']} "
            f"+ jitter_overflow {pacing['jitter_overflow']}: потери считаются неверно"
        )
    window_ms = report.get("window_ms")
    if _is_number(window_ms) and window_ms > 0:
        per_minute = pacing["jitter_overflow"] / (window_ms / MS_PER_MINUTE)
        if per_minute > MAX_JITTER_OVERFLOW_PER_MINUTE:
            problems.append(
                f"джиттер-буфер переполнялся {per_minute:.1f} раз в минуту (больше {MAX_JITTER_OVERFLOW_PER_MINUTE:g}): "
                "это не дрейф часов, а потеря кадров"
            )
    ratio = pacing.get("shown_once_ratio")
    if not _is_number(ratio):
        problems.append(f"pacing.shown_once_ratio={ratio!r}, ожидалось число")
    elif ratio < MIN_SHOWN_ONCE_RATIO:
        problems.append(f"показано ровно один раз {ratio:.3f} кадров, меньше {MIN_SHOWN_ONCE_RATIO}")
    return problems


def _adpf_problems(report: dict) -> list[str]:
    """ADPF в сводке ``viewfinder``: по умолчанию выключен; включён и ``failed`` — провал, ``unsupported`` — нет."""
    if "adpf" not in report:
        return []
    adpf = _dict(report.get("adpf"))
    session = adpf.get("session")
    if session not in ADPF_SESSIONS:
        return [f"состояние сессии ADPF {session!r}, ожидалось одно из {sorted(ADPF_SESSIONS)}"]
    if adpf.get("mode") == "enabled" and session == "failed":
        return [f"ADPF включён, но сессия не создана: {adpf.get('last_error')!r}"]
    return []


def check_viewfinder_latency(result: dict) -> list[str]:
    """``viewfinder-latency``: все окна ABBA закрыты, два профиля с числами, вывод о профилях есть."""
    problems, report = _report_problem(result)
    if problems:
        return problems
    windows, expected = report.get("windows"), report.get("expected_windows")
    if not _is_number(expected) or expected < 6 or windows != expected:
        problems.append(f"закрыто окон {windows!r} из {expected!r} (нужно не меньше трёх пар)")
    profiles = report.get("profiles")
    names = [_dict(profile).get("name") for profile in profiles] if isinstance(profiles, list) else []
    if len(names) != PROFILE_COUNT or len(set(names)) != PROFILE_COUNT:
        problems.append(f"профилей должно быть {PROFILE_COUNT} с разными именами (profiles={profiles!r})")
    problems.extend(_profile_knob_problems(profiles))
    comparison = _dict(report.get("comparison"))
    for role in ("baseline", "tuned"):
        summary = _dict(comparison.get(role))
        if not _is_number(summary.get("windows")) or summary["windows"] <= 0:
            problems.append(f"у профиля {role} нет окон в сводке (comparison.{role}.windows)")
        if not _is_number(summary.get("latency_p50_ms")):
            problems.append(f"у профиля {role} нет медианы задержки (comparison.{role}.latency_p50_ms)")
        for key in ("dropped_per_s", "repeats_per_s", "shown_once_ratio"):
            if not _is_number(summary.get(key)):
                problems.append(f"у профиля {role} нет показателя подачи кадров (comparison.{role}.{key})")
    verdict = _dict(comparison.get("verdict")).get("verdict")
    if verdict not in PROFILE_VERDICTS:
        problems.append(f"нет вывода о профилях (verdict={verdict!r})")
    return problems


def _profile_knob_problems(profiles: object) -> list[str]:
    """У каждого профиля A/B есть политика подачи кадров и режим схода на ближнюю линию (допустимые значения)."""
    if not isinstance(profiles, list):
        return []
    problems: list[str] = []
    for index, profile in enumerate(profiles):
        knobs = _dict(profile)
        pacing, depth = knobs.get("viewfinder_pacing"), knobs.get("depth_reduction")
        if pacing not in VIEWFINDER_PACINGS:
            problems.append(f"profiles[{index}].viewfinder_pacing={pacing!r}, ожидалось одно из {sorted(VIEWFINDER_PACINGS)}")
        if depth not in DEPTH_REDUCTIONS:
            problems.append(f"profiles[{index}].depth_reduction={depth!r}, ожидалось одно из {sorted(DEPTH_REDUCTIONS)}")
    return problems


def check_viewfinder_stage_budget(result: dict) -> list[str]:
    """``viewfinder-stage-budget``: таблица по строке на каждую запрошенную стадию, по 4 столбца."""
    problems, report = _report_problem(result)
    if problems:
        return problems
    table = report.get("table")
    requested = report.get("requested_rows")
    if not isinstance(table, list) or not isinstance(requested, list) or not requested:
        return [f"нет таблицы или списка строк (table={table!r}, requested_rows={requested!r})"]
    measured = [line for line in table if isinstance(line, str) and not line.startswith("camera_without_isp")]
    if len(measured) != len(requested):
        problems.append(f"строк таблицы {len(measured)}, запрошено {len(requested)}")
    for line in table:
        if not isinstance(line, str) or len(line.split(" | ")) != TABLE_COLUMNS:
            problems.append(f"строка таблицы не из {TABLE_COLUMNS} столбцов: {line!r}")
    if "whole_viewfinder" in requested and not any(
        isinstance(line, str) and line.startswith("camera_without_isp") for line in table
    ):
        problems.append("есть «весь видоискатель», но нет строки «камера без ISP»")
    return problems


def check_viewfinder_endurance(result: dict) -> list[str]:
    """``viewfinder-endurance``: прогон дошёл до конца, вывод о троттлинге записан (найден он или нет)."""
    problems, report = _report_problem(result)
    if problems:
        return problems
    throttling = _dict(report.get("throttling"))
    if not isinstance(throttling.get("found"), bool):
        problems.append("нет вывода о троттлинге (throttling.found)")
    elif throttling["found"] and not throttling.get("signs"):
        problems.append("троттлинг найден, но признаки не записаны")
    if not _dict(report.get("endurance")).get("totals"):
        problems.append("нет итогов прогона (endurance.totals)")
    problems.extend(_streaming_problem(report))
    return problems


def check_video_headroom_bench(result: dict) -> list[str]:
    """``video-headroom-bench``: стенд записывал кадры и замерен; частота экрана записана честно (любая)."""
    problems, report = _report_problem(result)
    if problems:
        return problems
    bench = _dict(report.get("bench"))
    recorded = bench.get("frames_recorded")
    if not _is_number(recorded) or recorded <= 0:
        problems.append(f"стенд не записал кадров (frames_recorded={recorded!r})")
    if not _is_number(_dict(bench.get("total")).get("p95_ms")):
        problems.append("нет времени кадра стенда (bench.total.p95_ms)")
    if not isinstance(bench.get("passes"), list) or not bench["passes"]:
        problems.append("нет времени по проходам стенда (bench.passes)")
    if "refresh_hz" not in _dict(report.get("display")):
        problems.append("в отчёте нет достигнутой частоты экрана (display.refresh_hz)")
    if not isinstance(report.get("fits_frame_budget"), bool):
        problems.append("нет сравнения с бюджетом кадра (fits_frame_budget)")
    return problems
