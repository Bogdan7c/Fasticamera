#!/usr/bin/env python3
"""Guardrail границ между crate-ами Fasticamera.

Правило README «нейтральные ядра не зависят от Android/Vulkan» и таблица
«Владельцы состояния» проверяются здесь, а не памятью агента.

Откуда берётся граф: `cargo metadata --format-version 1` с полным resolve
(без `--no-deps`) и без `--filter-platform`. Поэтому зависимости под
`[target.'cfg(target_os = "android")'.dependencies]` тоже попадают в граф.

Какие рёбра учитываются: normal (`kind == null`) и build. Dev-зависимости
НЕ учитываются осознанно: тестовые зависимости нейтрального crate-а — его
собственное дело, в сборку приложения они не попадают.

Проверка транзитивная: запрещённое имя ищется во всём замыкании зависимостей
crate-а, включая внешние крейты (например, внешний крейт, который тянет `jni`).
Имена сравниваются по имени пакета (`name`).

Вторая проверка — правила по исходникам (`[[source_rules]]`, решение 32 сессии 03):
в `.rs`-файлах crate-а запрещённый шаблон (например, `extern "C"`) разрешён только
в перечисленных файлах (пути относительно каталога crate-а). Совпадения внутри
строчных комментариев (`// ...`) не считаются. Блочные комментарии `/* */` не
распознаются: упоминание шаблона в них даст ложное срабатывание (это безопасная сторона).

Коды выхода: 0 — чисто, 1 — нарушения, 2 — ошибка конфигурации или запуска.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tomllib
from collections import deque
from dataclasses import dataclass, field
from pathlib import Path

EXIT_OK = 0
EXIT_VIOLATIONS = 1
EXIT_USAGE_ERROR = 2

SCRIPT_DIR = Path(__file__).resolve().parent
DEFAULT_RULES_PATH = SCRIPT_DIR / "crate-boundaries.toml"
DEFAULT_MANIFEST_PATH = SCRIPT_DIR.parent / "Cargo.toml"
SUPPORTED_SCHEMA_VERSION = 1

# Виды зависимостей, которые попадают в граф. None — обычная (normal) зависимость.
COUNTED_DEPENDENCY_KINDS = (None, "build")


class ToolError(Exception):
    """Ошибка конфигурации или запуска: превращается в код выхода 2."""


@dataclass(frozen=True)
class Rule:
    """Одно правило границ. Подробности полей — в crate-boundaries.toml."""

    rule_id: str
    applies_to_roles: tuple[str, ...]
    applies_to_crates: tuple[str, ...]
    forbidden_names: frozenset[str]
    reason: str


@dataclass(frozen=True)
class SourceRule:
    """Правило по исходникам: шаблон разрешён только в `allowed_files` crate-а."""

    rule_id: str
    crate_name: str
    pattern: str  # обычная подстрока, не регулярное выражение
    allowed_files: frozenset[str]  # пути с «/», относительно каталога crate-а
    reason: str


@dataclass(frozen=True)
class BoundaryConfig:
    """Разобранный и проверенный файл правил."""

    roles: frozenset[str]
    crate_roles: dict[str, str]
    rules: tuple[Rule, ...]
    source_rules: tuple[SourceRule, ...] = ()


@dataclass
class DependencyGraph:
    """Граф зависимостей: имена пакетов, рёбра (normal+build) и члены workspace."""

    names: dict[str, str] = field(default_factory=dict)  # id пакета -> имя
    edges: dict[str, list[str]] = field(default_factory=dict)  # id -> id зависимостей
    workspace_member_ids: list[str] = field(default_factory=list)
    crate_dirs: dict[str, Path] = field(default_factory=dict)  # имя пакета workspace -> каталог

    def workspace_names(self) -> list[str]:
        return [self.names[member_id] for member_id in self.workspace_member_ids]


@dataclass(frozen=True)
class Violation:
    """Нарушение: цепочка имён от crate-а workspace до запрещённого пакета."""

    chain: tuple[str, ...]
    rule: Rule


# --------------------------------------------------------------------------
# Загрузка и проверка файла правил
# --------------------------------------------------------------------------

def _require_string_list(value: object, where: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        raise ToolError(f"{where}: ожидается список строк")
    return tuple(value)


def _parse_rule(raw_rule: object, index: int, roles: frozenset[str],
                crate_roles: dict[str, str]) -> Rule:
    where = f"[[rules]] #{index + 1}"
    if not isinstance(raw_rule, dict):
        raise ToolError(f"{where}: ожидается таблица")
    rule_id = raw_rule.get("id")
    reason = raw_rule.get("reason")
    if not isinstance(rule_id, str) or not rule_id:
        raise ToolError(f"{where}: нет строкового поля id")
    where = f"правило {rule_id!r}"
    if not isinstance(reason, str) or not reason:
        raise ToolError(f"{where}: нет строкового поля reason")

    applies_to_roles = _require_string_list(raw_rule.get("applies_to", []), f"{where}, applies_to")
    applies_to_crates = _require_string_list(
        raw_rule.get("applies_to_crates", []), f"{where}, applies_to_crates")
    if not applies_to_roles and not applies_to_crates:
        raise ToolError(f"{where}: нужно хотя бы одно из applies_to / applies_to_crates")
    for role in applies_to_roles:
        if role not in roles:
            raise ToolError(f"{where}: неизвестная роль {role!r} в applies_to")
    for crate_name in applies_to_crates:
        if crate_name not in crate_roles:
            raise ToolError(f"{where}: crate {crate_name!r} из applies_to_crates не описан в [crates]")

    forbidden = set(_require_string_list(raw_rule.get("forbid", []), f"{where}, forbid"))
    forbid_other_crates = raw_rule.get("forbid_other_crates", False)
    if not isinstance(forbid_other_crates, bool):
        raise ToolError(f"{where}: forbid_other_crates должен быть true/false")
    if forbid_other_crates:
        # «Все остальные» считаем относительно каждого crate-а при проверке:
        # здесь добавляем все, а собственное имя отфильтруем в find_violations.
        forbidden |= set(crate_roles)
    if not forbidden:
        raise ToolError(f"{where}: правило ничего не запрещает")
    return Rule(rule_id, applies_to_roles, applies_to_crates, frozenset(forbidden), reason)


def parse_config(text: str) -> BoundaryConfig:
    """Разбирает TOML правил; любая ошибка схемы — ToolError."""
    try:
        document = tomllib.loads(text)
    except tomllib.TOMLDecodeError as error:
        raise ToolError(f"битый TOML правил: {error}") from error

    if document.get("schema_version") != SUPPORTED_SCHEMA_VERSION:
        raise ToolError(f"ожидается schema_version = {SUPPORTED_SCHEMA_VERSION}")
    roles = frozenset(_require_string_list(document.get("roles"), "roles"))
    raw_crates = document.get("crates")
    if not isinstance(raw_crates, dict):
        raise ToolError("нет секции [crates]")
    crate_roles: dict[str, str] = {}
    for crate_name, role in raw_crates.items():
        if role not in roles:
            raise ToolError(f"[crates]: неизвестная роль {role!r} у crate-а {crate_name!r}")
        crate_roles[crate_name] = role
    raw_rules = document.get("rules", [])
    if not isinstance(raw_rules, list):
        raise ToolError("rules должен быть массивом таблиц [[rules]]")
    rules = tuple(_parse_rule(raw, index, roles, crate_roles) for index, raw in enumerate(raw_rules))
    raw_source_rules = document.get("source_rules", [])
    if not isinstance(raw_source_rules, list):
        raise ToolError("source_rules должен быть массивом таблиц [[source_rules]]")
    source_rules = tuple(_parse_source_rule(raw, index, crate_roles)
                         for index, raw in enumerate(raw_source_rules))
    return BoundaryConfig(roles, crate_roles, rules, source_rules)


def load_config(path: Path) -> BoundaryConfig:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        raise ToolError(f"не удалось прочитать файл правил {path}: {error}") from error
    return parse_config(text)


SOURCE_RULE_KEYS = frozenset({"id", "crate", "pattern", "allowed_files", "reason"})


def _parse_source_rule(raw_rule: object, index: int, crate_roles: dict[str, str]) -> SourceRule:
    where = f"[[source_rules]] #{index + 1}"
    if not isinstance(raw_rule, dict):
        raise ToolError(f"{where}: ожидается таблица")
    unknown_keys = sorted(set(raw_rule) - SOURCE_RULE_KEYS)
    if unknown_keys:
        raise ToolError(f"{where}: неизвестные ключи {unknown_keys} "
                        f"(допустимы: {sorted(SOURCE_RULE_KEYS)})")
    for key in ("id", "crate", "pattern", "reason"):
        value = raw_rule.get(key)
        if not isinstance(value, str) or not value:
            raise ToolError(f"{where}: нет непустого строкового поля {key}")
    where = f"правило по исходникам {raw_rule['id']!r}"
    if raw_rule["crate"] not in crate_roles:
        raise ToolError(f"{where}: crate {raw_rule['crate']!r} не описан в [crates]")
    allowed_files = _require_string_list(raw_rule.get("allowed_files", []), f"{where}, allowed_files")
    for allowed in allowed_files:
        if allowed.startswith("/") or ".." in Path(allowed).parts or "\\" in allowed:
            raise ToolError(f"{where}: путь {allowed!r} в allowed_files должен быть "
                            f"относительным (с «/») и без «..»")
    return SourceRule(raw_rule["id"], raw_rule["crate"], raw_rule["pattern"],
                      frozenset(allowed_files), raw_rule["reason"])


# --------------------------------------------------------------------------
# Граф зависимостей
# --------------------------------------------------------------------------

def build_graph(metadata: dict) -> DependencyGraph:
    """Строит граф из JSON `cargo metadata` (чистая функция)."""
    try:
        graph = DependencyGraph()
        graph.names = {package["id"]: package["name"] for package in metadata["packages"]}
        graph.workspace_member_ids = list(metadata["workspace_members"])
        graph.crate_dirs = {
            package["name"]: Path(package["manifest_path"]).parent
            for package in metadata["packages"] if package["id"] in set(graph.workspace_member_ids)
        }
        for node in metadata["resolve"]["nodes"]:
            graph.edges[node["id"]] = [
                dep["pkg"] for dep in node["deps"]
                if any(kind["kind"] in COUNTED_DEPENDENCY_KINDS for kind in dep["dep_kinds"])
            ]
    except (KeyError, TypeError) as error:
        raise ToolError(f"неожиданная структура cargo metadata (нет поля {error})") from error
    return graph


def run_cargo_metadata(manifest_path: Path, offline: bool) -> dict:
    """Запускает `cargo metadata` и возвращает разобранный JSON."""
    command = ["cargo", "metadata", "--format-version", "1", "--manifest-path", str(manifest_path)]
    if offline:
        command.append("--offline")
    try:
        completed = subprocess.run(command, capture_output=True, text=True, check=False)
    except OSError as error:
        raise ToolError(f"не удалось запустить cargo: {error}") from error
    if completed.returncode != 0:
        raise ToolError(f"cargo metadata завершился с кодом {completed.returncode}:\n"
                        f"{completed.stderr.strip()}")
    return _parse_metadata_json(completed.stdout, "вывод cargo metadata")


def load_metadata_file(path: Path) -> dict:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        raise ToolError(f"не удалось прочитать {path}: {error}") from error
    return _parse_metadata_json(text, str(path))


def _parse_metadata_json(text: str, source: str) -> dict:
    try:
        return json.loads(text)
    except json.JSONDecodeError as error:
        raise ToolError(f"{source}: не JSON ({error})") from error


# --------------------------------------------------------------------------
# Логика проверки
# --------------------------------------------------------------------------

def shortest_chains(graph: DependencyGraph, start_id: str) -> dict[str, tuple[str, ...]]:
    """BFS: для каждого достижимого имени пакета — кратчайшая цепочка имён от start_id."""
    chains: dict[str, tuple[str, ...]] = {}
    visited = {start_id}
    queue: deque[tuple[str, tuple[str, ...]]] = deque([(start_id, (graph.names[start_id],))])
    while queue:
        package_id, chain = queue.popleft()
        for dependency_id in graph.edges.get(package_id, []):
            if dependency_id in visited:
                continue
            visited.add(dependency_id)
            dependency_chain = chain + (graph.names[dependency_id],)
            chains.setdefault(graph.names[dependency_id], dependency_chain)
            queue.append((dependency_id, dependency_chain))
    return chains


def rule_applies(rule: Rule, crate_name: str, crate_role: str) -> bool:
    return crate_role in rule.applies_to_roles or crate_name in rule.applies_to_crates


def find_violations(graph: DependencyGraph, config: BoundaryConfig) -> list[Violation]:
    """Нарушения правил среди классифицированных crate-ов workspace."""
    violations: list[Violation] = []
    for member_id in graph.workspace_member_ids:
        crate_name = graph.names[member_id]
        crate_role = config.crate_roles.get(crate_name)
        if crate_role is None:
            continue  # неклассифицированные ловит find_unclassified
        chains = shortest_chains(graph, member_id)
        for rule in config.rules:
            if not rule_applies(rule, crate_name, crate_role):
                continue
            for forbidden_name in sorted(rule.forbidden_names - {crate_name}):
                if forbidden_name in chains:
                    violations.append(Violation(chains[forbidden_name], rule))
    return violations


def find_unclassified(graph: DependencyGraph, config: BoundaryConfig) -> list[str]:
    return sorted(name for name in graph.workspace_names() if name not in config.crate_roles)


def find_future_crates(graph: DependencyGraph, config: BoundaryConfig) -> list[str]:
    present = set(graph.workspace_names())
    return sorted(name for name in config.crate_roles if name not in present)


@dataclass(frozen=True)
class SourceViolation:
    """Шаблон найден в файле, где он не разрешён."""

    rule: SourceRule
    relative_path: str
    line_number: int
    line_text: str


def code_part_of_line(line: str) -> str:
    """Часть строки до `//`-комментария. Строковые литералы учитываются: `//` внутри
    "..." комментарием не считается. Экранирование `\\"` учитывается."""
    inside_string = False
    index = 0
    while index < len(line):
        char = line[index]
        if inside_string and char == "\\":
            index += 2
            continue
        if char == '"':
            inside_string = not inside_string
        elif not inside_string and line.startswith("//", index):
            return line[:index]
        index += 1
    return line


def find_source_violations(graph: DependencyGraph, config: BoundaryConfig) -> list[SourceViolation]:
    """Ищет запрещённые шаблоны в `.rs`-файлах crate-ов. Чистое чтение файлов."""
    violations: list[SourceViolation] = []
    for rule in config.source_rules:
        crate_dir = graph.crate_dirs.get(rule.crate_name)
        if crate_dir is None:
            continue  # будущий crate: его ещё нет в workspace
        for source_path in sorted(crate_dir.rglob("*.rs")):
            relative_path = source_path.relative_to(crate_dir).as_posix()
            if relative_path.startswith("target/") or relative_path in rule.allowed_files:
                continue
            try:
                text = source_path.read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError) as error:
                raise ToolError(f"не удалось прочитать {source_path}: {error}") from error
            for line_number, line in enumerate(text.splitlines(), start=1):
                if rule.pattern in code_part_of_line(line):
                    violations.append(SourceViolation(rule, relative_path, line_number, line.strip()))
    return violations


# --------------------------------------------------------------------------
# Форматирование сообщений
# --------------------------------------------------------------------------

def format_violation(violation: Violation) -> str:
    chain_text = " → ".join(violation.chain)
    return (f"{chain_text}: запрещено правилом {violation.rule.rule_id} "
            f"({violation.rule.reason})")


def format_source_violation(violation: SourceViolation) -> str:
    rule = violation.rule
    allowed = ", ".join(sorted(rule.allowed_files)) or "нигде"
    return (f"{rule.crate_name}/{violation.relative_path}:{violation.line_number}: "
            f"шаблон {rule.pattern!r} запрещён правилом {rule.rule_id} "
            f"(разрешён только в: {allowed}; {rule.reason}); строка: {violation.line_text}")


def format_unclassified(crate_name: str, rules_path: Path, roles: frozenset[str]) -> str:
    return (f"crate {crate_name!r} не классифицирован: добавьте его в секцию [crates] "
            f"файла {rules_path} с одной из ролей ({', '.join(sorted(roles))})")


def format_future_crates(names: list[str]) -> str:
    return f"info: пока нет в workspace (будущие модули): {', '.join(names)}"


def format_summary(checked_count: int, problem_count: int) -> str:
    return f"Проверено crate-ов: {checked_count}, нарушений: {problem_count}"


# --------------------------------------------------------------------------
# Точка входа
# --------------------------------------------------------------------------

def load_graph(args: argparse.Namespace) -> DependencyGraph:
    if args.metadata_json is not None:
        return build_graph(load_metadata_file(args.metadata_json))
    if not args.manifest_path.is_file():
        raise ToolError(f"не найден манифест {args.manifest_path}: "
                        f"корневого Cargo.toml ещё нет? укажите --manifest-path")
    return build_graph(run_cargo_metadata(args.manifest_path, args.offline))


def parse_args(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Проверка границ между crate-ами workspace.")
    parser.add_argument("--manifest-path", type=Path, default=DEFAULT_MANIFEST_PATH,
                        help="Cargo.toml workspace (по умолчанию корень репозитория)")
    parser.add_argument("--rules", type=Path, default=DEFAULT_RULES_PATH,
                        help="TOML с правилами (по умолчанию рядом со скриптом)")
    parser.add_argument("--offline", action="store_true", help="передать --offline в cargo")
    parser.add_argument("--metadata-json", type=Path, default=None,
                        help="готовый JSON cargo metadata вместо запуска cargo")
    return parser.parse_args(argv)


def run(args: argparse.Namespace) -> int:
    config = load_config(args.rules)
    graph = load_graph(args)

    problems: list[str] = [format_unclassified(name, args.rules, config.roles)
                           for name in find_unclassified(graph, config)]
    problems += [format_violation(violation) for violation in find_violations(graph, config)]
    problems += [format_source_violation(violation)
                 for violation in find_source_violations(graph, config)]
    future_crates = find_future_crates(graph, config)

    for problem in problems:
        print(f"ОШИБКА: {problem}")
    if future_crates:
        print(format_future_crates(future_crates))
    print(format_summary(len(graph.workspace_member_ids), len(problems)))
    return EXIT_VIOLATIONS if problems else EXIT_OK


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        return run(args)
    except ToolError as error:
        print(f"ОШИБКА КОНФИГУРАЦИИ: {error}", file=sys.stderr)
        return EXIT_USAGE_ERROR


if __name__ == "__main__":
    sys.exit(main())
