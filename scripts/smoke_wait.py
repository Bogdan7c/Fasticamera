#!/usr/bin/env python3
"""Ожидание и проверка строки результата smoke-сценария Fasticamera в logcat.

Вызывается из ``scripts/device-smoke.sh`` после запуска Activity. Читает logcat телефона
(только тег ``Fasticamera`` и падения ``AndroidRuntime``) с момента запуска, ждёт строку
``FASTICAMERA_RESULT {json}`` нужного сценария и сверяет её с ожиданиями сценария.

Устройство:
* ``evaluate_stream`` — логика над последовательностью строк (тестируется на ПК); на строках
  стадий ``FASTICAMERA_STAGE`` вызывает обработчик (драйвер шагов из ``smoke_driver``);
* ``CHECKS`` — ожидания каждого сценария (что именно считается «прошёл»);
* ``read_logcat_lines`` — ввод: подпроцесс ``adb logcat`` с дедлайном, переживает обрыв ``adb``
  (логика переподключения — ``smoke_logcat``);
* ``main`` — разбор аргументов и вывод итога.

Коды выхода совпадают с ``scripts/lib/common.sh``.
"""

from __future__ import annotations

import argparse
import dataclasses
import json
import queue
import re
import subprocess
import sys
import threading
import time
from collections.abc import Callable, Iterable, Iterator
from dataclasses import dataclass, field, replace
from pathlib import Path

import smoke_checks
import smoke_checks_ae_trace
import smoke_checks_capture
import smoke_checks_capture_files
import smoke_checks_three_a
import smoke_checks_glass
import smoke_checks_saturation
import smoke_checks_screen
import smoke_checks_standby
import smoke_checks_zoom
import smoke_checks_viewfinder
import smoke_logcat
import smoke_neutral_card
import smoke_screen
from smoke_driver import STAGE_MARKER, AdbDeviceActions, DriverError, StageDriver
from smoke_dumpsys import CheckOutcome, PostCheck, dumpsys_post_check

EXIT_OK = 0
EXIT_SCENARIO_FAILED = 1
EXIT_NO_RESULT = 3
EXIT_USAGE = 64

RESULT_MARKER = "FASTICAMERA_RESULT "
"""Префикс строки результата; контракт с ``crates/fasticamera-app/src/result_line.rs``. Наблюдающие
сценарии повторяют строку несколько раз (решение 58, ``result_echo.rs``): logcat теряет строки во
всплеске записи. Вердикт — по первой дошедшей копии, остальные копии ожидание уже не читает."""

PANIC_MARKER = "panic at "
"""Начало записи panic hook; контракт с ``crates/fasticamera-app/src/panic_report.rs``."""

CRASH_MARKER = "FATAL EXCEPTION"
"""Так AndroidRuntime начинает отчёт о падении процесса."""

CRASH_PROCESS_PATTERN = re.compile(r"Process: (?P<process>[^,\s]+)")
"""Строка AndroidRuntime после заголовка: ``Process: <пакет>, PID: <pid>``."""

CRASH_PROCESS_LOOKAHEAD_LINES = 3
"""Сколько строк AndroidRuntime после заголовка ищем ``Process:``. Не нашли — чьё это падение,
неизвестно (например, ``FATAL EXCEPTION IN SYSTEM PROCESS``), и чужим процессам приложение не мешает."""

EXPECTED_ABI = "arm64-v8a"


DETAIL_LINE_MARKER = "FASTICAMERA_DETAIL"
"""Префикс строк подробностей в выводе скрипта (чтобы их можно было отфильтровать `grep`)."""

DETAIL_STAGE_PREFIX = "detail_"
"""Стадии с таким префиксом несут подробности отчёта, не поместившиеся в строку результата (4000 байт
записи logcat): их не исполняет драйвер, а печатает `main` после вердикта."""


@dataclass(frozen=True)
class SmokeVerdict:
    """Итог проверки: код выхода, объяснение для человека и подробности отчёта (строки ``detail_*``)."""

    exit_code: int
    message: str
    details: tuple[dict, ...] = ()
    notes: tuple[str, ...] = ()


