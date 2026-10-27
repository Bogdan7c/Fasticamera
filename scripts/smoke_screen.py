"""Анализ скриншотов видоискателя: не чёрный ли экран и меняется ли картинка.

``adb exec-out screencap`` (без ``-p``) отдаёт сырые пиксели: заголовок из беззнаковых 32-битных чисел
little-endian и сами пиксели. Заголовок бывает двух размеров (зависит от версии Android):

* 12 байт: ширина, высота, формат пикселя;
* 16 байт: то же плюс цветовое пространство.

Размер заголовка определяется не по версии системы, а по размеру данных: ``len(данные) - ширина * высота
* байт_на_пиксель``. Любое другое остаточное число — ошибка разбора (обрезанный вывод, неизвестный формат),
а не повод гадать.

Устройство (всё чистые функции, кроме ``load_thresholds`` и ``write_png``):
* ``parse_screencap`` — байты → ``Screenshot`` (RGBA);
* ``mean_luminance`` / ``mean_abs_difference`` — числа по центральной полосе;
* ``analyze_viewfinder`` — два снимка → нарушения и пояснения;
* ``screen_post_check`` — ввод-вывод: читает сохранённые драйвером снимки из каталога артефактов.

Скриншоты никогда не попадают в репозиторий: каталог артефактов задаёт ``device-smoke.sh``.
"""

from __future__ import annotations

import struct
import tomllib
import zlib
from dataclasses import dataclass
from pathlib import Path


THRESHOLDS_FILE = Path(__file__).resolve().parent / "smoke_screen.toml"
"""Пороги лежат рядом со скриптом."""

VIEWFINDER_LABELS = ("viewfinder_a", "viewfinder_b")
"""Метки двух снимков, которые драйвер делает на стадии ``viewfinder_live`` (контракт драйвера и пост-проверки)."""

CAPTURE_SUFFIX = ".screencap"
"""Расширение сохранённого сырого вывода ``screencap``."""

HEADER_SIZES = (12, 16)
"""Допустимые размеры заголовка: без и с цветовым пространством."""

PIXEL_FORMAT_RGBA_8888 = 1
PIXEL_FORMAT_RGBX_8888 = 2
PIXEL_FORMAT_BGRA_8888 = 5
"""Форматы ``PixelFormat`` Android, которые умеем читать (все по 4 байта на пиксель)."""

BYTES_PER_PIXEL = {PIXEL_FORMAT_RGBA_8888: 4, PIXEL_FORMAT_RGBX_8888: 4, PIXEL_FORMAT_BGRA_8888: 4}

MAX_SIDE_PIXELS = 20_000
"""Потолок стороны: заголовок с большим числом — мусор, а не экран."""


class ScreenError(Exception):
    """Скриншот не разобрать или проанализировать; сообщение уходит в вердикт."""


@dataclass(frozen=True)
class ScreenAnalysis:
    """Итог анализа: нарушения и пояснения для человека (печатаются при любом исходе)."""

    problems: list[str]
    notes: list[str]


@dataclass(frozen=True)
class Screenshot:
    """Кадр экрана: размеры и пиксели строго в порядке RGBA (по 4 байта)."""

    width: int
    height: int
    rgba: bytes
    header_size: int


@dataclass(frozen=True)
class ScreenThresholds:
    """Пороги и параметры выборки (см. ``smoke_screen.toml``)."""

    top_fraction: float
    bottom_fraction: float
    sample_step_pixels: int
    min_mean_luminance: float
    min_mean_abs_difference: float

    def __post_init__(self) -> None:
        if not 0.0 <= self.top_fraction < self.bottom_fraction <= 1.0:
            raise ScreenError(f"полоса анализа задана неверно: {self.top_fraction}..{self.bottom_fraction}")
        if self.sample_step_pixels < 1:
            raise ScreenError(f"sample_step_pixels должен быть не меньше 1, а не {self.sample_step_pixels}")
        if self.min_mean_luminance < 0 or self.min_mean_abs_difference < 0:
            raise ScreenError("пороги не могут быть отрицательными")


