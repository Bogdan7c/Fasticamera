"""Файлы снимков smoke-сценария ``capture``: проверка на ПК и уборка с телефона (сессия 14).

Сценарий передаёт в строке результата список адресов файлов, которые записала служба снимков (``report.uris``).
Здесь, уже ПОСЛЕ результата:

1. ``capture_files_post_check`` — по каждому адресу: строка MediaStore (``content query``): ``is_pending = 0``,
   ``relative_path = DCIM/Camera/``, MIME, размеры; файл скачивается (``exec-out content read``) во ВРЕМЕННЫЙ каталог вне
   репозитория и разбирается: HEIC — ``heif-info`` (размер, плитки); DNG — заголовок TIFF с тегом ``DNGVersion``,
   ``exiftool -validate`` и rawpy (недоступный инструмент — явная заметка, не провал); основной файл — системный
   ``ImageDecoder`` (приёмник есть только в debug-сборке; в release — заметка).
2. ``capture_cleanup`` — удаляет ТОЛЬКО эти адреса (``content delete``) при любом исходе прогона и при любой
   проверке; ``--keep-captures`` оставляет файлы владельцу (адреса печатаются).

Безопасность удаления (единственное необратимое действие): адрес обязан совпасть со строгим шаблоном ``SAFE_URI``,
принадлежать отчёту ЭТОГО прогона, а строка MediaStore — лежать в ``DCIM/Camera/``. Иначе ничего не удаляется.

Формат вывода ``content query`` и семантика ``content delete`` на телефоне не проверены до сессии 14c: разбор строг и
при неожиданном ответе даёт явную проблему/заметку, а не молчание.
"""

from __future__ import annotations

import json
import re
import shutil
import struct
import subprocess
import tempfile
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from smoke_checks_capture import SAVED_WITH_RAW_OUTCOME, saved_shots
from smoke_driver import AdbBytesRunner, AdbRunner, DriverError, run_adb_subprocess, run_adb_subprocess_bytes
from smoke_dumpsys import CheckOutcome, PostCheck

SAFE_URI = re.compile(r"^content://media/external(?:_primary)?/images/media/[0-9]+$")
"""Адреса, которые вообще можно трогать: картинка внешнего хранилища с числовым номером. Общий вид
``.../file/N`` не годится: через него адресуется любой файл хранилища, а наши снимки — всегда картинки.
Любое другое — не проверяется и не удаляется."""

OUR_FILE_NAME = re.compile(r"^IMG_\d{8}_\d{6}_0?\d{3}\.(?:HEIC|JPG|DNG)$")
"""Имя файла, которое даёт приложение (``StillFileNames``: ``IMG_ГГГГММДД_ЧЧММСС_ммм`` + расширение).

Необязательный ``0`` перед миллисекундами — факт 14c: галерея OnePlus (``com.oneplus.gallery``) после записи
переименовывает часть наших снимков (первый снимок прогона: ``…_886.HEIC`` → ``…_0886.HEIC``, файл тот же). Без
этого варианта уборка оставляла такой снимок в галерее владельца. Наш ли файл, по-прежнему решают владелец-пакет
и папка ``DCIM/Camera``."""

EXPECTED_RELATIVE_PATH = "DCIM/Camera"
"""Папка снимков (без завершающей косой: MediaStore отдаёт её с ней, разбор терпит оба вида)."""

QUERY_PROJECTION = "_display_name:mime_type:width:height:relative_path:is_pending:owner_package_name"

PRIMARY_MIME = {"heif": ("image/heic", "image/heif"), "jpeg": ("image/jpeg",)}
RAW_MIME = ("image/x-adobe-dng", "image/dng")
"""MIME, которыми MediaStore может назвать наши файлы."""

DNG_VERSION_TAG = 50706
"""Тег ``DNGVersion`` в IFD0 (спецификация DNG 1.7, раздел «DNGVersion»)."""

TIFF_HEADER_LENGTH = 8
IFD_ENTRY_LENGTH = 12

DECODE_RECEIVER = "dev.fasticamera.DebugImageDecodeReceiver"
"""Приёмник debug-сборки (``android/app/src/debug``): декодирует файл системным ``ImageDecoder``."""

NO_RESULT_ANSWER = "No result found"
"""Что отвечает ``content query``, когда строки нет."""

