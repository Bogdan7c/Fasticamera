"""Сверка каталога камер из отчёта ``camera-catalog`` с ``dumpsys media.camera`` телефона.

Каталог строит Rust из ``CameraCharacteristics`` через NDK; ``dumpsys media.camera`` печатает те же
характеристики со стороны системной службы. Расхождение значит, что наш разбор или фильтр видимости
что-то потерял или выдумал. Что сверяется:

* множество ID камер, видимых приложению: HAL-устройства без возможности ``SYSTEM_CAMERA`` ==
  камеры отчёта; скрытые (на OnePlus 11 это 4 и 5) обязаны **отсутствовать** в отчёте (инвариант 6 плана);
* направление камеры (``android.lens.facing``) каждой общей камеры;
* RAW-конфигурации выхода (``android.scaler.availableStreamConfigurations``, форматы 0x20 RAW16 и
  0x25 RAW10, направление ``OUTPUT``): множество (формат, ширина, высота) каждой общей камеры.

Разбор — чистые функции над текстом (тестируются на синтетической фикстуре формата, без снимков
с реального телефона). Ввод-вывод — только ``dumpsys_post_check``.
"""

from __future__ import annotations

import re
from collections.abc import Callable
from dataclasses import dataclass, field

from smoke_driver import AdbResult, AdbRunner, DriverError, run_adb_subprocess

RAW_FORMAT_NAMES = {32: "raw16", 37: "raw10"}
"""Коды ``ImageFormat`` → имена форматов отчёта: 0x20 = RAW_SENSOR (RAW16), 0x25 = RAW10."""

SYSTEM_CAMERA_CAPABILITY = "SYSTEM_CAMERA"
"""Возможность, которой фреймворк закрывает камеру от обычных приложений."""

HAL_HEADER = re.compile(r"^== Camera HAL device (?P<name>\S+) \(v[\d.]+\) static information: ==\s*$")
"""Начало раздела характеристик одного HAL-устройства; ID камеры — последний сегмент имени."""

SECTION_HEADER = re.compile(r"^== ")
"""Любой заголовок раздела dumpsys (всегда в нулевой колонке)."""

FACING_KEY = re.compile(r"^\s+android\.lens\.facing \(")
CAPABILITIES_KEY = re.compile(r"^\s+android\.request\.availableCapabilities \(")
STREAM_CONFIGURATIONS_KEY = re.compile(r"^\s+android\.scaler\.availableStreamConfigurations \(")
"""Ключи характеристик. Точное имя и скобка: ``...MaximumResolution`` — другой ключ, его не берём."""

STREAM_CONFIGURATION_ROW = re.compile(r"^\s+\[(?P<format>\d+) (?P<width>\d+) (?P<height>\d+) (?P<direction>INPUT|OUTPUT) \]\s*$")
VALUE_ROW = re.compile(r"^\s+\[(?P<values>[^\]]*)\]\s*$")

RawConfiguration = tuple[str, int, int]
"""(имя формата, ширина, высота)."""


@dataclass
class DumpsysCamera:
    """Что dumpsys знает об одном HAL-устройстве."""

    camera_id: str
    facing: str | None = None
    capabilities: set[str] = field(default_factory=set)
    raw_outputs: set[RawConfiguration] = field(default_factory=set)

    @property
    def is_system_camera(self) -> bool:
        """Камера закрыта от обычных приложений (``SYSTEM_CAMERA``)."""
        return SYSTEM_CAMERA_CAPABILITY in self.capabilities


def _value_tokens(line: str) -> list[str]:
    """Слова внутри ``[ ... ]`` строки значений (``[BACK ]`` -> ``["BACK"]``); не строка значений — пусто."""
    match = VALUE_ROW.match(line)
    return match.group("values").split() if match else []


