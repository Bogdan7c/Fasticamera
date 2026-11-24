"""Чистый разбор вывода телефона для журнала тепла: текст → значения. Без ввода-вывода и без часов.

Один замер — один ``adb shell`` с составной командой (``build_sample_command``). Каждая величина печатается
после маркера ``@@имя``, поэтому мусор, ``Permission denied`` и пропавшая строка портят только свою величину
(она станет ``None`` и посчитается как отказ чтения), а не весь замер. Исключение — оборванный вывод
(нет завершающего ``@@end``): обрезанное число ``3740`` вместо ``37400`` выглядело бы правдоподобно, поэтому
такой замер целиком отвергается (``SampleParseError``).
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field

from thermal_log_config import (
    BATTERY_DECI_CELSIUS_PER_CELSIUS,
    GPU_BUSY_PATH,
    MILLI_CELSIUS_PER_CELSIUS,
    THERMAL_ZONE_GLOB,
)

MARKER = "@@"
"""Префикс служебных строк в выводе составной команды."""

END_MARKER = "@@end"
"""Последняя строка полного вывода; её отсутствие = обрыв."""


class SampleParseError(ValueError):
    """Замер нельзя использовать целиком (обрыв вывода, нет времени телефона)."""


@dataclass(frozen=True)
class ParsedSample:
    """Разобранный замер. ``None`` в поле = «не прочитано / нет данных»."""

    phone_epoch: float
    zone_temps_c: dict[str, float | None]
    gpu_busy_fraction: float | None
    thermal_status: int | None
    battery_temp_c: float | None
    battery_level_percent: int | None
    external_power: bool | None
    read_failures: int
    """Сколько величин не удалось прочитать (``gpubusy`` «0 0» — это «нет данных», а не отказ)."""


def build_zone_discovery_command() -> str:
    """Команда обхода зон: по строке ``<каталог зоны> <type>`` (ошибка чтения ``type`` печатается как ``cat: ...``)."""
    return f'for zone in {THERMAL_ZONE_GLOB}; do echo "$zone $(cat "$zone/type" 2>&1)"; done'


def parse_zone_listing(listing: str) -> dict[str, str]:
    """Имя зоны (из ``type``) → каталог зоны. Дубликаты имён: побеждает первая зона; нечитаемые типы пропускаются."""
    zone_dirs_by_name: dict[str, str] = {}
    for line in listing.splitlines():
        zone_dir, separator, zone_type = line.strip().partition(" ")
        zone_type = zone_type.strip()
        if not separator or not re.search(r"thermal_zone\d+$", zone_dir):
            continue
        if not zone_type or zone_type.startswith("cat:") or "Permission denied" in zone_type:
            continue
        zone_dirs_by_name.setdefault(zone_type, zone_dir)
    return zone_dirs_by_name


def build_sample_command(zone_dirs_by_name: dict[str, str]) -> str:
    """Одна составная команда sh: время телефона, температуры нужных зон, gpubusy, thermalservice, battery."""
    lines = ['echo "@@time $(date +%s.%N)"']
    for zone_name, zone_dir in zone_dirs_by_name.items():
        lines.append(f'echo "@@zone {zone_name} $(cat {zone_dir}/temp 2>&1)"')
    lines.append(f'echo "@@gpubusy $(cat {GPU_BUSY_PATH} 2>&1)"')
    lines.append('echo "@@thermal"')
    lines.append("dumpsys thermalservice 2>&1 | grep -m1 'Thermal Status'")
    lines.append('echo "@@battery"')
    lines.append("dumpsys battery 2>&1")
    lines.append(f'echo "{END_MARKER}"')
    return "; ".join(lines)


@dataclass
class _Sections:
    """Вывод, разложенный по маркерам."""

    time_text: str | None = None
    zone_texts: dict[str, str] = field(default_factory=dict)
    gpubusy_text: str | None = None
    thermal_lines: list[str] = field(default_factory=list)
    battery_lines: list[str] = field(default_factory=list)
    complete: bool = False


def _split_sections(output: str) -> _Sections:
    sections = _Sections()
    current_multiline: list[str] | None = None
    for raw_line in output.splitlines():
        line = raw_line.rstrip("\r")
        if not line.startswith(MARKER):
            if current_multiline is not None:
                current_multiline.append(line)
            continue
        name, _, rest = line[len(MARKER) :].partition(" ")
        current_multiline = None
        if name == "time":
            sections.time_text = rest.strip()
        elif name == "zone":
            zone_name, _, zone_text = rest.partition(" ")
            sections.zone_texts[zone_name] = zone_text.strip()
        elif name == "gpubusy":
            sections.gpubusy_text = rest.strip()
        elif name == "thermal":
            current_multiline = sections.thermal_lines
        elif name == "battery":
            current_multiline = sections.battery_lines
        elif name == "end":
            sections.complete = True
    return sections


def parse_zone_temp_c(text: str | None) -> float | None:
    """Милли-градусы (``37400``) → градусы; всё остальное (``cat: ... Permission denied``, пусто) → ``None``."""
    if text is None or not re.fullmatch(r"-?\d+", text.strip()):
        return None
    return int(text.strip()) / MILLI_CELSIUS_PER_CELSIUS


def parse_gpu_busy(text: str | None) -> tuple[float | None, bool]:
    """``gpubusy`` «занято всего» → (доля, прочитано_ли). «0 0» — окно без данных: (None, True), не ноль и не отказ."""
    match = re.fullmatch(r"(\d+)\s+(\d+)", (text or "").strip())
    if match is None:
        return None, False
    busy, total = int(match.group(1)), int(match.group(2))
    if total == 0:
        return None, True
    return busy / total, True


def _key_values(lines: list[str]) -> dict[str, str]:
    """Строки ``  ключ: значение`` → словарь (первое вхождение ключа)."""
    values: dict[str, str] = {}
    for line in lines:
        key, separator, value = line.strip().partition(":")
        if separator:
            values.setdefault(key.strip(), value.strip())
    return values


def _parse_int(text: str | None) -> int | None:
    if text is not None and re.fullmatch(r"-?\d+", text.strip()):
        return int(text.strip())
    return None


def parse_thermal_status(lines: list[str]) -> int | None:
    """Число из строки ``Thermal Status: N``."""
    return _parse_int(_key_values(lines).get("Thermal Status"))


@dataclass(frozen=True)
class BatteryReading:
    temperature_c: float | None
    level_percent: int | None
    external_power: bool | None
    """Любой из ``AC/USB/Wireless powered: true``; ``None``, если ни одной строки про питание нет."""


def parse_battery(lines: list[str]) -> BatteryReading:
    """Температура (десятые доли °C), уровень и признак внешнего питания из ``dumpsys battery``."""
    values = _key_values(lines)
    deci = _parse_int(values.get("temperature"))
    power_flags = [values[key] for key in ("AC powered", "USB powered", "Wireless powered") if key in values]
    external_power = None if not power_flags else any(flag == "true" for flag in power_flags)
    return BatteryReading(
        temperature_c=None if deci is None else deci / BATTERY_DECI_CELSIUS_PER_CELSIUS,
        level_percent=_parse_int(values.get("level")),
        external_power=external_power,
    )


def parse_sample(output: str, zone_names: tuple[str, ...]) -> ParsedSample:
    """Разбирает вывод ``build_sample_command``. ``zone_names`` — все зоны, что должны быть в записи (нет → ``None``)."""
    sections = _split_sections(output)
    if not sections.complete:
        raise SampleParseError("вывод оборван: нет завершающего маркера")
    try:
        phone_epoch = float(sections.time_text or "")
    except ValueError as error:
        raise SampleParseError(f"нет времени телефона в выводе: {sections.time_text!r}") from error

    zone_temps = {name: parse_zone_temp_c(sections.zone_texts.get(name)) for name in zone_names}
    gpu_busy, gpu_read_ok = parse_gpu_busy(sections.gpubusy_text)
    thermal_status = parse_thermal_status(sections.thermal_lines)
    battery = parse_battery(sections.battery_lines)

    read_failures = sum(1 for value in zone_temps.values() if value is None)
    read_failures += 0 if gpu_read_ok else 1
    read_failures += 1 if thermal_status is None else 0
    read_failures += sum(
        1 for value in (battery.temperature_c, battery.level_percent, battery.external_power) if value is None
    )
    return ParsedSample(
        phone_epoch=phone_epoch,
        zone_temps_c=zone_temps,
        gpu_busy_fraction=gpu_busy,
        thermal_status=thermal_status,
        battery_temp_c=battery.temperature_c,
        battery_level_percent=battery.level_percent,
        external_power=battery.external_power,
        read_failures=read_failures,
    )
