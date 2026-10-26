#!/usr/bin/env python3
"""Ожидание и проверка строки результата smoke-сценария Fasticamera в logcat.

Вызывается из ``scripts/device-smoke.sh`` после запуска Activity. Читает logcat телефона
(только тег ``Fasticamera`` и падения ``AndroidRuntime``) с момента запуска, ждёт строку
``FASTICAMERA_RESULT {json}`` нужного сценария и сверяет её с ожиданиями сценария.

Устройство:
* ``evaluate_stream`` — логика над последовательностью строк (тестируется на ПК); на строках
  стадий ``FASTICAMERA_STAGE`` вызывает обработчик (драйвер шагов из ``smoke_driver``);
* ``CHECKS`` — ожидания каждого сценария (что именно считается «прошёл»);
* ``read_logcat_lines`` — ввод: подпроцесс ``adb logcat`` с дедлайном;
* ``main`` — разбор аргументов и вывод итога.

Коды выхода совпадают с ``scripts/lib/common.sh``.
"""

from __future__ import annotations

import argparse
import json
import queue
import re
import subprocess
import sys
import threading
import time
from collections.abc import Callable, Iterable, Iterator
from dataclasses import dataclass, field

import smoke_checks
from smoke_driver import STAGE_MARKER, AdbDeviceActions, DriverError, StageDriver

EXIT_OK = 0
EXIT_SCENARIO_FAILED = 1
EXIT_NO_RESULT = 3
EXIT_USAGE = 64

RESULT_MARKER = "FASTICAMERA_RESULT "
"""Префикс строки результата; контракт с ``crates/fasticamera-app/src/result_line.rs``."""

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


@dataclass(frozen=True)
class SmokeVerdict:
    """Итог проверки: код выхода и объяснение для человека."""

    exit_code: int
    message: str


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
}

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
) -> SmokeVerdict:
    """Читает строки до результата нужного сценария (или падения процесса) и выносит вердикт.

    Строки результата и стадии других сценариев пропускаются: это хвост прошлого запуска.
    Стадии нужного сценария передаются ``stage_handler`` (если задан) по мере прихода; пока
    обработчик занят (пауза, опрос диалога), строки копятся в очереди чтения и не теряются.
    Сбой шага (``DriverError``) — провал сценария с объяснением. Падение процесса считается
    только для ``package`` (см. ``CrashAttribution``); падения чужих приложений игнорируются.
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
        if problems:
            return SmokeVerdict(EXIT_SCENARIO_FAILED, f"сценарий {scenario} не прошёл: {'; '.join(problems)}\n{payload}")
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
        yield from _lines_until_deadline(process, time.monotonic() + timeout_seconds)
    finally:
        process.kill()
        process.wait()


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
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    arguments = parse_arguments(argv)
    driver = StageDriver(arguments.scenario, AdbDeviceActions(arguments.serial, arguments.package))
    verdict = evaluate_stream(
        read_logcat_lines(arguments.serial, arguments.since_epoch, arguments.timeout),
        arguments.scenario,
        driver.handle_stage,
        arguments.package,
    )
    if verdict.exit_code == EXIT_NO_RESULT:
        verdict = SmokeVerdict(verdict.exit_code, f"{verdict.message} за {arguments.timeout:g} с")
    stream = sys.stdout if verdict.exit_code == EXIT_OK else sys.stderr
    print(verdict.message, file=stream)
    return verdict.exit_code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