def parse_thresholds(text: str) -> ScreenThresholds:
    """Пороги из текста TOML; нет ключа — ``ScreenError`` с его именем."""
    try:
        document = tomllib.loads(text)
        band, limits = document["band"], document["thresholds"]
        return ScreenThresholds(
            top_fraction=float(band["top_fraction"]),
            bottom_fraction=float(band["bottom_fraction"]),
            sample_step_pixels=int(band["sample_step_pixels"]),
            min_mean_luminance=float(limits["min_mean_luminance"]),
            min_mean_abs_difference=float(limits["min_mean_abs_difference"]),
        )
    except tomllib.TOMLDecodeError as error:
        raise ScreenError(f"{THRESHOLDS_FILE.name}: не TOML ({error})") from error
    except KeyError as error:
        raise ScreenError(f"{THRESHOLDS_FILE.name}: нет ключа {error}") from error
    except (TypeError, ValueError) as error:
        raise ScreenError(f"{THRESHOLDS_FILE.name}: неверное значение ({error})") from error


def load_thresholds(path: Path = THRESHOLDS_FILE) -> ScreenThresholds:
    try:
        return parse_thresholds(path.read_text(encoding="utf-8"))
    except OSError as error:
        raise ScreenError(f"не прочитать {path}: {error}") from error


def parse_screencap(data: bytes) -> Screenshot:
    """Сырой вывод ``screencap`` → ``Screenshot``. Любое несоответствие размеров — ``ScreenError``."""
    if len(data) < min(HEADER_SIZES):
        raise ScreenError(f"вывод screencap слишком короткий: {len(data)} байт")
    width, height, pixel_format = struct.unpack_from("<III", data, 0)
    if not (0 < width <= MAX_SIDE_PIXELS and 0 < height <= MAX_SIDE_PIXELS):
        raise ScreenError(f"в заголовке screencap нелепый размер {width}x{height}: вывод не похож на кадр")
    bytes_per_pixel = BYTES_PER_PIXEL.get(pixel_format)
    if bytes_per_pixel is None:
        raise ScreenError(f"формат пикселя {pixel_format} не поддерживается (умеем {sorted(BYTES_PER_PIXEL)})")
    header_size = len(data) - width * height * bytes_per_pixel
    if header_size not in HEADER_SIZES:
        raise ScreenError(
            f"размер вывода {len(data)} байт не сходится с {width}x{height}x{bytes_per_pixel}: "
            f"остаток {header_size}, ожидался заголовок из {HEADER_SIZES}"
        )
    pixels = data[header_size:]
    if pixel_format == PIXEL_FORMAT_BGRA_8888:
        # Меняем местами красный и синий каналы: дальше везде RGBA.
        swapped = bytearray(pixels)
        swapped[0::4], swapped[2::4] = pixels[2::4], pixels[0::4]
        pixels = bytes(swapped)
    return Screenshot(width, height, pixels, header_size)


def _band_rows(screenshot: Screenshot, thresholds: ScreenThresholds) -> range:
    first = int(screenshot.height * thresholds.top_fraction)
    last = int(screenshot.height * thresholds.bottom_fraction)
    if last <= first:
        raise ScreenError(f"полоса анализа пуста при высоте {screenshot.height}")
    return range(first, last, thresholds.sample_step_pixels)


def _luminance_samples(screenshot: Screenshot, thresholds: ScreenThresholds) -> list[int]:
    """Яркости (0..255, веса Rec.709 в целых) пикселей выборки центральной полосы."""
    step = thresholds.sample_step_pixels
    rgba = screenshot.rgba
    row_bytes = screenshot.width * 4
    samples: list[int] = []
    for row in _band_rows(screenshot, thresholds):
        base = row * row_bytes
        for column in range(0, screenshot.width, step):
            offset = base + column * 4
            samples.append((54 * rgba[offset] + 183 * rgba[offset + 1] + 19 * rgba[offset + 2]) >> 8)
    return samples