@dataclass
class ObservedLog:
    """Что увидели в logcat до строки результата — нужно проверкам, которым мало JSON."""

    panic_lines: list[str] = field(default_factory=list)


class CrashAttribution:
    """Отличает падение НАШЕГО процесса от падения любого другого приложения на телефоне.

    ``AndroidRuntime:E`` — общий тег: туда пишут падения всех приложений. Формат отчёта:
    ``FATAL EXCEPTION: main`` и на следующей строке ``Process: <пакет>, PID: <pid>``. Заголовок
    запоминается, а решение принимает строка ``Process:``. ``package=None`` (пакет не известен)
    — старое консервативное поведение: считаем падением любой заголовок.
    """

    def __init__(self, package: str | None) -> None:
        self._package = package
        self._pending_header: str | None = None
        self._lookahead_left = 0

    def feed(self, line: str) -> str | None:
        """Подаёт очередную строку. Возвращает заголовок падения, если упал наш процесс."""
        if CRASH_MARKER in line:
            if self._package is None:
                return line.strip()
            self._pending_header = line.strip()
            self._lookahead_left = CRASH_PROCESS_LOOKAHEAD_LINES
            return None
        if self._pending_header is None or "AndroidRuntime" not in line:
            return None
        header = self._pending_header
        process_match = CRASH_PROCESS_PATTERN.search(line)
        if process_match is not None:
            self._pending_header = None
            return header if self._is_our_process(process_match.group("process")) else None
        self._lookahead_left -= 1
        if self._lookahead_left <= 0:
            self._pending_header = None
        return None

    def _is_our_process(self, process: str) -> bool:
        """Процесс приложения называется как пакет или ``пакет:суффикс`` (дополнительные процессы)."""
        return process == self._package or process.startswith(f"{self._package}:")


ScenarioCheck = Callable[[dict, ObservedLog], list[str]]
"""Проверка сценария: (JSON результата, наблюдения) → список проблем; пустой — прошёл."""


def check_hello(result: dict, _observed: ObservedLog) -> list[str]:
    """``hello``: библиотека загружена, знает свою версию, ревизию и ABI."""
    problems = []
    if result.get("status") != "passed":
        problems.append(f"status={result.get('status')!r}, ожидался 'passed' ({result.get('error', '')})")
        return problems
    report = result.get("report", {})
    if report.get("abi") != EXPECTED_ABI:
        problems.append(f"abi={report.get('abi')!r}, ожидался {EXPECTED_ABI!r}")
    build = report.get("build", {})
    if not build.get("crate_version"):
        problems.append("нет версии сборки (report.build.crate_version)")
    if not build.get("git_revision"):
        problems.append("нет ревизии git (report.build.git_revision): библиотека собрана не через Gradle?")
    return problems


def check_panic(result: dict, observed: ObservedLog) -> list[str]:
    """``panic``: паника поймана на границе, место и сообщение дошли и до отчёта, и до logcat."""
    problems = []
    if result.get("status") != "panicked":
        problems.append(f"status={result.get('status')!r}, ожидался 'panicked' ({result.get('error', '')})")
        return problems
    panic = result.get("panic", {})
    location = panic.get("location") or ""
    if "intentional_panic.rs:" not in location:
        problems.append(f"место паники {location!r} не указывает на сценарий intentional_panic.rs")
    if not panic.get("message"):
        problems.append("пустое сообщение паники")
    expected_log = f"{PANIC_MARKER}{location}"
    if location and not any(expected_log in line for line in observed.panic_lines):
        problems.append(f"в logcat нет записи panic hook {expected_log!r}")
    return problems


def ignoring_observations(check: Callable[[dict], list[str]]) -> ScenarioCheck:
    """Приводит проверку, которой хватает JSON результата, к общей сигнатуре ``ScenarioCheck``."""
    return lambda result, _observed: check(result)


