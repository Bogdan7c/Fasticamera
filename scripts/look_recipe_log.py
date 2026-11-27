#!/usr/bin/env python3
"""Разбор строк лога рецепта вида (сессия 14o): что release пишет в logcat и что из этого можно сверить.

Читает текст logcat (файл или stdin) и достаёт три вида строк (формат и точки записи — `look_recipe_log.rs`
приложения и `still/look_log.rs` рендера; менять только вместе с ними)::

    look_recipe origin=<origin> fp=<16 hex> base=<agx|aces|filmic|legacy> settings=<компактный json>
    look_still_press fp=<16 hex> view_fp=<16 hex|unknown> clean_view=<show_edited|show_base> scenario_claim=<held|free>
    look_still_render fp=<16 hex>

Правило «снимок = видоискатель по виду» (`check_still_matches_viewfinder`): у каждого нажатия затвора

* отпечаток рецепта снимка совпадает с отпечатком вида на рендере в момент нажатия — если не зажат чистый вид
  (рендер показывает нейтраль нарочно) и сценарий замера не держал вид рендера (он показывал своё);
* он же совпадает с последним записанным видом продукта (старт, смена пресета, итог жеста, снятие заявки сценария);
* движок снимка проявил кадр тем же рецептом (`look_still_render` после нажатия).

Запуск: ``scripts/look_recipe_log.py logcat.txt`` (``-`` — stdin). Код выхода: 0 — всё сошлось, 1 — есть нарушения
или в логе нет ни одной строки рецепта, 2 — ошибка вызова.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

EXIT_OK, EXIT_PROBLEMS, EXIT_USAGE = 0, 1, 2

PRODUCT_ORIGINS = frozenset(
    {"startup", "preset_changed", "edit_settled", "edit_before_still", "clean_view_show_edited", "scenario_release"}
)
"""Точки фиксации, после которых рендер показывает вид продукта (рецепт строки = рецепт библиотеки)."""

NON_PRODUCT_ORIGINS = frozenset({"clean_view_show_base", "scenario_claim"})
"""Точки, где в строке не вид продукта: нейтраль чистого вида и вид, который показывает сценарий замера."""

TONE_BASES = frozenset({"agx", "aces", "filmic", "legacy"})
FINGERPRINT = r"[0-9a-f]{16}"

RECIPE_PATTERN = re.compile(
    rf"look_recipe origin=(?P<origin>[a-z_]+) fp=(?P<fp>{FINGERPRINT}) base=(?P<base>[a-z]+) settings=(?P<settings>\{{.*\}})\s*$"
)
STILL_PRESS_PATTERN = re.compile(
    rf"look_still_press fp=(?P<fp>{FINGERPRINT}) view_fp=(?P<view_fp>{FINGERPRINT}|unknown) "
    r"clean_view=(?P<clean_view>show_edited|show_base) scenario_claim=(?P<claim>held|free)"
)
STILL_RENDER_PATTERN = re.compile(rf"look_still_render fp=(?P<fp>{FINGERPRINT})")


@dataclass(frozen=True)
class RecipeEvent:
    """Строка `look_recipe`."""

    line_number: int
    origin: str
    fingerprint: str
    base: str
    settings: dict


@dataclass(frozen=True)
class StillPressEvent:
    """Строка `look_still_press`: нажатие затвора."""

    line_number: int
    fingerprint: str
    view_fingerprint: str
    clean_view: str
    scenario_claim: str


@dataclass(frozen=True)
class StillRenderEvent:
    """Строка `look_still_render`: движок снимка взял рецепт в работу."""

    line_number: int
    fingerprint: str


Event = RecipeEvent | StillPressEvent | StillRenderEvent


class LogParseError(ValueError):
    """Строка похожа на строку рецепта, но не читается: молча пропускать её нельзя (иначе проверка «прошла» бы без неё)."""


def parse_line(line_number: int, line: str) -> Event | None:
    """Событие из строки logcat; `None` — это строка не про вид. Битая строка про вид — `LogParseError`."""
    if "look_recipe " in line:
        match = RECIPE_PATTERN.search(line)
        if match is None:
            raise LogParseError(f"строка {line_number}: не читается look_recipe: {line.strip()!r}")
        if match["base"] not in TONE_BASES:
            raise LogParseError(f"строка {line_number}: неизвестная основа {match['base']!r}")
        try:
            settings = json.loads(match["settings"])
        except json.JSONDecodeError as error:
            raise LogParseError(f"строка {line_number}: settings не JSON ({error})") from error
        return RecipeEvent(line_number, match["origin"], match["fp"], match["base"], settings)
    if "look_still_press " in line:
        match = STILL_PRESS_PATTERN.search(line)
        if match is None:
            raise LogParseError(f"строка {line_number}: не читается look_still_press: {line.strip()!r}")
        return StillPressEvent(line_number, match["fp"], match["view_fp"], match["clean_view"], match["claim"])
    if "look_still_render " in line:
        match = STILL_RENDER_PATTERN.search(line)
        if match is None:
            raise LogParseError(f"строка {line_number}: не читается look_still_render: {line.strip()!r}")
        return StillRenderEvent(line_number, match["fp"])
    return None


def parse_lines(lines: list[str]) -> list[Event]:
    """Все события лога по порядку."""
    events = []
    for number, line in enumerate(lines, start=1):
        event = parse_line(number, line)
        if event is not None:
            events.append(event)
    return events


def last_product_recipe(events: list[Event]) -> RecipeEvent | None:
    """Последний записанный вид продукта."""
    for event in reversed(events):
        if isinstance(event, RecipeEvent) and event.origin in PRODUCT_ORIGINS:
            return event
    return None


def check_still_matches_viewfinder(events: list[Event], require_render_line: bool = True) -> list[str]:
    """Нарушения правила «снимок = видоискатель по виду» (см. доку модуля). Пустой список — всё сошлось."""
    problems: list[str] = []
    product: RecipeEvent | None = None
    pending: StillPressEvent | None = None
    presses = 0
    for event in events:
        if isinstance(event, RecipeEvent):
            if event.origin not in PRODUCT_ORIGINS | NON_PRODUCT_ORIGINS:
                problems.append(f"строка {event.line_number}: неизвестная точка фиксации {event.origin!r}")
            elif event.origin in PRODUCT_ORIGINS:
                product = event
        elif isinstance(event, StillPressEvent):
            presses += 1
            if pending is not None and require_render_line:
                problems.append(f"нажатие на строке {pending.line_number}: движок снимка не записал вид проявки")
            pending = event
            problems.extend(_press_problems(event, product))
        elif isinstance(event, StillRenderEvent):
            if pending is None:
                problems.append(f"строка {event.line_number}: вид проявки без нажатия затвора")
            elif event.fingerprint != pending.fingerprint:
                problems.append(
                    f"нажатие на строке {pending.line_number}: снимок ушёл с видом {pending.fingerprint}, "
                    f"а проявлен видом {event.fingerprint} (строка {event.line_number})"
                )
            pending = None
    if pending is not None and require_render_line:
        problems.append(f"нажатие на строке {pending.line_number}: движок снимка не записал вид проявки")
    if presses == 0:
        problems.append("в логе нет ни одного нажатия затвора (look_still_press): сверять нечего")
    return problems


def _press_problems(press: StillPressEvent, product: RecipeEvent | None) -> list[str]:
    where = f"нажатие на строке {press.line_number}"
    problems: list[str] = []
    if press.scenario_claim == "held":
        return problems  # рендер показывает вид сценария замера: сверять с видоискателем нечего
    if press.clean_view == "show_edited":
        if press.view_fingerprint == "unknown":
            problems.append(f"{where}: что показывал рендер, неизвестно (команда вида не подтверждена)")
        elif press.view_fingerprint != press.fingerprint:
            problems.append(
                f"{where}: снимок уходит с видом {press.fingerprint}, а видоискатель показывал {press.view_fingerprint}"
            )
    if product is None:
        problems.append(f"{where}: до него не записано ни одного вида продукта (look_recipe)")
    elif product.fingerprint != press.fingerprint:
        problems.append(
            f"{where}: снимок уходит с видом {press.fingerprint}, а последний записанный вид продукта "
            f"{product.fingerprint} (строка {product.line_number})"
        )
    return problems


def check_log(lines: list[str], require_render_line: bool = True) -> list[str]:
    """Полная проверка текста лога: строки читаются, рецепты есть, снимки совпадают с видоискателем."""
    try:
        events = parse_lines(lines)
    except LogParseError as error:
        return [str(error)]
    if not any(isinstance(event, RecipeEvent) for event in events):
        return ["в логе нет ни одной строки look_recipe: release не писал вид (фильтр logcat или не тот прогон)"]
    return check_still_matches_viewfinder(events, require_render_line)


def summarize(events: list[Event]) -> list[str]:
    """Строки для человека: какие виды и когда были зафиксированы."""
    lines = []
    for event in events:
        if isinstance(event, RecipeEvent):
            lines.append(f"строка {event.line_number}: {event.origin} fp={event.fingerprint} основа {event.base}")
        elif isinstance(event, StillPressEvent):
            lines.append(f"строка {event.line_number}: снимок fp={event.fingerprint} видоискатель {event.view_fingerprint}")
    return lines


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("log", help="файл с текстом logcat; «-» — stdin")
    parser.add_argument(
        "--allow-unrendered",
        action="store_true",
        help="не требовать look_still_render после каждого нажатия (лог оборван до проявки)",
    )
    arguments = parser.parse_args(argv)
    try:
        text = sys.stdin.read() if arguments.log == "-" else Path(arguments.log).read_text(encoding="utf-8")
    except OSError as error:
        print(f"не прочитан {arguments.log}: {error}", file=sys.stderr)
        return EXIT_USAGE
    lines = text.splitlines()
    problems = check_log(lines, require_render_line=not arguments.allow_unrendered)
    try:
        for line in summarize(parse_lines(lines)):
            print(line)
    except LogParseError:
        pass  # причина уже в problems
    for problem in problems:
        print(f"НАРУШЕНИЕ: {problem}", file=sys.stderr)
    return EXIT_PROBLEMS if problems else EXIT_OK


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
