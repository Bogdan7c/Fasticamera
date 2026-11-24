#!/usr/bin/env python3
"""Журнал тепла телефона с ПК: пишет температуры/нагрузку GPU раз в N секунд и делает сводку по интервалам.

    python3 scripts/device_thermal_log.py record --out /путь/вне/репозитория/thermal.jsonl [--period-s 5] [--duration-s N]
    python3 scripts/device_thermal_log.py summarize thermal.jsonl [--from-epoch T0 --to-epoch T1] [--windows-json окна.json] [--json]

Всё читается без root одним ``adb shell`` на замер. Время замера — часы ТЕЛЕФОНА (как ``logcat -v epoch``).
Серийник и IP в файл не попадают (в заголовке только модель). Обрыв adb не роняет запись: пишется строка
``{"kind": "gap", ...}``, замеры продолжаются с тем же периодом.

Слои: ввод-вывод (adb, файл, часы) — этот модуль; разбор — ``thermal_log_parse``; сводка и вывод —
``thermal_log_summary``; параметры — ``thermal_log_config``.
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import IO

import thermal_log_config as config
import thermal_log_summary as summary
from thermal_log_parse import (
    ParsedSample,
    SampleParseError,
    build_sample_command,
    build_zone_discovery_command,
    parse_sample,
    parse_zone_listing,
)


class ThermalLogError(RuntimeError):
    """Запись невозможна (устройство не найдено, зоны не определены)."""


@dataclass(frozen=True)
class AdbResult:
    returncode: int
    stdout: str
    stderr: str


AdbRunner = Callable[[list[str]], AdbResult]
"""Выполнитель команд: полная командная строка (с ``adb``) → результат. Тесты подставляют фейк."""


def run_adb_subprocess(command: list[str]) -> AdbResult:
    """Настоящий выполнитель. Зависание (обрыв беспроводного adb) = код 124 и пояснение, а не исключение."""
    try:
        completed = subprocess.run(
            command, capture_output=True, text=True, errors="replace", timeout=config.ADB_TIMEOUT_SECONDS
        )
    except subprocess.TimeoutExpired:
        return AdbResult(124, "", f"adb не ответил за {config.ADB_TIMEOUT_SECONDS:g} с")
    except OSError as error:
        return AdbResult(127, "", f"не удалось запустить adb: {error}")
    return AdbResult(completed.returncode, completed.stdout, completed.stderr)


def device_model() -> str:
    return os.environ.get("FASTICAMERA_DEVICE_MODEL", config.DEFAULT_DEVICE_MODEL)


def find_device_serial(run_adb: AdbRunner, environment: dict[str, str] | None = None) -> str:
    """``ANDROID_SERIAL`` или единственный телефон нужной модели из ``adb devices -l`` (как ``find_device_serial``)."""
    env = os.environ if environment is None else environment
    if env.get("ANDROID_SERIAL"):
        return env["ANDROID_SERIAL"]
    result = run_adb(["adb", "devices", "-l"])
    if result.returncode != 0:
        raise ThermalLogError(f"adb devices не сработал: {result.stderr.strip()}")
    model = env.get("FASTICAMERA_DEVICE_MODEL", config.DEFAULT_DEVICE_MODEL)
    serials = [
        fields[0]
        for fields in (line.split() for line in result.stdout.splitlines())
        if len(fields) > 1 and fields[1] == "device" and f"model:{model}" in fields
    ]
    if len(serials) != 1:
        raise ThermalLogError(f"нужен ровно один телефон модели {model}, найдено {len(serials)}; задайте ANDROID_SERIAL")
    return serials[0]


def scrub_serial(text: str, serial: str) -> str:
    """Убирает серийник из текста причины (в файл он попасть не должен)."""
    return text.replace(serial, "<serial>")


def header_record(period_seconds: float, model: str, zone_names_found: list[str]) -> dict:
    """Первая строка файла. Серийника и IP здесь нет по построению."""
    return {
        "kind": "header",
        "format_version": config.FORMAT_VERSION,
        "period_s": period_seconds,
        "device_model": model,
        "zones_found": zone_names_found,
        "zones_missing": [name for name in config.TRACKED_ZONES if name not in zone_names_found],
    }


def sample_record(parsed: ParsedSample) -> dict:
    return {
        "kind": "sample",
        "t_phone": parsed.phone_epoch,
        "temps_c": parsed.zone_temps_c,
        "gpu_busy_fraction": parsed.gpu_busy_fraction,
        "thermal_status": parsed.thermal_status,
        "battery_temp_c": parsed.battery_temp_c,
        "battery_level_percent": parsed.battery_level_percent,
        "external_power": parsed.external_power,
        "read_failures": parsed.read_failures,
    }


def gap_record(reason: str, phone_time_estimate: float | None) -> dict:
    """Строка-пропуск; время телефона оценивается от последнего удачного замера (None, если его ещё не было)."""
    return {"kind": "gap", "t_phone_estimate": phone_time_estimate, "reason": reason}


def discover_zones(run_adb: AdbRunner, serial: str) -> dict[str, str]:
    """Один раз при старте: имя нужной зоны → её каталог. Пустой результат — ошибка (нечего писать)."""
    result = run_adb(["adb", "-s", serial, "shell", build_zone_discovery_command()])
    if result.returncode != 0:
        raise ThermalLogError(f"не удалось обойти тепловые зоны: {scrub_serial(result.stderr.strip(), serial)}")
    all_zones = parse_zone_listing(result.stdout)
    wanted = {name: all_zones[name] for name in config.TRACKED_ZONES if name in all_zones}
    if not wanted:
        raise ThermalLogError("ни одной из нужных тепловых зон не найдено")
    return wanted


class ThermalRecorder:
    """Цикл записи: замер → строка JSONL, обрыв → строка-пропуск. Часы и ожидание подставляются (тесты)."""

    def __init__(
        self,
        run_adb: AdbRunner,
        serial: str,
        zone_dirs_by_name: dict[str, str],
        output: IO[str],
        monotonic: Callable[[], float] = time.monotonic,
    ) -> None:
        self._run_adb = run_adb
        self._serial = serial
        self._command = build_sample_command(zone_dirs_by_name)
        self._output = output
        self._monotonic = monotonic
        self._last_phone_epoch: float | None = None
        self._last_phone_monotonic: float | None = None

    def _write(self, record: dict) -> None:
        self._output.write(json.dumps(record, ensure_ascii=False) + "\n")
        self._output.flush()  # запись должна пережить аварийное завершение сценария

    def _estimated_phone_time(self) -> float | None:
        if self._last_phone_epoch is None or self._last_phone_monotonic is None:
            return None
        return self._last_phone_epoch + (self._monotonic() - self._last_phone_monotonic)

    def take_sample(self) -> dict:
        """Один замер; всегда пишет ровно одну строку (замер или пропуск) и возвращает её."""
        result = self._run_adb(["adb", "-s", self._serial, "shell", self._command])
        record: dict
        if result.returncode != 0:
            reason = scrub_serial(result.stderr.strip() or result.stdout.strip() or "adb вернул ошибку", self._serial)
            record = gap_record(f"adb код {result.returncode}: {reason}", self._estimated_phone_time())
        else:
            try:
                parsed = parse_sample(result.stdout, config.TRACKED_ZONES)
            except SampleParseError as error:
                record = gap_record(f"разбор: {error}", self._estimated_phone_time())
            else:
                self._last_phone_epoch = parsed.phone_epoch
                self._last_phone_monotonic = self._monotonic()
                record = sample_record(parsed)
        self._write(record)
        return record

    def write_header(self, period_seconds: float, model: str, zone_names_found: list[str]) -> None:
        self._write(header_record(period_seconds, model, zone_names_found))

    def run(
        self,
        period_seconds: float,
        duration_seconds: float | None,
        stop_requested: Callable[[float], bool],
    ) -> int:
        """Замеры с постоянным шагом (без накопления дрейфа). ``stop_requested(timeout)`` ждёт до timeout секунд и
        возвращает True, если просили остановиться. Возвращает число записанных строк."""
        started = self._monotonic()
        written = 0
        while True:
            written += 1
            self.take_sample()
            next_tick = started + written * period_seconds
            if duration_seconds is not None and next_tick - started > duration_seconds:
                return written
            if stop_requested(max(0.0, next_tick - self._monotonic())):
                return written


def run_record(args: argparse.Namespace, run_adb: AdbRunner = run_adb_subprocess) -> int:
    if args.period_s < config.MIN_PERIOD_SECONDS:
        print(f"период не меньше {config.MIN_PERIOD_SECONDS:g} с", file=sys.stderr)
        return 2
    try:
        serial = find_device_serial(run_adb)
        zones = discover_zones(run_adb, serial)
    except ThermalLogError as error:
        print(f"ошибка: {error}", file=sys.stderr)
        return 2
    stop_event = threading.Event()
    for signum in (signal.SIGINT, signal.SIGTERM):
        signal.signal(signum, lambda _signum, _frame: stop_event.set())
    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    with out_path.open("w", encoding="utf-8") as output:
        recorder = ThermalRecorder(run_adb, serial, zones, output)
        recorder.write_header(args.period_s, device_model(), list(zones))
        written = recorder.run(args.period_s, args.duration_s, stop_event.wait)
    print(f"записано строк: {written} -> {out_path}", file=sys.stderr)
    return 0


def run_summarize(args: argparse.Namespace) -> int:
    try:
        _header, records = summary.read_log(Path(args.log))
        windows = summary.windows_from_options(
            args.from_epoch, args.to_epoch, Path(args.windows_json) if args.windows_json else None
        )
    except summary.SummaryError as error:
        print(f"ошибка: {error}", file=sys.stderr)
        return 2
    summaries = summary.summarize(records, windows)
    print(summary.summaries_to_json(summaries) if args.json else summary.render_markdown(summaries))
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    record = commands.add_parser("record", help="писать журнал, пока идёт сценарий (до Ctrl+C/SIGTERM или --duration-s)")
    record.add_argument("--out", required=True, help="файл .jsonl (вне репозитория)")
    record.add_argument("--period-s", type=float, default=config.DEFAULT_PERIOD_SECONDS)
    record.add_argument("--duration-s", type=float, default=None)
    summarize = commands.add_parser("summarize", help="сводка по интервалам (таблица Markdown)")
    summarize.add_argument("log")
    summarize.add_argument("--from-epoch", type=float, default=None)
    summarize.add_argument("--to-epoch", type=float, default=None)
    summarize.add_argument("--windows-json", default=None)
    summarize.add_argument("--json", action="store_true", help="вывести JSON вместо Markdown")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    return run_record(args) if args.command == "record" else run_summarize(args)


if __name__ == "__main__":
    sys.exit(main())