CHECKS: dict[str, ScenarioCheck] = {
    "hello": check_hello,
    "panic": check_panic,
    "lifecycle": ignoring_observations(smoke_checks.check_lifecycle),
    "permission": ignoring_observations(smoke_checks.check_permission),
    "motion": ignoring_observations(smoke_checks.check_motion),
    "platform": ignoring_observations(smoke_checks.check_platform),
    "render": ignoring_observations(smoke_checks.check_render),
    "render-lifecycle": ignoring_observations(smoke_checks.check_render_lifecycle),
    "ahb-import": ignoring_observations(smoke_checks.check_ahb_import),
    "gpu-fence": ignoring_observations(smoke_checks.check_gpu_fence),
    "camera-catalog": ignoring_observations(smoke_checks.check_camera_catalog),
    "camera-stream": ignoring_observations(smoke_checks.check_camera_stream),
    "camera-evict": ignoring_observations(smoke_checks.check_camera_evict),
    "raw-stream": ignoring_observations(smoke_checks.check_raw_stream),
    "raw-stream-endurance": ignoring_observations(smoke_checks.check_raw_stream),
    "raw-stream-cpu-probe": ignoring_observations(smoke_checks.check_raw_stream_cpu_probe),
    "viewfinder": ignoring_observations(smoke_checks_viewfinder.check_viewfinder),
    "viewfinder-latency": ignoring_observations(smoke_checks_viewfinder.check_viewfinder_latency),
    "viewfinder-stage-budget": ignoring_observations(smoke_checks_viewfinder.check_viewfinder_stage_budget),
    "viewfinder-endurance": ignoring_observations(smoke_checks_viewfinder.check_viewfinder_endurance),
    "video-headroom-bench": ignoring_observations(smoke_checks_viewfinder.check_video_headroom_bench),
    "3a": ignoring_observations(smoke_checks_three_a.check_three_a),
    "ae-trace": ignoring_observations(smoke_checks_ae_trace.check_ae_trace),
    "glass": ignoring_observations(smoke_checks_glass.check_glass),
    "screen": ignoring_observations(smoke_checks_screen.check_screen),
    "zoom": ignoring_observations(smoke_checks_zoom.check_zoom),
    "capture": ignoring_observations(smoke_checks_capture.check_capture),
    "standby": ignoring_observations(smoke_checks_standby.check_standby),
    "gpu-saturation-bench": ignoring_observations(smoke_checks_saturation.check_gpu_saturation_bench),
}

NOTES: dict[str, Callable[[dict], list[str]]] = {
    "3a": smoke_checks_three_a.three_a_notes,
    "glass": smoke_checks_glass.glass_notes,
    "screen": smoke_checks_screen.screen_notes,
    "zoom": smoke_checks_zoom.zoom_notes,
    "capture": smoke_checks_capture.capture_notes,
    "standby": smoke_checks_standby.standby_notes,
    "gpu-saturation-bench": smoke_checks_saturation.saturation_notes,
}
"""Пояснения к вердикту прошедшего сценария (не провал): печатаются после вердикта, как заметки пост-проверок."""

StageHandler = Callable[[dict], None]
"""Обработчик стадии сценария: получает JSON стадии, при неудаче шага бросает ``DriverError``."""


def extract_result_payload(line: str) -> str | None:
    """JSON из строки logcat с маркером результата или ``None``."""
    marker_position = line.find(RESULT_MARKER)
    if marker_position < 0:
        return None
    return line[marker_position + len(RESULT_MARKER) :].strip()


def extract_stage_payload(line: str) -> str | None:
    """JSON из строки logcat с маркером стадии или ``None``."""
    marker_position = line.find(STAGE_MARKER)
    if marker_position < 0:
        return None
    return line[marker_position + len(STAGE_MARKER) :].strip()


