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

STAGE_BUDGET_SURFACES = ("bare_viewfinder", "photo_screen", "calibrate_screen")
"""Места замера `viewfinder-stage-budget` (контракт с `stage_budget_surface_config.rs`, snake_case)."""

STAGE_BUDGET_LOOK_ROWS = (
    "look_tone_base_agx",
    "look_tone_base_aces",
    "look_tone_base_filmic",
    "look_layer",
    "look_wheels",
    "look_tone",
    "look_primaries",
)
"""Строки вида 14o (контракт с `look_row.rs::LookRow::name`): вид считает шейдер видоискателя, строки годятся на всех местах."""

STAGE_BUDGET_ROW_SURFACES = {
    "color_scopes": ("calibrate_screen",),
    "ui_overlay": ("photo_screen", "calibrate_screen"),
    "whole_product": ("photo_screen", "calibrate_screen"),
    **{row: STAGE_BUDGET_SURFACES for row in STAGE_BUDGET_LOOK_ROWS},
}
"""Строки 16a, допустимые не везде (остальные строки — стадии, `whole_viewfinder`, `display_120hz` — на всех местах).
Rust отвергает недопустимое сочетание ещё при initialize; здесь независимая перепроверка результата."""

STAGE_BUDGET_BASELINES = {
    "whole_viewfinder": ("camera_without_isp", "«весь видоискатель»", "«камера без ISP»"),
    "whole_product": ("camera_without_product_gpu", "«вся наша работа»", "«камера без нашей работы GPU»"),
}
"""Строка, из B-окон которой берётся абсолютная база: имя строки базы в таблице и слова для сообщения."""

TONE_BASES = ("agx", "aces", "filmic", "legacy")
"""Модели основы тона (контракт с `look_contract::ToneBaseModel::name`, snake_case)."""

LOOK_REPORT_FLAGS = ("layer_neutral", "wheels_active", "tone_active", "primaries_active")
"""Булевы признаки слоёв в отчёте о виде (контракт с `scenario/camera/look_report.rs`). `primaries_active` (14t) есть в
полном отчёте окна и на входе; в сжатом виде окна строки (`look_rows`) его нет — см. `_compact_look_problems`."""

FINGERPRINT_LENGTH = 16
"""Отпечаток рецепта вида — 16 шестнадцатеричных цифр (`LookRecipe::fingerprint_hex`)."""

STAGE_BUDGET_REJECTION_REASONS = frozenset({"state_not_applied", "display_refresh_not_reached"})
"""Причины отказа окна по состоянию (16a): строка с такой причиной в вердикте не `measured`."""

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

MIN_STATISTICS_PUBLISH_FRACTION = 0.25
"""Грубая нижняя граница публикаций статистики кадра от ожидаемого при прореживании (обработанные кадры / N);
Rust судит строже (0,5): исполнитель бывает занят, окно режет период на краях."""

MAX_ACCOUNTING_GAP = 0.10
"""Грубый допуск «обработано − скрыто на холодном старте = показано» (Rust судит строже, 3 %)."""

MS_PER_MINUTE = 60_000.0


def _dict(value: object) -> dict:
    return value if isinstance(value, dict) else {}


def look_report_problems(look: object, where: str) -> list[str]:
    """Отчёт о виде и основе (14o) на месте ``where``: основа из каталога, отпечаток рецепта, три признака слоёв и имя
    вида замера. Отчёт без этих полей — не отчёт: по нему нельзя сказать, какую картинку мерили."""
    if not isinstance(look, dict):
        return [f"в отчёте нет вида и основы ({where})"]
    problems: list[str] = []
    if look.get("tone_base") not in TONE_BASES:
        problems.append(f"{where}.tone_base={look.get('tone_base')!r}, ожидалось одно из {list(TONE_BASES)}")
    fingerprint = look.get("recipe_fingerprint")
    if not (
        isinstance(fingerprint, str)
        and len(fingerprint) == FINGERPRINT_LENGTH
        and all(char in "0123456789abcdef" for char in fingerprint)
    ):
        problems.append(f"{where}.recipe_fingerprint={fingerprint!r}, ожидалось {FINGERPRINT_LENGTH} hex-цифр")
    for flag in LOOK_REPORT_FLAGS:
        if not isinstance(look.get(flag), bool):
            problems.append(f"{where}.{flag}={look.get(flag)!r}, ожидалось true или false")
    if not isinstance(look.get("measurement_look"), str) or not look["measurement_look"]:
        problems.append(f"{where}.measurement_look не назван")
    return problems


def _compact_look_problems(look: object, where: str) -> list[str]:
    """Сжатый вид окна строки вида в строке результата: основа, отпечаток, колёса, тон (полный отчёт — в detail)."""
    if not isinstance(look, dict):
        return [f"в отчёте нет вида окна ({where})"]
    full = {"layer_neutral": False, "primaries_active": False, "measurement_look": "-", **look}
    return look_report_problems(full, where)