def parse_dumpsys_camera(text: str) -> dict[str, DumpsysCamera]:
    """ID камеры → характеристики по тексту ``dumpsys media.camera``.

    Берутся разделы ``Camera HAL device ... static information``. Внутри раздела значения ключей идут
    следующими строками: направление и возможности — одной строкой ``[...]``, конфигурации потоков —
    рядом строк ``[формат ширина высота INPUT|OUTPUT ]``.
    """
    cameras: dict[str, DumpsysCamera] = {}
    current: DumpsysCamera | None = None
    lines = text.splitlines()
    index = 0
    while index < len(lines):
        line = lines[index]
        header = HAL_HEADER.match(line)
        if header is not None:
            camera_id = header.group("name").rsplit("/", 1)[-1]
            current = cameras.setdefault(camera_id, DumpsysCamera(camera_id))
        elif SECTION_HEADER.match(line):
            current = None
        elif current is not None:
            index = _read_key(lines, index, current)
        index += 1
    return cameras


def _read_key(lines: list[str], index: int, camera: DumpsysCamera) -> int:
    """Читает значение ключа, если строка ``index`` — один из нужных ключей. Возвращает индекс последней
    использованной строки (чтобы основной цикл не разбирал её повторно)."""
    line = lines[index]
    if FACING_KEY.match(line) and index + 1 < len(lines):
        tokens = _value_tokens(lines[index + 1])
        camera.facing = tokens[0].lower() if tokens else None
        return index + 1
    if CAPABILITIES_KEY.match(line) and index + 1 < len(lines):
        camera.capabilities = set(_value_tokens(lines[index + 1]))
        return index + 1
    if STREAM_CONFIGURATIONS_KEY.match(line):
        last = index
        for offset in range(index + 1, len(lines)):
            row = STREAM_CONFIGURATION_ROW.match(lines[offset])
            if row is None:
                break
            last = offset
            raw_name = RAW_FORMAT_NAMES.get(int(row.group("format")))
            if raw_name is not None and row.group("direction") == "OUTPUT":
                camera.raw_outputs.add((raw_name, int(row.group("width")), int(row.group("height"))))
        return last
    return index


def _report_raw_streams(camera: dict) -> set[RawConfiguration]:
    """RAW-потоки камеры из отчёта; запись неверной формы даёт заведомо несовпадающий элемент."""
    streams: set[RawConfiguration] = set()
    for stream in camera.get("raw_streams") or []:
        if isinstance(stream, dict):
            streams.add((str(stream.get("format")), int(stream.get("width", -1)), int(stream.get("height", -1))))
    return streams


def _format_streams(streams: set[RawConfiguration]) -> str:
    return ", ".join(f"{name} {width}x{height}" for name, width, height in sorted(streams)) or "нет"


def compare_catalog(report_cameras: list[dict], dumpsys: dict[str, DumpsysCamera]) -> list[str]:
    """Расхождения между каталогом отчёта и dumpsys (пустой список — совпадает)."""
    if not dumpsys:
        return ["dumpsys media.camera не содержит ни одного раздела HAL-устройства: формат вывода изменился?"]
    problems: list[str] = []
    reported = {str(camera.get("id")): camera for camera in report_cameras if isinstance(camera, dict)}
    visible = {camera_id for camera_id, camera in dumpsys.items() if not camera.is_system_camera}
    for camera_id in sorted(visible - reported.keys()):
        problems.append(f"камера {camera_id} видна приложению по dumpsys, но отсутствует в каталоге")
    for camera_id in sorted(reported.keys() - visible):
        if camera_id in dumpsys:
            problems.append(
                f"камера {camera_id} закрыта ({SYSTEM_CAMERA_CAPABILITY}), но попала в каталог: инвариант 6 нарушен"
            )
        else:
            problems.append(f"камера {camera_id} есть в каталоге, но dumpsys о ней не знает")
    for camera_id in sorted(visible & reported.keys()):
        problems.extend(_compare_camera(camera_id, reported[camera_id], dumpsys[camera_id]))
    return problems