def evaluate_stream(
    lines: Iterable[str],
    scenario: str,
    stage_handler: StageHandler | None = None,
    package: str | None = None,
    post_check: PostCheck | None = None,
    cleanup: PostCheck | None = None,
) -> SmokeVerdict:
    """Как ``_evaluate_stream``, но вердикт несёт собранные подробности отчёта (стадии ``detail_*``)."""
    details: list[dict] = []
    notes: list[str] = []
    verdict = _evaluate_stream(lines, scenario, stage_handler, package, post_check, details, notes, cleanup)
    return replace(verdict, details=tuple(details), notes=tuple(notes))


def _collect_detail(stage_payload: str, scenario: str, details: list[dict]) -> None:
    """Запоминает стадию ``detail_*`` нужного сценария; битую строку пропускает (её разберёт обработчик стадий)."""
    try:
        stage_event = json.loads(stage_payload)
    except json.JSONDecodeError:
        return
    if (
        isinstance(stage_event, dict)
        and stage_event.get("scenario") == scenario
        and str(stage_event.get("stage", "")).startswith(DETAIL_STAGE_PREFIX)
    ):
        details.append(stage_event)


def _evaluate_stream(
    lines: Iterable[str],
    scenario: str,
    stage_handler: StageHandler | None,
    package: str | None,
    post_check: PostCheck | None,
    details: list[dict],
    notes: list[str],
    cleanup: PostCheck | None = None,
) -> SmokeVerdict:
    """Читает строки до результата нужного сценария (или падения процесса) и выносит вердикт.

    Строки результата и стадии других сценариев пропускаются: это хвост прошлого запуска.
    Стадии нужного сценария передаются ``stage_handler`` (если задан) по мере прихода; пока
    обработчик занят (пауза, опрос диалога), строки копятся в очереди чтения и не теряются.
    Сбой шага (``DriverError``) — провал сценария с объяснением. Падение процесса считается
    только для ``package`` (см. ``CrashAttribution``); падения чужих приложений игнорируются.
    ``post_check`` (если задан) вызывается над JSON результата, когда основная проверка прошла: так
    ``camera-catalog`` сверяется с ``dumpsys media.camera`` уже после результата. Её проблемы — провал.
    ``cleanup`` (если задан) вызывается над JSON результата ВСЕГДА, как только пришёл результат нужного сценария, —
    и после успешной, и после проваленной проверки: так ``capture`` убирает с телефона файлы, которые успел записать
    даже провальный прогон. Его пояснения — в заметки, проблемы — в провал.
    Кончились строки без результата — ``EXIT_NO_RESULT`` (таймаут или обрыв logcat).
    """
    check = CHECKS.get(scenario)
    if check is None:
        return SmokeVerdict(EXIT_USAGE, f"нет ожиданий для сценария {scenario!r}; известны: {sorted(CHECKS)}")
    observed = ObservedLog()
    crashes = CrashAttribution(package)
    for line in lines:
        crash_header = crashes.feed(line)
        if crash_header is not None:
            return SmokeVerdict(EXIT_SCENARIO_FAILED, f"процесс упал до строки результата: {crash_header}")
        if PANIC_MARKER in line:
            observed.panic_lines.append(line)
        stage_payload = extract_stage_payload(line)
        if stage_payload is not None:
            _collect_detail(stage_payload, scenario, details)
            stage_failure = _handle_stage_line(stage_payload, scenario, stage_handler)
            if stage_failure is not None:
                return stage_failure
            continue
        payload = extract_result_payload(line)
        if payload is None:
            continue
        try:
            result = json.loads(payload)
        except json.JSONDecodeError as error:
            return SmokeVerdict(EXIT_SCENARIO_FAILED, f"строка результата не JSON ({error}): {payload}")
        if not isinstance(result, dict):
            return SmokeVerdict(EXIT_SCENARIO_FAILED, f"строка результата не объект JSON: {payload}")
        if result.get("scenario") != scenario:
            continue
        problems = check(result, observed)
        if not problems and post_check is not None:
            outcome = post_check(result)
            problems = outcome.problems
            notes.extend(outcome.notes)
        if cleanup is not None:
            cleaned = cleanup(result)
            problems = [*problems, *cleaned.problems]
            notes.extend(cleaned.notes)
        if problems:
            return SmokeVerdict(EXIT_SCENARIO_FAILED, f"сценарий {scenario} не прошёл: {'; '.join(problems)}\n{payload}")
        notes.extend(NOTES.get(scenario, lambda _result: [])(result))
        return SmokeVerdict(EXIT_OK, f"сценарий {scenario} прошёл: {payload}")
    return SmokeVerdict(EXIT_NO_RESULT, f"нет строки {RESULT_MARKER.strip()} сценария {scenario}")