TOOL_TIMEOUT_SECONDS = 120.0

HEIF_IMAGE_LINE = re.compile(r"^image:\s*(?P<width>\d+)x(?P<height>\d+)\b.*\bprimary\b", re.MULTILINE)
HEIF_TILES_LINE = re.compile(r"^\s*tiles:\s*(?P<columns>\d+)x(?P<rows>\d+),\s*tile size:\s*(?P<tile_width>\d+)x(?P<tile_height>\d+)", re.MULTILINE)

EXIFTOOL_COUNT = re.compile(r"(?P<count>\d+)\s+(?P<kind>errors?|warnings?)", re.IGNORECASE)


# ---- Разбор строки MediaStore ---------------------------------------------------------------------------------


@dataclass(frozen=True)
class MediaRow:
    """Строка MediaStore одного файла; ``None`` — у файла нет значения (``NULL``)."""

    display_name: str | None
    mime_type: str | None
    width: int | None
    height: int | None
    relative_path: str | None
    is_pending: int | None
    owner_package_name: str | None = None
    """Пакет, создавший запись (``owner_package_name``): по нему отличаем свои файлы от чужих."""


ROW_FIELD = re.compile(r"(?P<key>[A-Za-z_]+)=(?P<value>.*?)(?=, [A-Za-z_]+=|$)")


def _optional_int(text: str | None) -> int | None:
    if text is None or not re.fullmatch(r"-?\d+", text):
        return None
    return int(text)


def parse_content_query(text: str) -> MediaRow | None:
    """Строка ``Row: 0 _display_name=..., mime_type=..., ...`` → ``MediaRow``; нет строки ``Row:`` — ``None``."""
    for line in text.splitlines():
        match = re.match(r"^Row:\s*\d+\s+(?P<fields>.*)$", line.strip())
        if match is None:
            continue
        fields = {item.group("key"): item.group("value").strip() for item in ROW_FIELD.finditer(match.group("fields"))}
        fields = {key: (None if value == "NULL" else value) for key, value in fields.items()}
        return MediaRow(
            display_name=fields.get("_display_name"),
            mime_type=fields.get("mime_type"),
            width=_optional_int(fields.get("width")),
            height=_optional_int(fields.get("height")),
            relative_path=fields.get("relative_path"),
            is_pending=_optional_int(fields.get("is_pending")),
            owner_package_name=fields.get("owner_package_name"),
        )
    return None


@dataclass(frozen=True)
class RowQuery:
    """Ответ на запрос строки: сама строка (``None`` — файла нет), сырой текст и удался ли запрос вообще."""

    row: MediaRow | None
    raw_answer: str
    succeeded: bool


def query_media_row(serial: str, uri: str, run_adb: AdbRunner) -> RowQuery:
    """Строка MediaStore адреса. Адрес обязан быть безопасным (проверяет вызывающий)."""
    result = run_adb(["adb", "-s", serial, "shell", "content", "query", "--uri", uri, "--projection", QUERY_PROJECTION])
    text = (result.stdout + result.stderr).strip()
    if result.returncode != 0:
        return RowQuery(None, text, succeeded=False)
    return RowQuery(parse_content_query(result.stdout), text, succeeded=True)


# ---- Ожидания ------------------------------------------------------------------------------------------------


@dataclass(frozen=True)
class FileExpectation:
    """Что ждём от файла по адресу: чей он снимок, вид, MIME и размеры (``None`` — размер не сверяется)."""

    uri: str
    shot: int
    kind: str  # "primary" | "raw"
    mimes: tuple[str, ...]
    size: tuple[int, int] | None
    size_note: str | None = None


def expected_size(shot: dict, full_frame: object) -> tuple[tuple[int, int] | None, str | None]:
    """Размеры основной картинки по кадрированию из отчёта: полный кадр сенсора при 4:3 и 1×; иначе — не сверяем."""
    if not isinstance(full_frame, dict):
        return None, "полный кадр не указан в отчёте"
    width, height = full_frame.get("width_px"), full_frame.get("height_px")
    if not isinstance(width, int) or not isinstance(height, int):
        return None, "полный кадр в отчёте не числами"
    if shot.get("aspect") == "4:3" and shot.get("zoom_milli") == 1000:
        return (width, height), None
    return None, f"кадрирование {shot.get('aspect')} при зуме {shot.get('zoom_milli')} тыс.: размер не сверяется"


