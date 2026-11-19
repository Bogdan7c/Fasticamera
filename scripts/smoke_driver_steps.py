"""Типы шагов драйвера smoke-сценариев: вид шага, точка экрана в долях, длительность жеста и сам ``Step``.

Вынесены из ``smoke_driver.py`` ради размера модуля (правило проекта: файл около 700 строк делим). Логика выполнения
шагов осталась в ``smoke_driver`` (``StageDriver``, ``AdbDeviceActions``); имена реэкспортируются оттуда, поэтому
``smoke_driver.Step`` и ``smoke_driver.StepKind`` работают как прежде.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum


class StepKind(Enum):
    """Виды шагов драйвера."""

    PRESS_HOME = "press_home"
    PAUSE = "pause"
    RETURN_TO_APP = "return_to_app"
    DENY_PERMISSION_DIALOG = "deny_permission_dialog"
    SCREEN_OFF = "screen_off"
    SCREEN_ON = "screen_on"
    TAP_SCREEN = "tap_screen"
    LAUNCH_SYSTEM_CAMERA = "launch_system_camera"
    SCREENCAP = "screencap"
    TAP_POINT = "tap_point"
    LONG_PRESS_POINT = "long_press_point"
    SWIPE = "swipe"
    STAGE_GESTURE = "stage_gesture"
    """Жест описывает сама стадия (поле ``gesture``: tap / long_press / swipe / volume_up с долями экрана).
    Так Rust-сценарий ``screen`` остаётся единственным владельцем точек касаний (план в его конфиге)."""


@dataclass(frozen=True)
class ScreenFraction:
    """Точка экрана в долях: ``x`` по ширине, ``y`` по высоте, (0, 0) — верхний левый угол.

    Диапазон [0, 1] проверяется при создании: неверная точка ловится при построении таблицы шагов.
    """

    x: float
    y: float

    def __post_init__(self) -> None:
        for axis, value in (("x", self.x), ("y", self.y)):
            # Условие записано так, чтобы NaN тоже не прошёл (сравнения с NaN ложны).
            if not 0.0 <= value <= 1.0:
                raise ValueError(f"доля экрана {axis}={value!r} вне диапазона [0, 1]")


@dataclass(frozen=True)
class DurationMs:
    """Длительность жеста в миллисекундах; строго положительная."""

    value: int

    def __post_init__(self) -> None:
        if isinstance(self.value, bool) or not isinstance(self.value, int) or self.value <= 0:
            raise ValueError(f"длительность жеста должна быть целым положительным числом мс, получено {self.value!r}")


@dataclass(frozen=True)
class Step:
    """Один шаг; ``seconds`` нужен только шагу ``PAUSE``, ``label`` — только шагу ``SCREENCAP``
    (имя файла скриншота в каталоге артефактов).

    Жесты: ``TAP_POINT`` требует ``point``; ``LONG_PRESS_POINT`` — ``point`` и ``duration`` (удержание);
    ``SWIPE`` — ``point`` (откуда), ``end_point`` (куда) и ``duration``. Лишние и недостающие поля —
    ``ValueError`` при построении.

    ``only_if_true`` — имя булева поля стадии: шаг выполняется, только если поле есть и равно
    ``true`` (иначе пропускается). Так стадия, общая для разных ситуаций, управляет шагом данными.
    """

    kind: StepKind
    seconds: float = 0.0
    only_if_true: str | None = None
    label: str = ""
    point: ScreenFraction | None = None
    end_point: ScreenFraction | None = None
    duration: DurationMs | None = None

    def __post_init__(self) -> None:
        needs_point = self.kind in _GESTURE_KINDS
        needs_end_point = self.kind is StepKind.SWIPE
        needs_duration = self.kind in (StepKind.LONG_PRESS_POINT, StepKind.SWIPE)
        for field_name, present, required in (
            ("point", self.point is not None, needs_point),
            ("end_point", self.end_point is not None, needs_end_point),
            ("duration", self.duration is not None, needs_duration),
        ):
            if present != required:
                verdict = "обязательно" if required else "не допускается"
                raise ValueError(f"у шага {self.kind.value} поле {field_name} {verdict}")


_GESTURE_KINDS = (StepKind.TAP_POINT, StepKind.LONG_PRESS_POINT, StepKind.SWIPE)
"""Шаги-жесты в произвольную точку экрана (нагрузка в ``Step.point`` и далее)."""