def _handle_stage_line(payload: str, scenario: str, stage_handler: StageHandler | None) -> SmokeVerdict | None:
    """Разбирает строку стадии и отдаёт её обработчику. ``None`` — продолжать чтение потока."""
    try:
        stage_event = json.loads(payload)
    except json.JSONDecodeError as error:
        return SmokeVerdict(EXIT_SCENARIO_FAILED, f"строка стадии не JSON ({error}): {payload}")
    if not isinstance(stage_event, dict):
        return SmokeVerdict(EXIT_SCENARIO_FAILED, f"строка стадии не объект JSON: {payload}")
    if stage_event.get("scenario") != scenario or stage_handler is None:
        return None
    try:
        stage_handler(stage_event)
    except DriverError as error:
        return SmokeVerdict(EXIT_SCENARIO_FAILED, f"сценарий {scenario}, стадия {stage_event.get('stage')!r}: {error}")
    return None


def read_logcat_lines(serial: str, since_epoch: str, timeout_seconds: float) -> Iterator[str]:
    """Строки logcat телефона с момента ``since_epoch``, пока не истечёт ``timeout_seconds``.

    ``-T`` вместо очистки буфера: ``logcat -c`` меняло бы состояние системы.
    Обрыв беспроводного ``adb`` посреди прогона не кончает ожидание: ``smoke_logcat`` ждёт
    устройство и продолжает с последней увиденной метки (дефект сессии 07, стенд V4).
    """
    return smoke_logcat.resumable_lines(
        open_segment=lambda cursor_epoch, deadline: _logcat_segment(serial, cursor_epoch, deadline),
        wait_for_device=lambda wait_seconds: _wait_for_device(serial, wait_seconds),
        since_epoch=since_epoch,
        deadline=time.monotonic() + timeout_seconds,
    )


def _logcat_segment(serial: str, since_epoch: str, deadline: float) -> Iterator[str]:
    """Один подпроцесс ``adb logcat`` с метки ``since_epoch`` до конца его вывода или до ``deadline``.

    Строки читает отдельный поток блокирующим ``readline`` и кладёт в очередь, а генератор
    ждёт очередь с дедлайном. Ожидать готовности дескриптора через ``select`` нельзя:
    ``readline`` забирает в буфер Python сразу пачку строк, дескриптор пустеет, и ``select``
    ждал бы до таймаута, хотя строка результата уже прочитана (дефект сессии 01).
    Подпроцесс гарантированно завершается при выходе из генератора.
    """
    command = [
        "adb", "-s", serial, "logcat", "-v", "epoch", "-T", since_epoch,
        "-s", "Fasticamera:V", "AndroidRuntime:E",
    ]
    process = subprocess.Popen(command, stdout=subprocess.PIPE, text=True, errors="replace")
    try:
        yield from _lines_until_deadline(process, deadline)
    finally:
        process.kill()
        process.wait()