def _compare_camera(camera_id: str, reported: dict, expected: DumpsysCamera) -> list[str]:
    problems: list[str] = []
    if str(reported.get("facing")) != expected.facing:
        problems.append(
            f"камера {camera_id}: направление в каталоге {reported.get('facing')!r}, по dumpsys {expected.facing!r}"
        )
    reported_raw = _report_raw_streams(reported)
    if reported_raw != expected.raw_outputs:
        problems.append(
            f"камера {camera_id}: RAW-потоки в каталоге [{_format_streams(reported_raw)}], "
            f"по dumpsys [{_format_streams(expected.raw_outputs)}]"
        )
    return problems


def describe_match(report_cameras: list[dict], dumpsys: dict[str, DumpsysCamera]) -> str:
    """Доказательная строка успешной сверки: что именно сошлось (иначе молчаливый успех ничего не доказывает).

    Пример: ``сверка с dumpsys: видимые ID [0, 1, 2, 3] совпали; скрытые [4, 5] (SYSTEM_CAMERA) в каталоге
    отсутствуют; RAW-конфигураций совпало 9 (0: 3, 1: 4, 2: 2); направления совпали``.
    """
    reported = {str(camera.get("id")): camera for camera in report_cameras if isinstance(camera, dict)}
    visible = sorted(camera_id for camera_id, camera in dumpsys.items() if not camera.is_system_camera)
    hidden = sorted(camera_id for camera_id, camera in dumpsys.items() if camera.is_system_camera)
    per_camera = {camera_id: len(dumpsys[camera_id].raw_outputs) for camera_id in visible if camera_id in reported}
    total = sum(per_camera.values())
    breakdown = ", ".join(f"{camera_id}: {count}" for camera_id, count in per_camera.items())
    hidden_text = f"скрытые [{', '.join(hidden)}] ({SYSTEM_CAMERA_CAPABILITY}) в каталоге отсутствуют" if hidden else "скрытых камер нет"
    return (
        f"сверка с dumpsys: видимые ID [{', '.join(visible)}] совпали; {hidden_text}; "
        f"RAW-конфигураций совпало {total} ({breakdown}); направления совпали"
    )


@dataclass(frozen=True)
class CheckOutcome:
    """Итог дополнительной проверки результата: нарушения и пояснения для человека (печатаются при любом исходе)."""

    problems: list[str]
    notes: list[str] = field(default_factory=list)


PostCheck = Callable[[dict], CheckOutcome]
"""Дополнительная проверка результата сценария: (JSON результата) → нарушения и пояснения."""

DUMPSYS_COMMAND_TAIL = ("shell", "dumpsys", "media.camera")


def read_dumpsys(serial: str, run_adb: AdbRunner = run_adb_subprocess) -> str:
    """Снимает ``dumpsys media.camera``; сбой команды — ``DriverError``."""
    result: AdbResult = run_adb(["adb", "-s", serial, *DUMPSYS_COMMAND_TAIL])
    if result.returncode != 0:
        raise DriverError(f"dumpsys media.camera завершился с кодом {result.returncode}: {result.stderr.strip()}")
    return result.stdout


def dumpsys_post_check(serial: str, run_adb: AdbRunner = run_adb_subprocess) -> PostCheck:
    """Проверка результата ``camera-catalog``: снимает dumpsys и сверяет с каталогом из отчёта."""

    def check(result: dict) -> CheckOutcome:
        report = result.get("report")
        cameras = report.get("cameras") if isinstance(report, dict) else None
        if not isinstance(cameras, list):
            return CheckOutcome(["в отчёте нет списка cameras: сверять с dumpsys нечего"])
        try:
            text = read_dumpsys(serial, run_adb)
        except DriverError as error:
            return CheckOutcome([str(error)])
        parsed = parse_dumpsys_camera(text)
        problems = compare_catalog(cameras, parsed)
        notes = [] if problems or not parsed else [describe_match(cameras, parsed)]
        return CheckOutcome(problems, notes)

    return check
