"""Ожидания smoke-сценария ``capture`` (сессия 14): снимок на настоящем экране камеры до файла в галерее.

Точные правила и пороги судит Rust (конфиг ``camera.scenarios.capture``, ``capture_verdict.rs``); здесь — независимая
грубая перепроверка сводки строки результата: все шаги плана выполнены, сервис снимка был НАСТОЯЩИМ и при старте, и в
конце, ни один снимок не сорван и не остался без итога, адреса файлов не повторяются и их число сходится с числом
снимков (HEIC + DNG там, где просили RAW), учёт серии сходится (нажатия = попытки + отказы экрана; принятое =
сохранённое + сорванное + без итога), видоискатель не просел, выбор кадра известен у каждого снимка и задержка
«нажатие → кадр» измерена (``frames``). Подробности (каждый снимок с временами, шаги, серия)
лежат в строках ``detail_capture`` и печатаются как ``FASTICAMERA_DETAIL``.

Файлы на телефоне проверяет и удаляет ``smoke_checks_capture_files.py`` (после результата, адреса — из сводки).

Каждая проверка — чистая функция «JSON результата → список проблем» (пустой список — прошёл).
"""

from __future__ import annotations

from smoke_checks import _is_number, _report_problem

EXPECTED_STEPS = 3
"""Шагов плана: ``single_heif``, ``single_heif_dng``, ``burst`` (``capture_plan.rs``)."""

SAVED_OUTCOME = "saved"
SAVED_WITH_RAW_OUTCOME = "saved_with_raw"
"""Исходы снимка в сводке (``compact_shot`` в ``capture_report.rs``): один файл или HEIC + DNG."""

BURST_VIEWFINDER_ZERO_FIELDS = (
    ("pool_exhausted", "исчерпан пул кадров камеры"),
    ("black_frames_while_picture_expected", "чёрных кадров, где ждали картинку"),
    ("holds_started", "удержаний картинки"),
)
"""Счётчики видоискателя серии, которые обязаны быть нулями (``jitter_overflow`` — по порогу конфига Rust)."""


def check_capture(result: dict) -> list[str]:
    """``capture``: шаги прошли, сервис настоящий, снимки сохранены, адреса сходятся, учёт серии замкнут."""
    problems, report = _report_problem(result)
    if problems:
        return problems
    problems.extend(_steps_problems(report))
    problems.extend(_mode_problems(report))
    problems.extend(_shots_problems(report))
    problems.extend(_burst_problems(report))
    problems.extend(_frames_problems(report))
    if report.get("phase_entries_lost") != 0:
        problems.append(f"журнал итогов снимков переполнился: потеряно записей {report.get('phase_entries_lost')!r}")
    return problems


def _steps_problems(report: dict) -> list[str]:
    planned = report.get("steps_planned")
    done = report.get("steps_done")
    problems: list[str] = []
    if planned != EXPECTED_STEPS:
        problems.append(f"в плане шагов {planned!r}, ожидалось {EXPECTED_STEPS}")
    if done != planned:
        problems.append(f"выполнено шагов {done!r} из {planned!r}")
    if report.get("unfinished") != 0:
        problems.append(f"не выполнено шагов: {report.get('unfinished')!r}")
    return problems


def _mode_problems(report: dict) -> list[str]:
    """Сервис снимка настоящий и при старте, и в конце: фейк делает файлы «несуществующими»."""
    problems: list[str] = []
    for key, when in (("mode", "при старте"), ("mode_end", "в конце")):
        mode = report.get(key)
        name = mode.get("mode") if isinstance(mode, dict) else None
        if name != "real":
            problems.append(f"сервис снимка {when} не настоящий: {mode!r}")
    return problems


def saved_shots(report: dict) -> list[dict]:
    """Снимки сводки с файлами (``saved`` и ``saved_with_raw``) по порядку: от них зависит порядок адресов."""
    shots = report.get("shots")
    if not isinstance(shots, list):
        return []
    return [
        shot
        for shot in shots
        if isinstance(shot, dict) and shot.get("outcome") in (SAVED_OUTCOME, SAVED_WITH_RAW_OUTCOME)
    ]


