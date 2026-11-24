"""Параметры журнала тепла телефона (``device_thermal_log.py``): всё, что можно подкрутить, лежит здесь одним местом."""

from __future__ import annotations

FORMAT_VERSION = 1
"""Версия формата JSON Lines; растёт при несовместимом изменении полей строки."""

DEFAULT_PERIOD_SECONDS = 5.0
"""Период замера по умолчанию (секунды): достаточно часто для графика, достаточно редко, чтобы не греть телефон."""

MIN_PERIOD_SECONDS = 1.0
"""Нижняя граница периода: один замер = один ``adb shell`` (+ ``dumpsys``), чаще нет смысла и это нагрузка."""

DEFAULT_DEVICE_MODEL = "CPH2447"
"""Модель телефона (OnePlus 11); переопределяется ``FASTICAMERA_DEVICE_MODEL``, как в ``lib/common.sh``."""

ADB_TIMEOUT_SECONDS = 20.0
"""Потолок одного ``adb shell``: при обрыве беспроводного adb команда может зависнуть."""

THERMAL_ZONE_GLOB = "/sys/class/thermal/thermal_zone*"
"""Каталоги тепловых зон; тип зоны лежит в ``<зона>/type``, температура — в ``<зона>/temp``."""

GPU_BUSY_PATH = "/sys/class/kgsl/kgsl-3d0/gpubusy"
"""Единственный читаемый без root файл kgsl: два числа «занято» / «всего» за окно драйвера."""

GPUSS_PREFIX = "gpuss-"
"""Префикс зон кристалла GPU (``gpuss-0`` … ``gpuss-7``); по ним считаются «максимум/среднее по всем gpuss»."""

TRACKED_ZONES: tuple[str, ...] = (
    "gpuss-0",
    "gpuss-1",
    "gpuss-2",
    "gpuss-3",
    "gpuss-4",
    "gpuss-5",
    "gpuss-6",
    "gpuss-7",
    "shell_front",
    "shell_frame",
    "shell_back",
    "skin-msm-therm",
    "battery",
)
"""Тепловые зоны, которые пишем (по имени из ``type``)."""

MILLI_CELSIUS_PER_CELSIUS = 1000.0
"""Ядро Linux отдаёт температуру зон в милли-градусах."""

BATTERY_DECI_CELSIUS_PER_CELSIUS = 10.0
"""``dumpsys battery`` отдаёт температуру в десятых долях градуса (353 = 35,3 °C)."""

SUMMARY_DECIMALS = 2
"""Знаков после запятой в таблице сводки."""