def expected_tone_base_problems(result: dict, expected: str) -> list[str]:
    """Основа, запрошенная флагом ``--tone-base``, действительно была показана (14o).

    Конфиг ``render.viewfinder.tone.base`` — лишь УМОЛЧАНИЕ встроенных пресетов: если у владельца активен свой пресет
    или есть сохранённые правки, у них своя основа и флаг молча ничего не меняет. Поэтому расхождение — провал с
    причиной, а не «прошло с другой основой». Читает ``viewfinder`` (``look.picture``) и ``viewfinder-stage-budget``
    (``look`` на входе на место замера); другим сценариям основа в отчёте не нужна — флаг для них отвергает
    ``device-smoke.sh``."""
    report = _dict(result.get("report"))
    if result.get("scenario") == "viewfinder":
        shown = _dict(_dict(report.get("look")).get("picture")).get("tone_base")
    else:
        shown = _dict(report.get("look")).get("tone_base")
    if shown == expected:
        return []
    return [
        f"основа в отчёте {shown!r}, а запрошена {expected!r} (--tone-base): у активного пресета или сохранённых "
        "правок своя основа, конфиг viewfinder.tone.base лишь умолчание встроенных пресетов; смените пресет или "
        "сбросьте правки на телефоне"
    ]


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
    problems.extend(look_report_problems(look.get("picture"), "look.picture"))
    if look.get("painter") != "isp":
        problems.append(f"рисовальщик {look.get('painter')!r}, ожидался 'isp'")
    if look.get("display_color_space") not in COLOR_SPACES:
        problems.append(f"пространство экрана {look.get('display_color_space')!r}, ожидалось одно из {sorted(COLOR_SPACES)}")
    problems.extend(_statistics_problems(report))
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


def _statistics_problems(report: dict) -> list[str]:
    """Статистика кадра в сводке ``viewfinder``: идёт с прореживанием (не на каждом кадре), если на потоке рендера
    есть её потребитель (наш AE). Раздел ``frame_statistics`` — это расписание: объект с периодом ``every_n_frames``
    или ``null`` (потребителей нет, вендорский AE: публикаций 0, и это не поломка). Ожидается не меньше доли
    ``frames_processed / every_n_frames`` публикаций; верхней границы нет (холодный старт AE считает на каждом кадре)."""
    if "frame_statistics" not in report:
        return ["в сводке нет раздела frame_statistics (расписание статистики кадра)"]
    schedule = report["frame_statistics"]
    if schedule is None:
        return []
    if not isinstance(schedule, dict):
        return [f"frame_statistics={schedule!r}, ожидался объект или null"]
    published = report.get("statistics_published")
    if not _is_number(published) or published <= 0:
        return [f"статистика кадра не публиковалась при включённом AE (statistics_published={published!r})"]
    period, processed = schedule.get("every_n_frames"), report.get("frames_processed")
    if not _is_number(period) or period < 1 or not _is_number(processed):
        return [f"нет периода статистики или числа кадров (every_n_frames={period!r}, frames_processed={processed!r})"]
    if published < processed / period * MIN_STATISTICS_PUBLISH_FRACTION:
        return [f"статистика кадра опубликована {published} раз, при {processed} кадрах и периоде {period} ожидалось больше"]
    return []


def _awb_problems(report: dict) -> list[str]:
    """Белая в сводке ``viewfinder``: белая взята у телефона, заявка закрыта исходом applied, холодный старт кончился
    и скрыл не больше предела. Оценки старого оценщика продукт больше не судит; статистика кадра — в
    ``_statistics_problems``."""
    white_balance = report.get("white_balance")
    if not isinstance(white_balance, dict):
        return ["в сводке нет раздела white_balance (белая)"]
    problems: list[str] = []
    origin = white_balance.get("origin")
    request = _dict(white_balance.get("request")).get("state")
    if origin != "phone":
        problems.append(f"белая не взята у телефона (origin={origin!r}, request={request!r})")
    elif request != "applied":
        problems.append(f"заявка белого не закрыта исходом applied (request={request!r})")
    if white_balance.get("cold_start_pending") is not False:
        problems.append("холодный старт не кончился")
    hidden, limit = white_balance.get("hidden_frames"), white_balance.get("max_hidden_frames")
    if not _is_number(hidden) or not _is_number(limit):
        problems.append(f"нет числа скрытых кадров (hidden_frames={hidden!r}, max_hidden_frames={limit!r})")
    elif hidden > limit:
        problems.append(f"холодный старт скрыл {hidden} кадров, предел {limit}")
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
    """``viewfinder-stage-budget``: таблица по строке на каждую запрошенную строку, по 4 столбца, база для строк
    «весь видоискатель» и «вся наша работа», место замера и (на экране калибровки) запрос прибора."""
    problems, report = _report_problem(result)
    if problems:
        return problems
    table = report.get("table")
    requested = report.get("requested_rows")
    if not isinstance(table, list) or not isinstance(requested, list) or not requested:
        return [f"нет таблицы или списка строк (table={table!r}, requested_rows={requested!r})"]
    baseline_names = tuple(baseline for baseline, _row_words, _baseline_words in STAGE_BUDGET_BASELINES.values())
    measured = [
        line for line in table if isinstance(line, str) and not line.startswith(baseline_names)
    ]
    if len(measured) != len(requested):
        problems.append(f"строк таблицы {len(measured)}, запрошено {len(requested)}")
    for line in table:
        if not isinstance(line, str) or len(line.split(" | ")) != TABLE_COLUMNS:
            problems.append(f"строка таблицы не из {TABLE_COLUMNS} столбцов: {line!r}")
    for row, (baseline, row_words, baseline_words) in STAGE_BUDGET_BASELINES.items():
        if row in requested and not any(isinstance(line, str) and line.startswith(baseline) for line in table):
            problems.append(f"есть {row_words}, но нет строки {baseline_words} ({baseline})")
    problems.extend(_stage_budget_surface_problems(report, requested))
    problems.extend(_stage_budget_look_problems(report, requested))
    problems.extend(_stage_budget_rejection_problems(table))
    return problems


