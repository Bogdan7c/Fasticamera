#!/usr/bin/env python3
"""Ожидание и проверка строки результата smoke-сценария Fasticamera в logcat.

Вызывается из ``scripts/device-smoke.sh`` после запуска Activity. Читает logcat телефона
(только тег ``Fasticamera`` и падения ``AndroidRuntime``) с момента запуска, ждёт строку
``FASTICAMERA_RESULT {json}`` нужного сценария и сверяет её с ожиданиями сценария.

Устройство:
* ``evaluate_stream`` — чистая логика над последовательностью строк (тестируется на ПК);
* ``CHECKS`` — ожидания каждого сценария (что именно считается «прошёл»);
* ``read_logcat_lines`` — ввод: подпроцесс ``adb logcat`` с дедлайном;
* ``main`` — разбор аргументов и вывод итога.

Коды выхода совпадают с ``scripts/lib/common.sh``.
"""

from __future__ import annotations

import argparse
import json
import queue
import subprocess
import sys
import threading
import time
from collections.abc import Callable, Iterable, Iterator
from dataclasses import dataclass, field

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


CHECKS: dict[str, ScenarioCheck] = {
    "hello": check_hello,
    "panic": check_panic,
}


def extract_result_payload(line: str) -> str | None:
    """JSON из строки logcat с маркером результата или ``None``."""
    marker_position = line.find(RESULT_MARKER)
    if marker_position < 0:
        return None
    return line[marker_position + len(RESULT_MARKER) :].strip()


def evaluate_stream(lines: Iterable[str], scenario: str) -> SmokeVerdict:
    """Читает строки до результата нужного сценария (или падения процесса) и выносит вердикт.

    Строки результата других сценариев пропускаются: это хвост прошлого запуска.
    Кончились строки без результата — ``EXIT_NO_RESULT`` (таймаут или обрыв logcat).
    """
    check = CHECKS.get(scenario)
    if check is None:
        return SmokeVerdict(EXIT_USAGE, f"нет ожиданий для сценария {scenario!r}; известны: {sorted(CHECKS)}")
    observed = ObservedLog()
    for line in lines:
        if CRASH_MARKER in line:
            return SmokeVerdict(EXIT_SCENARIO_FAILED, f"процесс упал до строки результата: {line.strip()}")
        if PANIC_MARKER in line:
            observed.panic_lines.append(line)
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


def read_logcat_lines(serial: str, since_epoch: int, timeout_seconds: float) -> Iterator[str]:
    """Строки logcat телефона с момента ``since_epoch``, пока не истечёт ``timeout_seconds``.

    ``-T`` вместо очистки буфера: ``logcat -c`` меняло бы состояние системы.

    Строки читает отдельный поток блокирующим ``readline`` и кладёт в очередь, а генератор
    ждёт очередь с дедлайном. Ожидать готовности дескриптора через ``select`` нельзя:
    ``readline`` забирает в буфер Python сразу пачку строк, дескриптор пустеет, и ``select``
    ждал бы до таймаута, хотя строка результата уже прочитана (дефект сессии 01).
    Подпроцесс гарантированно завершается при выходе из генератора.
    """
    command = [
        "adb", "-s", serial, "logcat", "-v", "epoch", "-T", f"{since_epoch}.000",
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


def parse_arguments(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--serial", required=True, help="serial устройства adb")
    parser.add_argument("--since-epoch", type=int, required=True, help="время устройства до запуска, секунды эпохи")
    parser.add_argument("--scenario", required=True, help="имя сценария")
    parser.add_argument("--timeout", type=float, required=True, help="сколько секунд ждать результата")
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    arguments = parse_arguments(argv)
    verdict = evaluate_stream(
        read_logcat_lines(arguments.serial, arguments.since_epoch, arguments.timeout),
        arguments.scenario,
    )
    if verdict.exit_code == EXIT_NO_RESULT:
        verdict = SmokeVerdict(verdict.exit_code, f"{verdict.message} за {arguments.timeout:g} с")
    stream = sys.stdout if verdict.exit_code == EXIT_OK else sys.stderr
    print(verdict.message, file=stream)
    return verdict.exit_code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
