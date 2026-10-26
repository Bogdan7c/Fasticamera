"""Разбор дампа иерархии окна (``uiautomator dump``) для smoke-сценариев Fasticamera.

Нужен одному: найти в системном диалоге разрешения кнопку отказа и вернуть точку нажатия.
Модуль чистый (без adb и файлов) и тестируется на синтетических XML.

Приватность: дамп содержит текст всего экрана, поэтому модуль ничего не сохраняет и не печатает;
в сообщения об ошибках XML не попадает.
"""

from __future__ import annotations

import re
import xml.etree.ElementTree as ElementTree
from dataclasses import dataclass

DENY_BUTTON_ID_SUFFIXES = (
    ":id/permission_deny_button",
    ":id/permission_deny_and_dont_ask_again_button",
)
"""Окончания resource-id кнопок отказа. Пакет (``com.android.…`` или ``com.google.android.…``
permissioncontroller) зависит от прошивки, поэтому сравниваем только по окончанию."""

HIERARCHY_END_TAG = "</hierarchy>"
"""Конец документа: после него ``uiautomator dump /dev/tty`` печатает «UI hierchary dumped to: …»."""

_BOUNDS_PATTERN = re.compile(r"^\[(\d+),(\d+)\]\[(\d+),(\d+)\]$")


class UiDumpParseError(Exception):
    """Дамп нельзя разобрать (оборван, не XML, кривые bounds). Для опроса это «пока не готов»."""


@dataclass(frozen=True)
class TapPoint:
    """Точка нажатия в пикселях экрана."""

    x: int
    y: int


def strip_dump_tail(dump_text: str) -> str:
    """Отрезает всё после ``</hierarchy>`` (хвост «dumped to: /dev/tty» не является XML)."""
    end = dump_text.rfind(HIERARCHY_END_TAG)
    if end < 0:
        return dump_text
    return dump_text[: end + len(HIERARCHY_END_TAG)]


def parse_bounds_center(bounds: str) -> TapPoint:
    """Центр прямоугольника из атрибута ``bounds="[x1,y1][x2,y2]"``."""
    match = _BOUNDS_PATTERN.match(bounds.strip())
    if match is None:
        raise UiDumpParseError(f"не разобрать bounds {bounds!r}")
    left, top, right, bottom = (int(group) for group in match.groups())
    if right < left or bottom < top:
        raise UiDumpParseError(f"перевёрнутый прямоугольник bounds {bounds!r}")
    return TapPoint((left + right) // 2, (top + bottom) // 2)


def find_deny_button(dump_text: str) -> TapPoint | None:
    """Центр кнопки отказа в диалоге разрешения или ``None``, если кнопки на экране нет.

    Битый XML — ``UiDumpParseError`` (а не ``None``): «нет кнопки» и «дамп испорчен» —
    разные причины, и в сообщении о таймауте они должны различаться.
    """
    try:
        root = ElementTree.fromstring(strip_dump_tail(dump_text).strip())
    except ElementTree.ParseError as error:
        raise UiDumpParseError(f"дамп не XML: {error}") from error
    for node in root.iter("node"):
        resource_id = node.get("resource-id", "")
        if resource_id.endswith(DENY_BUTTON_ID_SUFFIXES):
            return parse_bounds_center(node.get("bounds", ""))
    return None