def expectations_of(report: dict) -> list[FileExpectation]:
    """Адреса отчёта в порядке снимков: у ``saved`` один файл, у ``saved_with_raw`` — основной и DNG."""
    uris = report.get("uris")
    if not isinstance(uris, list):
        return []
    result: list[FileExpectation] = []
    cursor = 0
    for shot in saved_shots(report):
        fmt = shot.get("fmt") if shot.get("fmt") in PRIMARY_MIME else "heif"
        size, note = expected_size(shot, report.get("full_frame"))
        if cursor < len(uris):
            result.append(FileExpectation(str(uris[cursor]), int(shot.get("n", 0)), "primary", PRIMARY_MIME[fmt], size, note))
        cursor += 1
        if shot.get("outcome") == SAVED_WITH_RAW_OUTCOME:
            if cursor < len(uris):
                full = report.get("full_frame")
                raw_size = (full["width_px"], full["height_px"]) if isinstance(full, dict) and isinstance(full.get("width_px"), int) and isinstance(full.get("height_px"), int) else None
                result.append(FileExpectation(str(uris[cursor]), int(shot.get("n", 0)), "raw", RAW_MIME, raw_size, None))
            cursor += 1
    return result


# ---- Инструменты ПК -------------------------------------------------------------------------------------------


@dataclass(frozen=True)
class ToolResult:
    returncode: int
    stdout: str
    stderr: str


ToolRunner = Callable[[list[str]], ToolResult]


def run_tool_subprocess(command: list[str]) -> ToolResult:
    """Настоящий запуск инструмента ПК с потолком времени (зависший инструмент — провал проверки, не зависание)."""
    try:
        completed = subprocess.run(command, capture_output=True, text=True, errors="replace", timeout=TOOL_TIMEOUT_SECONDS, check=False)
    except subprocess.TimeoutExpired:
        return ToolResult(124, "", f"инструмент завис (> {TOOL_TIMEOUT_SECONDS:g} с): {' '.join(command)}")
    except OSError as error:
        return ToolResult(127, "", f"инструмент не запустился: {error}")
    return ToolResult(completed.returncode, completed.stdout, completed.stderr)


@dataclass(frozen=True)
class Tools:
    """Какие инструменты ПК доступны. ``None`` — нет, проверка пропускается явной заметкой."""

    heif_info: str | None
    exiftool: Path | None
    rawpy_python: Path | None


def discover_tools(cache_dir: Path | None = None, which: Callable[[str], str | None] = shutil.which) -> Tools:
    """Ищет ``heif-info`` в PATH, ``exiftool`` и rawpy в ``~/.cache/fasticamera`` (как остальной стенд)."""
    root = cache_dir if cache_dir is not None else Path.home() / ".cache" / "fasticamera"
    exiftool = next(iter(sorted((root / "tools" / "exiftool").glob("Image-ExifTool-*/exiftool"))), None)
    rawpy_python = root / "isp-reference-venv" / "bin" / "python"
    return Tools(
        heif_info=which("heif-info"),
        exiftool=exiftool if exiftool is not None and which("perl") else None,
        rawpy_python=rawpy_python if rawpy_python.exists() else None,
    )


RAWPY_SCRIPT = (
    "import json, sys, rawpy\n"
    "with rawpy.imread(sys.argv[1]) as raw:\n"
    "    height, width = raw.raw_image.shape\n"
    "    print(json.dumps({'width': width, 'height': height}))\n"
)


# ---- Разбор файлов -------------------------------------------------------------------------------------------


def tiff_ifd0_tags(data: bytes) -> set[int] | None:
    """Теги IFD0 файла TIFF/DNG; ``None`` — это не TIFF или заголовок/каталог повреждены (TIFF 6.0, раздел 2)."""
    if len(data) < TIFF_HEADER_LENGTH or data[:2] not in (b"II", b"MM"):
        return None
    order = "<" if data[:2] == b"II" else ">"
    magic, offset = struct.unpack_from(f"{order}HI", data, 2)
    if magic != 42 or offset + 2 > len(data):
        return None
    (count,) = struct.unpack_from(f"{order}H", data, offset)
    end = offset + 2 + count * IFD_ENTRY_LENGTH
    if end > len(data):
        return None
    return {struct.unpack_from(f"{order}H", data, offset + 2 + index * IFD_ENTRY_LENGTH)[0] for index in range(count)}