def _shots_problems(report: dict) -> list[str]:
    shots = report.get("shots")
    uris = report.get("uris")
    if not isinstance(shots, list) or not shots:
        return [f"в сводке нет снимков (shots={shots!r})"]
    if not isinstance(uris, list) or not all(isinstance(uri, str) for uri in uris):
        return [f"в сводке нет списка адресов файлов (uris={uris!r})"]
    problems: list[str] = []
    for shot in shots:
        outcome = shot.get("outcome") if isinstance(shot, dict) else None
        if outcome not in (SAVED_OUTCOME, SAVED_WITH_RAW_OUTCOME):
            problems.append(f"снимок {shot.get('n') if isinstance(shot, dict) else '?'} не сохранён: {outcome!r}")
    expected_files = sum(2 if shot.get("outcome") == SAVED_WITH_RAW_OUTCOME else 1 for shot in saved_shots(report))
    if len(uris) != expected_files:
        problems.append(f"адресов файлов {len(uris)}, а по снимкам должно быть {expected_files}")
    if len(set(uris)) != len(uris):
        problems.append(f"адреса файлов повторяются: {len(uris)} адресов, уникальных {len(set(uris))}")
    raw_shots = [shot for shot in shots if isinstance(shot, dict) and shot.get("outcome") == SAVED_WITH_RAW_OUTCOME]
    if len(raw_shots) != 1:
        problems.append(f"снимков с DNG {len(raw_shots)}, ожидался ровно один (шаг single_heif_dng)")
    if any(isinstance(shot, dict) and shot.get("fmt") != "heif" for shot in shots):
        problems.append("основной формат не только heif")
    return problems


def _burst_problems(report: dict) -> list[str]:
    burst = report.get("burst")
    if not isinstance(burst, dict):
        return [f"в сводке нет учёта серии (burst={burst!r})"]
    numbers = {key: burst.get(key) for key in ("presses", "submitted", "accepted", "rejected_by_service", "refused_by_screen", "saved", "failed", "unfinished")}
    bad = [key for key, value in numbers.items() if not _is_number(value)]
    if bad:
        return [f"в учёте серии нет чисел: {', '.join(bad)}"]
    problems: list[str] = []
    if numbers["presses"] != numbers["submitted"] + numbers["refused_by_screen"]:
        problems.append(
            f"серия: нажатий {numbers['presses']}, а попыток постановки {numbers['submitted']} "
            f"+ отказов экрана {numbers['refused_by_screen']}"
        )
    if numbers["submitted"] != numbers["accepted"] + numbers["rejected_by_service"]:
        problems.append(
            f"серия: попыток {numbers['submitted']}, а принято {numbers['accepted']} + отвергнуто {numbers['rejected_by_service']}"
        )
    accounted = numbers["saved"] + numbers["failed"] + numbers["unfinished"]
    if numbers["accepted"] != accounted:
        problems.append(f"серия: принято {numbers['accepted']}, а итогов {accounted}")
    if numbers["failed"] != 0 or numbers["unfinished"] != 0:
        problems.append(f"серия: сорвано {numbers['failed']}, без итога {numbers['unfinished']}")
    if numbers["saved"] < 1:
        problems.append("серия: ни одного сохранённого снимка")
    viewfinder = burst.get("viewfinder")
    if not isinstance(viewfinder, dict):
        return problems + [f"в учёте серии нет счётчиков видоискателя (viewfinder={viewfinder!r})"]
    for key, label in BURST_VIEWFINDER_ZERO_FIELDS:
        if viewfinder.get(key) != 0:
            problems.append(f"серия, видоискатель: {label} {viewfinder.get(key)!r}, ожидалось 0")
    return problems