def mean_luminance(screenshot: Screenshot, thresholds: ScreenThresholds) -> float:
    """Средняя яркость центральной полосы, шкала 0..255."""
    samples = _luminance_samples(screenshot, thresholds)
    return sum(samples) / len(samples)


def mean_abs_difference(first: Screenshot, second: Screenshot, thresholds: ScreenThresholds) -> float:
    """Средняя абсолютная разность яркости двух снимков по центральной полосе, шкала 0..255."""
    if (first.width, first.height) != (second.width, second.height):
        raise ScreenError(
            f"снимки разного размера: {first.width}x{first.height} и {second.width}x{second.height}"
        )
    samples_first = _luminance_samples(first, thresholds)
    samples_second = _luminance_samples(second, thresholds)
    return sum(abs(a - b) for a, b in zip(samples_first, samples_second, strict=True)) / len(samples_first)


def analyze_viewfinder(first: Screenshot, second: Screenshot, thresholds: ScreenThresholds) -> ScreenAnalysis:
    """Два снимка видоискателя → нарушения («чёрный», «замер») и пояснения с числами."""
    luminance_first = mean_luminance(first, thresholds)
    luminance_second = mean_luminance(second, thresholds)
    difference = mean_abs_difference(first, second, thresholds)
    problems: list[str] = []
    for label, value in zip(VIEWFINDER_LABELS, (luminance_first, luminance_second), strict=True):
        if value < thresholds.min_mean_luminance:
            problems.append(
                f"экран чёрный на снимке {label}: средняя яркость {value:.2f} < {thresholds.min_mean_luminance}"
            )
    if difference < thresholds.min_mean_abs_difference:
        problems.append(
            f"картинка не меняется между снимками: средняя разность {difference:.3f} < {thresholds.min_mean_abs_difference}"
        )
    notes = [
        f"скриншоты: яркость {luminance_first:.2f} и {luminance_second:.2f} (шкала 0..255), "
        f"разность {difference:.3f}, экран {first.width}x{first.height}, заголовок {first.header_size} байт"
    ]
    return ScreenAnalysis(problems, notes)


def write_png(path: Path, screenshot: Screenshot) -> None:
    """Сохраняет снимок как PNG (RGBA, 8 бит) — чтобы владелец мог открыть его глазами."""

    def chunk(kind: bytes, payload: bytes) -> bytes:
        body = kind + payload
        return struct.pack(">I", len(payload)) + body + struct.pack(">I", zlib.crc32(body))

    row_bytes = screenshot.width * 4
    raw_rows = b"".join(
        b"\x00" + screenshot.rgba[row * row_bytes : (row + 1) * row_bytes] for row in range(screenshot.height)
    )
    header = struct.pack(">IIBBBBB", screenshot.width, screenshot.height, 8, 6, 0, 0, 0)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(raw_rows, 6)) + chunk(b"IEND", b"")
    path.write_bytes(png)


def capture_path(artifacts_dir: Path, label: str) -> Path:
    """Куда драйвер кладёт сырой вывод ``screencap`` с меткой ``label``."""
    return artifacts_dir / f"{label}{CAPTURE_SUFFIX}"


def screen_post_check(artifacts_dir: Path, thresholds: ScreenThresholds | None = None) -> ScreenAnalysis:
    """Читает два снимка видоискателя из каталога артефактов и судит их."""
    try:
        limits = thresholds or load_thresholds()
        shots = []
        for label in VIEWFINDER_LABELS:
            path = capture_path(artifacts_dir, label)
            if not path.is_file():
                return ScreenAnalysis([f"нет скриншота {path.name} в {artifacts_dir}: стадия viewfinder_live не отработала?"], [])
            shots.append(parse_screencap(path.read_bytes()))
        return analyze_viewfinder(shots[0], shots[1], limits)
    except (ScreenError, OSError) as error:
        return ScreenAnalysis([f"анализ скриншотов не удался: {error}"], [])
