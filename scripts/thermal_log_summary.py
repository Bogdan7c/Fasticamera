"""Сводка журнала тепла по интервалам времени: чтение JSONL → статистика → таблица Markdown / JSON.

Окно полуоткрытое: ``from_epoch <= t < to_epoch`` (время телефона). Пропуски (``kind == "gap"``) попадают в окно
по оценке времени ``t_phone_estimate``; у пропуска без оценки (обрыв до первого замера) времени нет, он
учитывается только в окне «весь файл».
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

from thermal_log_config import GPUSS_PREFIX, SUMMARY_DECIMALS

GPUSS_MAX_METRIC = "gpuss_max"
"""Метрика: максимум по всем ``gpuss-*`` в каждый момент замера."""

GPUSS_MEAN_METRIC = "gpuss_mean"
"""Метрика: среднее по всем ``gpuss-*`` в каждый момент замера."""

WHOLE_FILE_WINDOW_NAME = "весь файл"
INTERVAL_WINDOW_NAME = "интервал"


class SummaryError(ValueError):
    """Файл журнала или файл окон нельзя прочитать."""


@dataclass(frozen=True)
class TimeWindow:
    name: str
    from_epoch: float | None
    to_epoch: float | None

    def contains(self, epoch: float) -> bool:
        return (self.from_epoch is None or epoch >= self.from_epoch) and (self.to_epoch is None or epoch < self.to_epoch)


@dataclass(frozen=True)
class MetricStats:
    count: int
    minimum: float
    mean: float
    maximum: float
    first: float
    last: float


@dataclass(frozen=True)
class WindowSummary:
    window: TimeWindow
    sample_count: int
    gap_count: int
    read_failures: int
    metrics: dict[str, MetricStats]


def read_log(path: Path) -> tuple[dict, list[dict]]:
    """Заголовок и записи (замеры и пропуски) в порядке файла. Битые строки — ошибка, а не молчаливый пропуск."""
    try:
        lines = [line for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]
    except OSError as error:
        raise SummaryError(f"не удалось прочитать журнал {path}: {error}") from error
    try:
        records = [json.loads(line) for line in lines]
    except json.JSONDecodeError as error:
        raise SummaryError(f"журнал {path} содержит строку не-JSON: {error}") from error
    if not records or records[0].get("kind") != "header":
        raise SummaryError(f"в журнале {path} нет строки-заголовка")
    return records[0], records[1:]


def load_windows(path: Path) -> list[TimeWindow]:
    """Окна из JSON ``[{"name": ..., "from_epoch": ..., "to_epoch": ...}]``."""
    try:
        items = json.loads(path.read_text(encoding="utf-8"))
        return [TimeWindow(str(item["name"]), float(item["from_epoch"]), float(item["to_epoch"])) for item in items]
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise SummaryError(f"не удалось разобрать файл окон {path}: {error!r}") from error


def sample_metrics(sample: dict) -> dict[str, float]:
    """Числовые величины одного замера (``None`` отбрасываются) + агрегаты по ``gpuss-*``."""
    metrics: dict[str, float] = {}
    gpuss_values: list[float] = []
    for zone_name, value in sample.get("temps_c", {}).items():
        if value is None:
            continue
        metrics[f"temp:{zone_name}"] = float(value)
        if zone_name.startswith(GPUSS_PREFIX):
            gpuss_values.append(float(value))
    if gpuss_values:
        metrics[f"temp:{GPUSS_MAX_METRIC}"] = max(gpuss_values)
        metrics[f"temp:{GPUSS_MEAN_METRIC}"] = sum(gpuss_values) / len(gpuss_values)
    for key in ("gpu_busy_fraction", "thermal_status", "battery_temp_c", "battery_level_percent"):
        if sample.get(key) is not None:
            metrics[key] = float(sample[key])
    if sample.get("external_power") is not None:
        metrics["external_power"] = 1.0 if sample["external_power"] else 0.0
    return metrics


def _stats(series: list[float]) -> MetricStats:
    return MetricStats(len(series), min(series), sum(series) / len(series), max(series), series[0], series[-1])


def summarize_window(records: list[dict], window: TimeWindow) -> WindowSummary:
    """Статистика по замерам и пропускам, попавшим в окно."""
    series: dict[str, list[float]] = {}
    sample_count = gap_count = read_failures = 0
    for record in records:
        if record.get("kind") == "sample" and window.contains(record["t_phone"]):
            sample_count += 1
            read_failures += record.get("read_failures", 0)
            for metric_name, value in sample_metrics(record).items():
                series.setdefault(metric_name, []).append(value)
        elif record.get("kind") == "gap":
            estimate = record.get("t_phone_estimate")
            in_window = window.contains(estimate) if estimate is not None else (
                window.from_epoch is None and window.to_epoch is None
            )
            gap_count += 1 if in_window else 0
    return WindowSummary(window, sample_count, gap_count, read_failures, {n: _stats(s) for n, s in series.items()})


def summarize(records: list[dict], windows: list[TimeWindow]) -> list[WindowSummary]:
    return [summarize_window(records, window) for window in windows]


def windows_from_options(
    from_epoch: float | None, to_epoch: float | None, windows_file: Path | None
) -> list[TimeWindow]:
    """Окна из CLI: файл окон важнее; иначе один интервал; иначе весь файл."""
    if windows_file is not None:
        return load_windows(windows_file)
    if from_epoch is None and to_epoch is None:
        return [TimeWindow(WHOLE_FILE_WINDOW_NAME, None, None)]
    return [TimeWindow(INTERVAL_WINDOW_NAME, from_epoch, to_epoch)]


def _metric_label(metric_name: str) -> str:
    return metric_name.removeprefix("temp:")


def _format_number(value: float) -> str:
    return f"{value:.{SUMMARY_DECIMALS}f}"


def render_markdown(summaries: list[WindowSummary]) -> str:
    """Таблица Markdown на каждое окно (для вставки в отчёт)."""
    blocks: list[str] = []
    for summary in summaries:
        window = summary.window
        bounds = f"{window.from_epoch if window.from_epoch is not None else '-'} … {window.to_epoch if window.to_epoch is not None else '-'}"
        blocks.append(
            f"### {window.name} ({bounds})\n\n"
            f"Замеров: {summary.sample_count}, пропусков: {summary.gap_count}, отказов чтения: {summary.read_failures}\n"
        )
        rows = ["| величина | n | min | среднее | max | начало | конец |", "|---|---|---|---|---|---|---|"]
        for metric_name, stats in summary.metrics.items():
            numbers = (stats.minimum, stats.mean, stats.maximum, stats.first, stats.last)
            rows.append(f"| {_metric_label(metric_name)} | {stats.count} | " + " | ".join(map(_format_number, numbers)) + " |")
        blocks.append("\n".join(rows) + "\n")
    return "\n".join(blocks)


def summaries_to_json(summaries: list[WindowSummary]) -> str:
    payload = [
        {
            "name": s.window.name,
            "from_epoch": s.window.from_epoch,
            "to_epoch": s.window.to_epoch,
            "samples": s.sample_count,
            "gaps": s.gap_count,
            "read_failures": s.read_failures,
            "metrics": {
                name: {
                    "n": st.count, "min": st.minimum, "mean": st.mean, "max": st.maximum, "first": st.first, "last": st.last,
                }
                for name, st in s.metrics.items()
            },
        }
        for s in summaries
    ]
    return json.dumps(payload, ensure_ascii=False, indent=2)