def check_heic_file(path: Path, expected: tuple[int, int] | None, tools: Tools, run_tool: ToolRunner) -> CheckOutcome:
    """HEIC через ``heif-info``: главная картинка нужного размера, плитки в заметке."""
    if tools.heif_info is None:
        return CheckOutcome([], ["capture: heif-info не найден: HEIC не разобран на ПК (установите libheif)"])
    result = run_tool([tools.heif_info, str(path)])
    if result.returncode != 0:
        return CheckOutcome([f"heif-info не открыл {path.name}: код {result.returncode}: {(result.stderr or result.stdout).strip()[:200]}"])
    image = HEIF_IMAGE_LINE.search(result.stdout)
    if image is None:
        return CheckOutcome([f"heif-info не показал главную картинку {path.name}: {result.stdout.strip()[:200]!r}"])
    size = (int(image.group("width")), int(image.group("height")))
    problems: list[str] = []
    if expected is not None and size not in (expected, (expected[1], expected[0])):
        problems.append(f"HEIC {path.name}: размер {size[0]}x{size[1]}, ожидали {expected[0]}x{expected[1]}")
    tiles = HEIF_TILES_LINE.search(result.stdout)
    tiles_text = (
        f"плитки {tiles.group('columns')}x{tiles.group('rows')} по {tiles.group('tile_width')}x{tiles.group('tile_height')}"
        if tiles
        else "плиток нет (одна картинка)"
    )
    return CheckOutcome(problems, [f"capture: HEIC {path.name}: heif-info {size[0]}x{size[1]}, {tiles_text}"])


def check_dng_file(path: Path, data: bytes, expected: tuple[int, int] | None, tools: Tools, run_tool: ToolRunner) -> CheckOutcome:
    """DNG: заголовок TIFF с ``DNGVersion``, затем ``exiftool -validate`` и rawpy (если есть)."""
    tags = tiff_ifd0_tags(data)
    if tags is None:
        return CheckOutcome([f"DNG {path.name}: нет корректного заголовка TIFF"])
    if DNG_VERSION_TAG not in tags:
        return CheckOutcome([f"DNG {path.name}: в IFD0 нет тега DNGVersion ({DNG_VERSION_TAG})"])
    problems: list[str] = []
    notes = [f"capture: DNG {path.name}: заголовок TIFF верен, DNGVersion на месте (тегов в IFD0: {len(tags)})"]
    if tools.exiftool is None:
        notes.append("capture: exiftool или perl не найден: DNG не проверен exiftool -validate")
    else:
        result = run_tool(["perl", str(tools.exiftool), "-validate", "-a", "-s", str(path)])
        problems.extend(_exiftool_problems(path, result))
        notes.append(f"capture: DNG {path.name}: exiftool -validate: {_one_line(result.stdout) or 'пусто'}")
    if tools.rawpy_python is None:
        notes.append("capture: rawpy (isp-reference-venv) не найден: DNG не открыт rawpy")
    else:
        result = run_tool([str(tools.rawpy_python), "-I", "-c", RAWPY_SCRIPT, str(path)])
        if result.returncode != 0:
            problems.append(f"rawpy не открыл {path.name}: {(result.stderr or result.stdout).strip()[-200:]}")
        else:
            notes.append(f"capture: DNG {path.name}: rawpy открыл, {result.stdout.strip()}")
            problems.extend(_rawpy_size_problems(path, result.stdout, expected))
    return CheckOutcome(problems, notes)


def _one_line(text: str) -> str:
    return " | ".join(line.strip() for line in text.splitlines() if line.strip())[:300]