def _stage_budget_surface_problems(report: dict, requested: list) -> list[str]:
    """Место замера названо, строки на нём допустимы, а на экране калибровки назван прибор и возможности f32."""
    surface = report.get("surface")
    if surface not in STAGE_BUDGET_SURFACES:
        return [f"место замера surface={surface!r}, ожидалось одно из {list(STAGE_BUDGET_SURFACES)}"]
    problems = [
        f"строка {row} не мерится на месте {surface} (допустимо: {', '.join(allowed)})"
        for row, allowed in STAGE_BUDGET_ROW_SURFACES.items()
        if row in requested and surface not in allowed
    ]
    if surface == "calibrate_screen":
        scopes = _dict(report.get("scopes"))
        if not isinstance(scopes.get("request"), dict):
            problems.append("на экране калибровки в отчёте нет запроса прибора (scopes.request)")
        if not isinstance(scopes.get("float32_controls"), dict):
            problems.append("на экране калибровки в отчёте нет возможностей f32 (scopes.float32_controls)")
    return problems


def _stage_budget_look_problems(report: dict, requested: list) -> list[str]:
    """14o: вид и основа на входе названы; у каждой строки вида окна A и B показали РАЗНЫЕ рецепты (иначе строка мерила
    бы ноль); на экране калибровки пирамида размытия фона не строилась (решение 261)."""
    problems = look_report_problems(report.get("look"), "look")
    rows = {_dict(row).get("row"): _dict(row) for row in report.get("look_rows") or []}
    for name in requested:
        if name not in STAGE_BUDGET_LOOK_ROWS:
            continue
        row = rows.get(name)
        if row is None:
            problems.append(f"у строки вида {name} нет видов окон в отчёте (look_rows)")
            continue
        windows = {window: row.get(window) for window in ("on", "off")}
        for window, look in windows.items():
            problems.extend(_compact_look_problems(look, f"look_rows[{name}].{window}"))
        on_fp, off_fp = (_dict(look).get("recipe_fingerprint") for look in windows.values())
        if on_fp is not None and on_fp == off_fp:
            problems.append(f"строка {name}: окна A и B показали один и тот же рецепт ({on_fp}), строка мерила бы ноль")
    if "backdrop_pyramid_rebuilds" not in report:
        problems.append("в отчёте нет числа построений размытия фона (backdrop_pyramid_rebuilds)")
    elif report.get("surface") == "calibrate_screen":
        rebuilds = report["backdrop_pyramid_rebuilds"]
        if not isinstance(rebuilds, int) or isinstance(rebuilds, bool):
            problems.append(f"backdrop_pyramid_rebuilds={rebuilds!r} на экране калибровки: окон с диагностикой не было")
        elif rebuilds != 0:
            problems.append(f"в разделе калибровки строилась пирамида размытия фона {rebuilds} раз (решение 261: 0)")
    return problems


def _stage_budget_rejection_problems(table: list) -> list[str]:
    """Строка с отказом окна по состоянию — проблема с названной причиной, а не «измерено» и не безымянный отказ."""
    problems = []
    for line in table:
        if not isinstance(line, str):
            continue
        parts = line.split(" | ")
        verdict = parts[-1].strip() if parts else ""
        if verdict.startswith("rejected: "):
            reason = verdict[len("rejected: "):]
            kind = "состояние не подтвердилось" if reason in STAGE_BUDGET_REJECTION_REASONS else "ток не измерен"
            problems.append(f"строка {parts[0]}: окно отвергнуто ({kind}): {reason}")
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