def _wait_for_device(serial: str, wait_seconds: float) -> bool:
    """``adb wait-for-device`` не дольше ``wait_seconds``: вернулось ли устройство в состояние ``device``."""
    try:
        completed = subprocess.run(
            ["adb", "-s", serial, "wait-for-device"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=wait_seconds,
            check=False,
        )
    except subprocess.TimeoutExpired:
        return False
    return completed.returncode == 0


_END_OF_STREAM = None
"""Метка в очереди: logcat закрыл вывод."""


def _lines_until_deadline(process: subprocess.Popen[str], deadline: float) -> Iterator[str]:
    """Отдаёт строки ``process.stdout`` до конца потока или до ``deadline`` (time.monotonic)."""
    line_queue: queue.Queue[str | None] = queue.Queue()

    def pump_lines() -> None:
        assert process.stdout is not None  # stdout=PIPE задан в read_logcat_lines
        for line in process.stdout:
            line_queue.put(line)
        line_queue.put(_END_OF_STREAM)

    # daemon: поток не держит интерпретатор, если дедлайн истёк раньше конца потока.
    threading.Thread(target=pump_lines, name="logcat-reader", daemon=True).start()
    while (remaining := deadline - time.monotonic()) > 0:
        try:
            line = line_queue.get(timeout=remaining)
        except queue.Empty:
            return
        if line is _END_OF_STREAM:
            return
        yield line


SINCE_EPOCH_PATTERN = re.compile(r"^[0-9]+\.[0-9]{3}$")
"""Формат ``logcat -T``: секунды эпохи и ровно три цифры миллисекунд."""


def parse_since_epoch(text: str) -> str:
    """Точка отсчёта logcat с миллисекундами; секунды без дробной части — ошибка вызова.

    Целые секунды недопустимы: прогон, начатый в ту же секунду, что и конец предыдущего,
    прочитал бы его строку результата (дефект сессии 03).
    """
    if not SINCE_EPOCH_PATTERN.match(text):
        raise argparse.ArgumentTypeError(f"ожидалось «секунды.миллисекунды», получено {text!r}")
    return text


def parse_arguments(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--serial", required=True, help="serial устройства adb")
    parser.add_argument(
        "--since-epoch",
        type=parse_since_epoch,
        required=True,
        help="время устройства до запуска, «секунды.миллисекунды» эпохи",
    )
    parser.add_argument("--package", required=True, help="имя пакета приложения (нужно шагам драйвера)")
    parser.add_argument("--scenario", required=True, help="имя сценария")
    parser.add_argument("--timeout", type=float, required=True, help="сколько секунд ждать результата")
    parser.add_argument(
        "--artifacts-dir",
        type=Path,
        default=None,
        help="каталог для скриншотов (вне репозитория); нужен сценариям raw-stream* и viewfinder",
    )
    parser.add_argument(
        "--keep-captures",
        action="store_true",
        help="capture: не удалять с телефона файлы снимков этого прогона (оставить владельцу посмотреть)",
    )
    parser.add_argument(
        "--neutral-card",
        action="store_true",
        help="viewfinder и 3a: проверить по скриншотам, что серая карта в центре кадра вышла серой",
    )
    return parser.parse_args(argv)


SCREENSHOT_SCENARIOS = frozenset({"raw-stream", "raw-stream-endurance", "viewfinder", "3a", "glass"})
"""Сценарии, на стадии ``viewfinder_live`` (``3a`` — ``awb_ready``, ``glass`` — ``glass_live``) которых драйвер делает скриншоты и после результата их анализирует."""


NEUTRAL_CARD_SCENARIOS = frozenset({"viewfinder", "3a"})
"""Сценарии, для которых работает флаг ``--neutral-card``."""


def screen_post_check(artifacts_dir: Path) -> PostCheck:
    """Пост-проверка скриншотов видоискателя: «не чёрный» и «меняется»; числа идут пояснением при любом исходе."""

    def check(_result: dict) -> CheckOutcome:
        analysis = smoke_screen.screen_post_check(artifacts_dir)
        return CheckOutcome(analysis.problems, analysis.notes)

    return check


def with_neutral_card(post_check: PostCheck | None, artifacts_dir: Path) -> PostCheck:
    """Добавляет к пост-проверке ``viewfinder`` «серая карта в центре вышла серой» (флаг ``--neutral-card``)."""

    def check(result: dict) -> CheckOutcome:
        base = post_check(result) if post_check is not None else CheckOutcome([])
        card = smoke_neutral_card.neutral_card_post_check(artifacts_dir)
        return CheckOutcome([*base.problems, *card.problems], [*base.notes, *card.notes])

    return check


def post_check_for(scenario: str, serial: str, artifacts_dir: Path | None = None) -> PostCheck | None:
    """Проверка после результата, если сценарию она нужна: ``camera-catalog`` сверяется с dumpsys,
    ``raw-stream*`` и ``viewfinder`` — со скриншотами в каталоге артефактов."""
    if scenario == "camera-catalog":
        return dumpsys_post_check(serial)
    if scenario in SCREENSHOT_SCENARIOS:
        if artifacts_dir is None:
            return lambda _result: CheckOutcome(["каталог артефактов не задан (--artifacts-dir): скриншоты проверить нечем"])
        return screen_post_check(artifacts_dir)
    return None


def capture_post_check_for(scenario: str, serial: str, package: str) -> PostCheck | None:
    """Проверка файлов снимков (``capture``): каждый адрес отчёта — в MediaStore, на диске ПК и в ``ImageDecoder``."""
    if scenario != "capture":
        return None
    return smoke_checks_capture_files.capture_files_post_check(serial, package)


def cleanup_for(scenario: str, serial: str, keep_captures: bool, package: str) -> PostCheck | None:
    """Уборка после сценария: ``capture`` удаляет с телефона свои файлы, если не просили оставить."""
    if scenario != "capture":
        return None
    return smoke_checks_capture_files.capture_cleanup(serial, keep=keep_captures, package=package)


def _finish_driver(driver: StageDriver, verdict: SmokeVerdict) -> SmokeVerdict:
    """Дожидается фоновых нажатий драйвера. Сбой нажатия не маскирует уже вынесенный вердикт: он дописывается к нему,
    а успешный вердикт из-за него становится провалом (замер на не удержанном пальце недостоверен)."""
    try:
        driver.finish()
    except DriverError as error:
        if verdict.exit_code == EXIT_OK:
            return dataclasses.replace(
                verdict, exit_code=EXIT_SCENARIO_FAILED, message=f"фоновое нажатие драйвера: {error}\n{verdict.message}"
            )
        return dataclasses.replace(verdict, message=f"{verdict.message}\nфоновое нажатие драйвера: {error}")
    return verdict


def main(argv: list[str]) -> int:
    arguments = parse_arguments(argv)
    driver = StageDriver(
        arguments.scenario,
        AdbDeviceActions(arguments.serial, arguments.package, artifacts_dir=arguments.artifacts_dir),
    )
    post_check = post_check_for(arguments.scenario, arguments.serial, arguments.artifacts_dir)
    if post_check is None:
        post_check = capture_post_check_for(arguments.scenario, arguments.serial, arguments.package)
    if arguments.neutral_card and arguments.scenario in NEUTRAL_CARD_SCENARIOS and arguments.artifacts_dir is not None:
        post_check = with_neutral_card(post_check, arguments.artifacts_dir)
    verdict = evaluate_stream(
        read_logcat_lines(arguments.serial, arguments.since_epoch, arguments.timeout),
        arguments.scenario,
        driver.handle_stage,
        arguments.package,
        post_check,
        cleanup_for(arguments.scenario, arguments.serial, arguments.keep_captures, arguments.package),
    )
    verdict = _finish_driver(driver, verdict)
    if verdict.exit_code == EXIT_NO_RESULT:
        verdict = SmokeVerdict(verdict.exit_code, f"{verdict.message} за {arguments.timeout:g} с")
    stream = sys.stdout if verdict.exit_code == EXIT_OK else sys.stderr
    print(verdict.message, file=stream)
    for note in verdict.notes:
        print(note, file=stream)
    for detail in verdict.details:
        print(f"{DETAIL_LINE_MARKER} {json.dumps(detail, ensure_ascii=False)}", file=stream)
    return verdict.exit_code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