def _exiftool_problems(path: Path, result: ToolResult) -> list[str]:
    """Ошибки ``exiftool -validate``: ненулевой код или счётчик/строка ``Error``; предупреждения — только в заметке."""
    if result.returncode != 0:
        return [f"exiftool не отработал на {path.name}: код {result.returncode}: {(result.stderr or result.stdout).strip()[:200]}"]
    problems: list[str] = []
    for line in result.stdout.splitlines():
        key, _, value = line.partition(":")
        if key.strip().lower() == "error":
            problems.append(f"exiftool: ошибка в {path.name}: {value.strip()}")
        elif key.strip().lower() == "validate":
            for count in EXIFTOOL_COUNT.finditer(value):
                if count.group("kind").lower().startswith("error") and int(count.group("count")) > 0:
                    problems.append(f"exiftool -validate: ошибок {count.group('count')} в {path.name}")
    return problems


def _rawpy_size_problems(path: Path, stdout: str, expected: tuple[int, int] | None) -> list[str]:
    try:
        size = json.loads(stdout)
    except json.JSONDecodeError:
        return [f"rawpy: непонятный ответ для {path.name}: {stdout.strip()[:100]!r}"]
    if expected is None or not isinstance(size, dict):
        return []
    actual = (size.get("width"), size.get("height"))
    if actual not in (expected, (expected[1], expected[0])):
        return [f"DNG {path.name}: rawpy видит {actual[0]}x{actual[1]}, ожидали {expected[0]}x{expected[1]}"]
    return []


# ---- Системный ImageDecoder -----------------------------------------------------------------------------------

BROADCAST_DATA = re.compile(r'data="(?P<json>.*)"\s*$', re.MULTILINE)


def system_decode(serial: str, package: str, uri: str, run_adb: AdbRunner) -> dict | None:
    """Ответ приёмника ``ImageDecoder``; ``None`` — приёмник не ответил (его нет в сборке)."""
    result = run_adb(
        ["adb", "-s", serial, "shell", "am", "broadcast", "-n", f"{package}/{DECODE_RECEIVER}", "--es", "uri", uri]
    )
    match = BROADCAST_DATA.search(result.stdout)
    if result.returncode != 0 or match is None:
        return None
    try:
        answer = json.loads(match.group("json"))
    except json.JSONDecodeError:
        return {"status": "failed", "error": f"приёмник вернул не JSON: {match.group('json')[:100]!r}"}
    return answer if isinstance(answer, dict) else None


def check_system_decode(
    serial: str, package: str, expectation: FileExpectation, run_adb: AdbRunner
) -> CheckOutcome:
    """«HEIF декодируется системным ImageDecoder»: ответ приёмника debug-сборки, размеры и пространство цвета."""
    if not package.endswith(".debug"):
        return CheckOutcome([], ["capture: ImageDecoder не проверен: приёмник есть только в debug-сборке (пакет без .debug)"])
    answer = system_decode(serial, package, expectation.uri, run_adb)
    if answer is None:
        return CheckOutcome([f"ImageDecoder: приёмник {DECODE_RECEIVER} не ответил (сборка без него?) на {expectation.uri}"])
    if answer.get("status") != "decoded":
        return CheckOutcome([f"ImageDecoder не открыл {expectation.uri}: {answer.get('error', answer)!r}"])
    size = (answer.get("width"), answer.get("height"))
    problems: list[str] = []
    expected = expectation.size
    if expected is not None and size not in (expected, (expected[1], expected[0])):
        problems.append(f"ImageDecoder: {expectation.uri} декодирован как {size[0]}x{size[1]}, ожидали {expected[0]}x{expected[1]}")
    note = f"capture: ImageDecoder: снимок {expectation.shot} {size[0]}x{size[1]}, пространство цвета {answer.get('color_space')}, {answer.get('bitmap_config')}"
    return CheckOutcome(problems, [note])


# ---- Проверка файлов ------------------------------------------------------------------------------------------


def row_problems(expectation: FileExpectation, row: MediaRow | None, raw_answer: str) -> list[str]:
    """Строка MediaStore против ожиданий: файл виден, готов (не pending), в DCIM/Camera, тот MIME и размер."""
    label = f"снимок {expectation.shot} ({expectation.kind}) {expectation.uri}"
    if row is None:
        return [f"{label}: MediaStore не вернул строку (ответ: {raw_answer[:160]!r})"]
    problems: list[str] = []
    if row.is_pending != 0:
        problems.append(f"{label}: is_pending={row.is_pending!r}, ожидалось 0 (частичный файл виден в галерее)")
    if row.relative_path is None or row.relative_path.rstrip("/") != EXPECTED_RELATIVE_PATH:
        problems.append(f"{label}: relative_path={row.relative_path!r}, ожидалось {EXPECTED_RELATIVE_PATH}/")
    if row.mime_type not in expectation.mimes:
        problems.append(f"{label}: MIME {row.mime_type!r}, ожидалось одно из {list(expectation.mimes)}")
    if expectation.size is not None and row.width is not None and row.height is not None:
        size = (row.width, row.height)
        if size not in (expectation.size, (expectation.size[1], expectation.size[0])):
            problems.append(f"{label}: размеры {row.width}x{row.height}, ожидалось {expectation.size[0]}x{expectation.size[1]}")
    return problems