def _frames_problems(report: dict) -> list[str]:
    """«Снимок без задержки затвора»: выбор кадра известен у каждого снимка, показанный кадр не вытеснен из кольца,
    задержка измерена. Пороги (доля точных, предел задержки) судит Rust по конфигу; здесь — только наличие и согласие чисел."""
    frames = report.get("frames")
    if not isinstance(frames, dict):
        return [f"в сводке нет учёта выбора кадра (frames={frames!r})"]
    counts = {key: frames.get(key) for key in ("exact", "subst", "evicted", "unknown")}
    bad = [key for key, value in counts.items() if not _is_number(value)]
    if bad:
        return [f"в учёте выбора кадра нет чисел: {', '.join(bad)}"]
    problems: list[str] = []
    total = report.get("shots_total")
    accounted = counts["exact"] + counts["subst"] + counts["unknown"]
    if _is_number(total) and accounted != total:
        problems.append(f"выбор кадра учтён у {accounted} снимков из {total}")
    if counts["unknown"] != 0:
        problems.append(f"выбор кадра неизвестен у снимков: {counts['unknown']}")
    if counts["evicted"] != 0:
        problems.append(f"показанный кадр вытеснен из кольца у {counts['evicted']} снимков (кольцо мелкое)")
    if not _is_number(frames.get("p2f_max_ms")):
        problems.append(f"задержка «нажатие → кадр» не измерена (p2f_max_ms={frames.get('p2f_max_ms')!r})")
    return problems


def capture_notes(result: dict) -> list[str]:
    """Пояснения прошедшего ``capture`` (не провал): времена снимков, отказы серии, интервалы, опоздавшие кадры."""
    report = result.get("report")
    if not isinstance(report, dict):
        return []
    notes: list[str] = []
    shots = report.get("shots")
    if isinstance(shots, list):
        for shot in shots:
            if isinstance(shot, dict) and shot.get("step") != "burst":
                notes.append(
                    f"capture: снимок {shot.get('n')} ({shot.get('step')}): «кадр захвачен» через {shot.get('secured_ms')} мс, "
                    f"«сохранено» через {shot.get('saved_ms')} мс от нажатия"
                )
        burst_times = [shot.get("saved_ms") for shot in shots if isinstance(shot, dict) and shot.get("step") == "burst" and _is_number(shot.get("saved_ms"))]
        if burst_times:
            notes.append(f"capture: серия: «сохранено» через {min(burst_times)}…{max(burst_times)} мс от своих нажатий")
    burst = report.get("burst")
    if isinstance(burst, dict):
        notes.append(
            f"capture: серия: нажатий {burst.get('presses')}, до сервиса дошло {burst.get('submitted')}, "
            f"отказал экран {burst.get('refused_by_screen')}, отверг сервис {burst.get('rejected_by_service')}, "
            f"сохранено {burst.get('saved')}; интервал между нажатиями {burst.get('press_gap_ms')} мс"
        )
        viewfinder = burst.get("viewfinder")
        if isinstance(viewfinder, dict):
            notes.append(
                f"capture: видоискатель за серию: выброшено джиттер-буфером {viewfinder.get('jitter_overflow')}, "
                f"повторов кадра {viewfinder.get('underruns')}, опоздавших кадров {viewfinder.get('late_frames')} "
                f"(число, не провал), живых кадров {viewfinder.get('live_frames_presented')}"
            )
        if _is_number(burst.get("refused_by_screen")) and burst["refused_by_screen"] > 0:
            notes.append(
                f"capture: NOTE экран отказал {burst['refused_by_screen']} нажатиям серии (затвор занят до «кадр захвачен» "
                "или набор сохраняемых полон): допустимо ли это при 100 мс — решение владельца"
            )
    render = report.get("render_still")
    if isinstance(render, dict):
        notes.append(f"capture: рендер принял команд снимка {render.get('begins_accepted')} (приращение за прогон)")
    frames = report.get("frames")
    if isinstance(frames, dict):
        notes.append(
            f"capture: NOTE точно показанный кадр взят у {frames.get('exact')} снимков, замен {frames.get('subst')} "
            f"(вытеснено из кольца {frames.get('evicted')}), доля точных {frames.get('share')}; задержка «нажатие → начало "
            f"экспозиции кадра»: медиана {frames.get('p2f_p50_ms')} мс, максимум {frames.get('p2f_max_ms')} мс"
        )
    return notes
