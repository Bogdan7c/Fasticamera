#!/usr/bin/env python3
"""Ratchet размера модулей.

Файлы длиннее жёсткого предела запрещены. Исключение — legacy-файлы из секции
`[baseline]` политики: им разрешено ровно записанное число строк, и оно может
только уменьшаться («храповик»: рост узаконить нельзя).

Логика:
  * > hard и нет в baseline          -> ошибка;
  * > soft и <= hard                  -> предупреждение;
  * в baseline и вырос                -> ошибка «legacy-файл вырос»;
  * в baseline и стал <= hard         -> предупреждение: уберите запись;
  * запись baseline без файла         -> предупреждение «устаревшая запись».

`--update-baseline` только уменьшает записи и удаляет лишние; новые файлы выше
предела он не добавляет. Для первичного заведения legacy-файла есть явный
`--adopt <путь>`.

Коды выхода: 0 — ок (возможны предупреждения), 1 — нарушения, 2 — ошибка
конфигурации или запуска.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import tomllib
from dataclasses import dataclass
from enum import Enum
from pathlib import Path

EXIT_OK = 0
EXIT_VIOLATIONS = 1
EXIT_USAGE_ERROR = 2

SCRIPT_DIR = Path(__file__).resolve().parent
DEFAULT_POLICY_PATH = SCRIPT_DIR / "module-size-policy.toml"
DEFAULT_ROOT = SCRIPT_DIR.parent
SUPPORTED_SCHEMA_VERSION = 1

# Строка заголовка секции baseline; всё от неё и до конца файла принадлежит скрипту.
BASELINE_HEADER_PATTERN = re.compile(r"^\[baseline\][ \t]*(#.*)?$", re.MULTILINE)


class ToolError(Exception):
    """Ошибка конфигурации или запуска: превращается в код выхода 2."""


class Severity(Enum):
    WARNING = "предупреждение"
    ERROR = "ОШИБКА"


@dataclass(frozen=True)
class Policy:
    soft_limit_lines: int
    hard_limit_lines: int
    roots: tuple[str, ...]
    extensions: tuple[str, ...]
    exclude_dir_names: frozenset[str]
    baseline: dict[str, int]


@dataclass(frozen=True)
class Finding:
    severity: Severity
    path: str
    message: str


# --------------------------------------------------------------------------
# Политика
# --------------------------------------------------------------------------

def _require_int(document: dict, key: str) -> int:
    value = document.get(key)
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise ToolError(f"{key}: ожидается положительное целое")
    return value


def _require_str_list(document: dict, key: str) -> tuple[str, ...]:
    value = document.get(key)
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        raise ToolError(f"{key}: ожидается список строк")
    return tuple(value)


def parse_policy(text: str) -> Policy:
    try:
        document = tomllib.loads(text)
    except tomllib.TOMLDecodeError as error:
        raise ToolError(f"битый TOML политики: {error}") from error
    if document.get("schema_version") != SUPPORTED_SCHEMA_VERSION:
        raise ToolError(f"ожидается schema_version = {SUPPORTED_SCHEMA_VERSION}")
    soft_limit = _require_int(document, "soft_limit_lines")
    hard_limit = _require_int(document, "hard_limit_lines")
    if soft_limit >= hard_limit:
        raise ToolError(f"soft_limit_lines ({soft_limit}) должен быть меньше hard_limit_lines ({hard_limit})")
    baseline = document.get("baseline", {})
    if not isinstance(baseline, dict):
        raise ToolError("baseline: ожидается таблица путь = число строк")
    for path, allowed_lines in baseline.items():
        if not isinstance(allowed_lines, int) or isinstance(allowed_lines, bool) or allowed_lines <= 0:
            raise ToolError(f"baseline[{path!r}]: ожидается положительное целое")
    return Policy(
        soft_limit_lines=soft_limit,
        hard_limit_lines=hard_limit,
        roots=_require_str_list(document, "roots"),
        extensions=_require_str_list(document, "extensions"),
        exclude_dir_names=frozenset(_require_str_list(document, "exclude_dir_names")),
        baseline=dict(baseline),
    )


def read_policy_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except OSError as error:
        raise ToolError(f"не удалось прочитать политику {path}: {error}") from error


# --------------------------------------------------------------------------
# Сканирование файловой системы
# --------------------------------------------------------------------------

def count_lines(path: Path) -> int:
    """Число строк: последняя строка без перевода строки тоже считается."""
    try:
        content = path.read_bytes()
    except OSError as error:
        raise ToolError(f"не удалось прочитать {path}: {error}") from error
    newline_count = content.count(b"\n")
    return newline_count + (1 if content and not content.endswith(b"\n") else 0)


def scan_files(root: Path, policy: Policy) -> dict[str, int]:
    """Возвращает {относительный путь с '/': число строк} для проверяемых файлов."""
    line_counts: dict[str, int] = {}
    for root_name in policy.roots:
        start_dir = root / root_name
        if not start_dir.is_dir():
            continue  # crate-ы появляются постепенно — это нормально
        for current_dir, child_dirs, file_names in os.walk(start_dir):
            # Сортировка и удаление на месте дают детерминированный обход без исключённых каталогов.
            child_dirs[:] = sorted(name for name in child_dirs if name not in policy.exclude_dir_names)
            for file_name in file_names:
                if Path(file_name).suffix not in policy.extensions:
                    continue
                file_path = Path(current_dir) / file_name
                relative = file_path.relative_to(root).as_posix()
                line_counts[relative] = count_lines(file_path)
    return line_counts


# --------------------------------------------------------------------------
# Логика ratchet
# --------------------------------------------------------------------------

def plural_lines(count: int) -> str:
    """Склонение: 1 строка, 2 строки, 5 строк."""
    if count % 100 in range(11, 15):
        return f"{count} строк"
    last_digit = count % 10
    if last_digit == 1:
        return f"{count} строка"
    if last_digit in (2, 3, 4):
        return f"{count} строки"
    return f"{count} строк"


def _evaluate_baseline_file(path: str, lines: int, allowed: int, policy: Policy) -> list[Finding]:
    if lines > allowed:
        return [Finding(Severity.ERROR, path,
                        f"legacy-файл вырос: {plural_lines(lines)} > записанных в baseline {allowed}")]
    if lines <= policy.hard_limit_lines:
        return [Finding(Severity.WARNING, path,
                        f"{plural_lines(lines)} <= жёсткий предел {policy.hard_limit_lines}: "
                        f"уберите запись из baseline (--update-baseline)")]
    return []


def _evaluate_regular_file(path: str, lines: int, policy: Policy) -> list[Finding]:
    if lines > policy.hard_limit_lines:
        return [Finding(Severity.ERROR, path,
                        f"{plural_lines(lines)} > жёсткий предел {policy.hard_limit_lines}")]
    if lines > policy.soft_limit_lines:
        return [Finding(Severity.WARNING, path,
                        f"{plural_lines(lines)} > мягкий предел {policy.soft_limit_lines}")]
    return []


def evaluate(line_counts: dict[str, int], policy: Policy) -> list[Finding]:
    findings: list[Finding] = []
    for path in sorted(line_counts):
        lines = line_counts[path]
        if path in policy.baseline:
            findings += _evaluate_baseline_file(path, lines, policy.baseline[path], policy)
        else:
            findings += _evaluate_regular_file(path, lines, policy)
    for path in sorted(policy.baseline):
        if path not in line_counts:
            findings.append(Finding(Severity.WARNING, path,
                                    "устаревшая запись baseline: файла нет (--update-baseline удалит её)"))
    return findings


def shrink_baseline(line_counts: dict[str, int], policy: Policy) -> dict[str, int]:
    """Новый baseline только «вниз»: min(текущее, записанное); лишние записи удаляются."""
    shrunk: dict[str, int] = {}
    for path, allowed in policy.baseline.items():
        if path not in line_counts:
            continue
        lines = min(line_counts[path], allowed)
        if lines > policy.hard_limit_lines:
            shrunk[path] = lines
    return shrunk


def adopt_files(paths: list[str], line_counts: dict[str, int], policy: Policy) -> dict[str, int]:
    """Явно заводит legacy-файлы в baseline. Только для файлов выше жёсткого предела."""
    adopted = dict(policy.baseline)
    for path in paths:
        if path not in line_counts:
            raise ToolError(f"--adopt: файл {path} не найден среди проверяемых")
        if path in policy.baseline:
            raise ToolError(f"--adopt: {path} уже в baseline; рост узаконить нельзя")
        if line_counts[path] <= policy.hard_limit_lines:
            raise ToolError(f"--adopt: {path} ({plural_lines(line_counts[path])}) не выше жёсткого "
                            f"предела {policy.hard_limit_lines}, заводить в baseline нечего")
        adopted[path] = line_counts[path]
    return adopted


# --------------------------------------------------------------------------
# Переписывание секции [baseline]
# --------------------------------------------------------------------------

def render_baseline_section(baseline: dict[str, int]) -> str:
    entries = "".join(f"{json.dumps(path, ensure_ascii=False)} = {allowed}\n"
                      for path, allowed in sorted(baseline.items()))
    return "[baseline]\n" + entries


def replace_baseline_section(policy_text: str, baseline: dict[str, int]) -> str:
    """Сохраняет всё до `[baseline]`, а секцию от неё и до конца файла пишет заново."""
    match = BASELINE_HEADER_PATTERN.search(policy_text)
    head = policy_text[:match.start()] if match else policy_text.rstrip("\n") + "\n\n"
    new_text = head + render_baseline_section(baseline)
    if tomllib.loads(new_text).get("baseline", {}) != baseline:
        # Страховка: если после [baseline] была чужая секция, она бы исказила результат.
        raise ToolError("не удалось безопасно переписать [baseline]: "
                        "секция должна быть последней в файле политики")
    return new_text


def write_policy(path: Path, text: str) -> None:
    try:
        path.write_text(text, encoding="utf-8")
    except OSError as error:
        raise ToolError(f"не удалось записать политику {path}: {error}") from error


# --------------------------------------------------------------------------
# Вывод и точка входа
# --------------------------------------------------------------------------

def format_finding(finding: Finding) -> str:
    return f"{finding.severity.value}: {finding.path}: {finding.message}"


def format_summary(files_count: int, warnings: int, errors: int) -> str:
    return f"Просмотрено файлов: {files_count}, предупреждений: {warnings}, ошибок: {errors}"


def parse_args(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Ratchet размера модулей.")
    parser.add_argument("--root", type=Path, default=DEFAULT_ROOT, help="корень репозитория")
    parser.add_argument("--policy", type=Path, default=DEFAULT_POLICY_PATH, help="TOML политики")
    parser.add_argument("--update-baseline", action="store_true",
                        help="только уменьшить/удалить записи baseline; рост не узаконивает")
    parser.add_argument("--adopt", action="append", default=[], metavar="PATH",
                        help="завести legacy-файл (путь от root) в baseline с текущим размером")
    return parser.parse_args(argv)


def apply_baseline_changes(args: argparse.Namespace, policy_text: str, policy: Policy,
                           line_counts: dict[str, int]) -> Policy:
    """Выполняет --adopt и --update-baseline; возвращает политику с новым baseline."""
    new_baseline = policy.baseline
    if args.adopt:
        new_baseline = adopt_files(args.adopt, line_counts, policy)
        for path in args.adopt:
            print(f"--adopt: {path} заведён в baseline с {plural_lines(new_baseline[path])} "
                  f"(легализация legacy; рост после этого запрещён)")
    if args.update_baseline:
        new_baseline = shrink_baseline(line_counts, Policy(
            policy.soft_limit_lines, policy.hard_limit_lines, policy.roots, policy.extensions,
            policy.exclude_dir_names, new_baseline))
    if new_baseline == policy.baseline:
        return policy
    write_policy(args.policy, replace_baseline_section(policy_text, new_baseline))
    print(f"baseline обновлён: записей {len(new_baseline)}")
    return Policy(policy.soft_limit_lines, policy.hard_limit_lines, policy.roots,
                  policy.extensions, policy.exclude_dir_names, new_baseline)


def run(args: argparse.Namespace) -> int:
    policy_text = read_policy_text(args.policy)
    policy = parse_policy(policy_text)
    line_counts = scan_files(args.root, policy)
    if args.update_baseline or args.adopt:
        policy = apply_baseline_changes(args, policy_text, policy, line_counts)

    findings = evaluate(line_counts, policy)
    for finding in findings:
        print(format_finding(finding))
    warnings = sum(1 for finding in findings if finding.severity is Severity.WARNING)
    errors = sum(1 for finding in findings if finding.severity is Severity.ERROR)
    print(format_summary(len(line_counts), warnings, errors))
    return EXIT_VIOLATIONS if errors else EXIT_OK


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        return run(args)
    except ToolError as error:
        print(f"ОШИБКА КОНФИГУРАЦИИ: {error}", file=sys.stderr)
        return EXIT_USAGE_ERROR


if __name__ == "__main__":
    sys.exit(main())