def capture_files_post_check(
    serial: str,
    package: str,
    run_adb: AdbRunner = run_adb_subprocess,
    run_adb_bytes: AdbBytesRunner = run_adb_subprocess_bytes,
    run_tool: ToolRunner = run_tool_subprocess,
    tools: Tools | None = None,
    make_temp_dir: Callable[[], str] = lambda: tempfile.mkdtemp(prefix="fasticamera-capture-"),
) -> PostCheck:
    """Пост-проверка ``capture``: каждый файл отчёта — в MediaStore, на диске ПК, в системном декодере."""

    def check(result: dict) -> CheckOutcome:
        report = result.get("report")
        if not isinstance(report, dict):
            return CheckOutcome(["в результате нет report: файлы проверить нечем"])
        expectations = expectations_of(report)
        if not expectations:
            return CheckOutcome(["в сводке нет адресов файлов: проверять нечего"])
        available = tools if tools is not None else discover_tools()
        problems: list[str] = []
        notes: list[str] = []
        workdir = Path(make_temp_dir())
        try:
            for expectation in expectations:
                outcome = _check_one_file(serial, package, expectation, workdir, available, run_adb, run_adb_bytes, run_tool)
                problems.extend(outcome.problems)
                notes.extend(outcome.notes)
        finally:
            shutil.rmtree(workdir, ignore_errors=True)
        notes.append(f"capture: проверено файлов {len(expectations)}")
        return CheckOutcome(problems, notes)

    return check


def _check_one_file(
    serial: str,
    package: str,
    expectation: FileExpectation,
    workdir: Path,
    tools: Tools,
    run_adb: AdbRunner,
    run_adb_bytes: AdbBytesRunner,
    run_tool: ToolRunner,
) -> CheckOutcome:
    if SAFE_URI.match(expectation.uri) is None:
        return CheckOutcome([f"адрес {expectation.uri!r} не похож на файл MediaStore: не трогаем"])
    query = query_media_row(serial, expectation.uri, run_adb)
    row = query.row
    problems = row_problems(expectation, row, query.raw_answer)
    notes: list[str] = []
    if expectation.size_note:
        notes.append(f"capture: снимок {expectation.shot}: {expectation.size_note}")
    if row is None:
        return CheckOutcome(problems, notes)
    suffix = ".dng" if expectation.kind == "raw" else (".heic" if "heic" in (row.mime_type or "") else ".bin")
    path = workdir / f"shot{expectation.shot}_{expectation.kind}{suffix}"
    try:
        data = _download(serial, expectation.uri, run_adb_bytes)
    except DriverError as error:
        return CheckOutcome([*problems, f"снимок {expectation.shot}: не скачался: {error}"], notes)
    path.write_bytes(data)
    notes.append(f"capture: снимок {expectation.shot} ({expectation.kind}): {row.display_name}, {row.mime_type}, {len(data)} байт, MediaStore {row.width}x{row.height}")
    if expectation.kind == "raw":
        outcome = check_dng_file(path, data, expectation.size, tools, run_tool)
    elif "heic" in (row.mime_type or "") or "heif" in (row.mime_type or ""):
        outcome = check_heic_file(path, expectation.size, tools, run_tool)
        decode = check_system_decode(serial, package, expectation, run_adb)
        outcome = CheckOutcome([*outcome.problems, *decode.problems], [*outcome.notes, *decode.notes])
    else:
        outcome = CheckOutcome([], [])
    return CheckOutcome([*problems, *outcome.problems], [*notes, *outcome.notes])


def _download(serial: str, uri: str, run_adb_bytes: AdbBytesRunner) -> bytes:
    result = run_adb_bytes(["adb", "-s", serial, "exec-out", "content", "read", "--uri", uri])
    if result.returncode != 0 or not result.stdout:
        raise DriverError(f"content read: код {result.returncode}, {len(result.stdout)} байт: {result.stderr.strip()[:160]}")
    return result.stdout


# ---- Уборка ---------------------------------------------------------------------------------------------------


def capture_cleanup(serial: str, keep: bool, package: str, run_adb: AdbRunner = run_adb_subprocess) -> PostCheck:
    """Удаляет с телефона файлы ЭТОГО прогона (или, при ``keep``, оставляет и печатает адреса). Проблем не создаёт:
    неудачная уборка — громкая заметка (галерея владельца засорена), но не провал сценария. ``package`` — пакет
    приложения: чужие по владельцу файлы не трогаем."""

    def cleanup(result: dict) -> CheckOutcome:
        report = result.get("report")
        uris = report.get("uris") if isinstance(report, dict) else None
        if not isinstance(uris, list) or not uris:
            return CheckOutcome([])
        reported = [uri for uri in uris if isinstance(uri, str)]
        if keep:
            return CheckOutcome([], [f"capture: файлы оставлены на телефоне (--keep-captures): {', '.join(reported)}"])
        deleted, left, skipped = [], [], []
        for uri in reported:
            verdict = _delete_one(serial, uri, package, run_adb)
            {"deleted": deleted, "left": left, "skipped": skipped}[verdict.kind].append(verdict.text)
        notes = [f"capture: удалено файлов {len(deleted)} из {len(reported)}"]
        for text in skipped:
            notes.append(f"capture: НЕ удалён (проверка безопасности): {text}")
        for text in left:
            notes.append(f"capture: ВНИМАНИЕ, файл остался в галерее: {text}")
        return CheckOutcome([], notes)

    return cleanup


@dataclass(frozen=True)
class DeleteVerdict:
    kind: str  # "deleted" | "left" | "skipped"
    text: str


def _delete_one(serial: str, uri: str, package: str, run_adb: AdbRunner) -> DeleteVerdict:
    """Удаляет один адрес, только если он безопасен, а строка MediaStore — НАША: владелец — пакет приложения, имя
    по шаблону приложения, папка DCIM/Camera/. Потом проверяет, что строки больше нет."""
    if SAFE_URI.fullmatch(uri) is None:
        return DeleteVerdict("skipped", f"{uri!r} не похож на картинку MediaStore")
    query = query_media_row(serial, uri, run_adb)
    if not query.succeeded:
        return DeleteVerdict("left", f"{uri}: MediaStore не ответил на запрос ({query.raw_answer[:120]!r}), удалять вслепую нельзя")
    row = query.row
    if row is None:
        # «Нет строки» — только когда MediaStore так и сказал; любой другой непонятный ответ не равен «удалено».
        if NO_RESULT_ANSWER in query.raw_answer:
            return DeleteVerdict("deleted", uri)
        return DeleteVerdict("left", f"{uri}: ответ MediaStore не разобран ({query.raw_answer[:120]!r}), удалять вслепую нельзя")
    if row.owner_package_name != package:
        return DeleteVerdict("skipped", f"{uri} принадлежит {row.owner_package_name!r}, а не {package}")
    if row.display_name is None or OUR_FILE_NAME.fullmatch(row.display_name) is None:
        return DeleteVerdict("skipped", f"{uri} называется {row.display_name!r}, не по шаблону имён приложения")
    if row.relative_path is None or row.relative_path.rstrip("/") != EXPECTED_RELATIVE_PATH:
        return DeleteVerdict("skipped", f"{uri} лежит в {row.relative_path!r}, а не в {EXPECTED_RELATIVE_PATH}/")
    result = run_adb(["adb", "-s", serial, "shell", "content", "delete", "--uri", uri])
    if result.returncode != 0:
        return DeleteVerdict("left", f"{uri}: content delete вернул {result.returncode}: {(result.stderr or result.stdout).strip()[:160]}")
    after = query_media_row(serial, uri, run_adb)
    if not after.succeeded or after.row is not None:
        return DeleteVerdict("left", f"{uri}: после удаления строка MediaStore ещё есть")
    return DeleteVerdict("deleted", uri)
