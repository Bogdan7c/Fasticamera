#!/usr/bin/env python3
"""ПК-сторона зонда Fasticamera (сессия 00 плана user/foundation).

Команды:
  build                         собрать все три варианта APK
  install <flavor>              поставить вариант (сначала проверка pm list packages)
  uninstall <flavor>            удалить наш вариант
  run <flavor> <scenario> [k=v ...]   запустить сценарий, забрать JSONL прогона через run-as
  matrix <flavor> <file.json>   прогнать список экспериментов из файла
  table <dir>                   сводная таблица потоков (C2) по сохранённым прогонам
  pull-captures <flavor> [dest] забрать эталонные снимки C7 (files/captures) и удалить их с телефона
  pull-align <flavor> <run> [dest]  забрать files/align/<run> (зелёные плоскости product_alignment); телефон не чистим

Результаты пишутся в каталог вне репозитория (по умолчанию ~/fasticamera-captures/probe-runs),
в git и user/ они не попадают (правило README плана).
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import secrets
import shlex
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

# --- Конфигурация ---------------------------------------------------------------

PROBE_ROOT = Path(__file__).resolve().parent.parent
ACTIVITY = "dev.fasticamera.probe.ProbeActivity"

# Имя пакета для каждого варианта сборки (productFlavors в app/build.gradle.kts).
FLAVOR_PACKAGES = {
    "control": "dev.fasticamera.probe",
    "oneplus": "com.oneplus.camera",
    "oppo": "com.oppo.camera",
}

# Разрешения, которые зонд получает без диалога (выдаёт ПК-скрипт).
RUNTIME_PERMISSIONS = ["android.permission.CAMERA", "android.permission.RECORD_AUDIO"]

DEFAULT_OUTPUT_DIR = Path(os.environ.get("FASTICAMERA_PROBE_OUT", Path.home() / "fasticamera-captures" / "probe-runs"))
DEFAULT_JAVA_HOME = os.environ.get("JAVA_HOME", "/usr/lib/jvm/java-21-openjdk")
DEFAULT_RUN_TIMEOUT_S = 120
POLL_INTERVAL_S = 1.0


class ProbeError(RuntimeError):
    """Ошибка ПК-скрипта с понятным текстом для человека."""


# --- adb ------------------------------------------------------------------------


def adb(*args: str, check: bool = True, capture: bool = True) -> subprocess.CompletedProcess[str]:
    serial = os.environ.get("ANDROID_SERIAL")
    command = ["adb"] + (["-s", serial] if serial else []) + list(args)
    result = subprocess.run(command, text=True, capture_output=capture)
    if check and result.returncode != 0:
        raise ProbeError(f"adb {' '.join(args)} завершился с кодом {result.returncode}: {result.stderr.strip()}")
    return result


def ensure_single_device() -> None:
    """Правило README: устройство одно или задан ANDROID_SERIAL."""
    if os.environ.get("ANDROID_SERIAL"):
        return
    lines = adb("devices").stdout.strip().splitlines()[1:]
    devices = [line for line in lines if line.strip().endswith("device")]
    if len(devices) != 1:
        raise ProbeError(f"ожидалось одно устройство, найдено {len(devices)}; задайте ANDROID_SERIAL")


def package_path(package: str) -> str | None:
    """Путь APK пакета на телефоне или None, если пакета нет."""
    output = adb("shell", "pm", "path", package, check=False).stdout.strip()
    return output.removeprefix("package:") if output.startswith("package:") else None


# --- Сборка и установка ---------------------------------------------------------


def gradle(*tasks: str) -> None:
    env = dict(os.environ, JAVA_HOME=DEFAULT_JAVA_HOME)
    result = subprocess.run(["./gradlew", "--console=plain", *tasks], cwd=PROBE_ROOT, env=env)
    if result.returncode != 0:
        raise ProbeError(f"gradle {' '.join(tasks)} завершился с кодом {result.returncode}")


def apk_path(flavor: str) -> Path:
    return PROBE_ROOT / "app" / "build" / "outputs" / "apk" / flavor / "debug" / f"app-{flavor}-debug.apk"


def install(flavor: str, skip_native: bool = False) -> None:
    package = FLAVOR_PACKAGES[flavor]
    existing = package_path(package)
    # Защита системы: ставим только если пакета нет или это наш (из /data/app) прошлый вариант.
    if existing is not None and not existing.startswith("/data/app/"):
        raise ProbeError(f"{package} уже установлен как системный ({existing}) — не трогаем")
    # --skip-native: APK без Rust-библиотеки (Kotlin-сценарии работают, нативные честно вернут load_library).
    gradle(f":app:assemble{flavor.capitalize()}Debug", *(["-x", "cargoNdkBuild"] if skip_native else []))
    apk = apk_path(flavor)
    if not apk.exists():
        raise ProbeError(f"нет APK {apk}")
    result = adb("install", "-r", "-g", str(apk), check=False)
    # Отказ установки (например, проверка подписи прошивкой) — факт для отчёта, печатаем как есть.
    print(json.dumps({"install": flavor, "package": package, "returncode": result.returncode,
                      "stdout": result.stdout.strip(), "stderr": result.stderr.strip()}, ensure_ascii=False))
    if result.returncode != 0:
        raise ProbeError(f"установка {package} отклонена")
    for permission in RUNTIME_PERMISSIONS:
        adb("shell", "pm", "grant", package, permission, check=False)


def uninstall(flavor: str) -> None:
    package = FLAVOR_PACKAGES[flavor]
    existing = package_path(package)
    if existing is None:
        print(f"{package} не установлен")
        return
    if not existing.startswith("/data/app/"):
        raise ProbeError(f"{package} — системный ({existing}), удалять запрещено")
    adb("uninstall", package)


# --- Запуск сценария и сбор логов ------------------------------------------------


@dataclass
class RunResult:
    run_id: str
    records: list[dict]
    finished: bool
    elapsed_s: float


def read_run_file(package: str, run_id: str) -> list[dict]:
    """Читает JSONL прогона из files/runs приложения через run-as (работает для debug-сборки)."""
    result = adb("exec-out", "run-as", package, "cat", f"files/runs/{run_id}.jsonl", check=False)
    if result.returncode != 0:
        return []
    records = []
    for line in result.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            records.append(json.loads(line))
        except json.JSONDecodeError:
            # Последняя строка может быть недописана в момент чтения — дочитаем на следующем опросе.
            break
    return records


def run_scenario(flavor: str, scenario: str, params: dict[str, str], timeout_s: int) -> RunResult:
    """Запускает сценарий и ждёт записи `done` в файле прогона.

    Logcat как канал данных не годится: кольцевой буфер на телефоне 256 КиБ, и CamX
    вытесняет из него записи зонда за секунды. Поэтому источник истины — файл.
    """
    package = FLAVOR_PACKAGES[flavor]
    if package_path(package) is None:
        raise ProbeError(f"{package} не установлен: python3 pc/probe.py install {flavor}")
    # Время + случайный суффикс: две попытки за одну секунду не должны делить файл прогона.
    run_id = dt.datetime.now().strftime("%H%M%S") + "-" + secrets.token_hex(3)
    # Принудительно останавливаем прошлый экземпляр: камера должна быть свободна.
    adb("shell", "am", "force-stop", package, check=False)
    extras = {"scenario": scenario, "run": run_id, **params}
    remote = ["am", "start", "-W", "-n", f"{package}/{ACTIVITY}"]
    for key, value in extras.items():
        remote += ["-e", key, value]
    adb("shell", " ".join(shlex.quote(part) for part in remote))
    started = time.monotonic()
    records: list[dict] = []
    finished = False
    while time.monotonic() - started < timeout_s:
        records = read_run_file(package, run_id)
        if any(record.get("kind") == "done" for record in records):
            finished = True
            break
        time.sleep(POLL_INTERVAL_S)
    if not finished:
        # Таймаут: возможно, процесс завис в HAL. Останавливаем, чтобы освободить камеру для следующей попытки.
        adb("shell", "am", "force-stop", package, check=False)
        records = read_run_file(package, run_id)
    return RunResult(run_id, records, finished, time.monotonic() - started)


def save_run(result: RunResult, flavor: str, scenario: str, params: dict[str, str], label: str | None) -> Path:
    day_dir = DEFAULT_OUTPUT_DIR / dt.date.today().isoformat()
    day_dir.mkdir(parents=True, exist_ok=True)
    name = f"{result.run_id}-{flavor}-{scenario}" + (f"-{label}" if label else "")
    path = day_dir / f"{name}.json"
    path.write_text(json.dumps({
        "run_id": result.run_id, "flavor": flavor, "scenario": scenario, "label": label,
        "params": params, "finished": result.finished, "elapsed_s": result.elapsed_s,
        "records": result.records,
    }, ensure_ascii=False, indent=1))
    return path


def pull_captures(flavor: str, destination: Path) -> None:
    """Забирает files/captures приложения tar-потоком через run-as и удаляет их с телефона после проверки."""
    package = FLAVOR_PACKAGES[flavor]
    destination.mkdir(parents=True, exist_ok=True)
    serial = os.environ.get("ANDROID_SERIAL")
    adb_prefix = ["adb"] + (["-s", serial] if serial else [])
    archive = subprocess.run(adb_prefix + ["exec-out", "run-as", package, "tar", "-C", "files", "-c", "captures"],
                             capture_output=True)
    if archive.returncode != 0 or not archive.stdout:
        raise ProbeError(f"не удалось упаковать captures: {archive.stderr.decode(errors='replace').strip()}")
    unpacked = subprocess.run(["tar", "-x", "-C", str(destination)], input=archive.stdout, capture_output=True)
    if unpacked.returncode != 0:
        raise ProbeError(f"tar -x: {unpacked.stderr.decode(errors='replace').strip()}")
    files = sorted(path for path in (destination / "captures").rglob("*") if path.is_file())
    for path in files:
        print(f"{path.stat().st_size:>12}  {path}")
    # Удаляем с телефона только после успешной распаковки: снимки не теряются при сбое передачи.
    adb("shell", f"run-as {shlex.quote(package)} rm -rf files/captures")


def pull_align(flavor: str, run_id: str, destination: Path) -> None:
    """Забирает files/align/<run> (кадры замера совмещения) tar-потоком через run-as; файлы на телефоне остаются."""
    package = FLAVOR_PACKAGES[flavor]
    destination.mkdir(parents=True, exist_ok=True)
    serial = os.environ.get("ANDROID_SERIAL")
    adb_prefix = ["adb"] + (["-s", serial] if serial else [])
    archive = subprocess.run(adb_prefix + ["exec-out", "run-as", package, "tar", "-C", f"files/align/{run_id}", "-c", "."],
                             capture_output=True)
    if archive.returncode != 0 or not archive.stdout:
        raise ProbeError(f"не удалось упаковать align/{run_id}: {archive.stderr.decode(errors='replace').strip()}")
    unpacked = subprocess.run(["tar", "-x", "-C", str(destination)], input=archive.stdout, capture_output=True)
    if unpacked.returncode != 0:
        raise ProbeError(f"tar -x: {unpacked.stderr.decode(errors='replace').strip()}")
    for path in sorted(p for p in destination.rglob("*") if p.is_file()):
        print(f"{path.stat().st_size:>12}  {path}")


# --- Сводка -----------------------------------------------------------------------


def summarize_stream(record: dict) -> dict:
    """Строка таблицы C2 из записи `stream`/`import_stream`."""
    data = record.get("data", {})
    config = data.get("config", {})
    stream = data.get("stream", {})
    image_timing = stream.get("image_timing", {})
    result_timing = stream.get("result_timing", {})
    first = stream.get("first_result") or {}
    return {
        "camera": config.get("camera_id"),
        "physical": config.get("physical_camera_id"),
        "format": config.get("format"),
        "size": f"{config.get('width')}x{config.get('height')}",
        "session_type": config.get("session_type"),
        "ae_fps": config.get("ae_target_fps"),
        "manual": config.get("manual_exposure"),
        "keys": (config.get("session_keys") or []) + (config.get("request_keys") or []),
        "stage": data.get("stage_reached"),
        "open": (data.get("open") or {}).get("status"),
        "session": (data.get("session") or {}).get("status"),
        "img_fps": round(image_timing.get("fps_median", 0.0), 2) if image_timing.get("status") == "computed" else None,
        "res_fps": round(result_timing.get("fps_median", 0.0), 2) if result_timing.get("status") == "computed" else None,
        "images": stream.get("images_received"),
        "drops": image_timing.get("estimated_dropped_frames"),
        "frame_dur": (stream.get("frame_duration_ns") or {}).get("distinct_sample"),
        "binning": stream.get("raw_binning_factor_used"),
        "vendor": stream.get("vendor_result_values"),
        "first_fps_range": first.get("ae_target_fps_range"),
        "errors": data.get("request_key_errors") or data.get("stream_exception") or data.get("raw_reader_error"),
    }


def summarize_sensitivity(data: dict) -> list[str]:
    """Строки замера чувствительности (`product_sensitivity`): EV основная-минус-ширик по ступеням."""
    lines = []
    static = data.get("static")
    if static:
        lines.append(f"expected ev: aperture={static.get('expected_ev_aperture_only'):.3f} "
                     f"aperture+pixel_area={static.get('expected_ev_aperture_and_pixel_area'):.3f} "
                     f"zoom_ratio_wide={static.get('zoom_ratio_wide'):.3f}")
    for step in data.get("steps") or []:
        label = f"exp={step['requested_exposure_ns']} iso={step['requested_iso']}"
        ev = step.get("ev_main_minus_wide")
        if ev:
            lines.append(f"ev {label}: " + " ".join(f"{name}={ev[name]:+.3f}" for name in ("R", "Gr", "Gb", "B", "green_avg")))
        else:
            lines.append(f"ev {label}: {step.get('status')}")
        for camera in ("main", "wide"):
            stats = step.get(camera)
            if stats:
                lines.append(f"  {camera}: exp={stats['actual_exposure_ns']} iso={stats['actual_iso']} used={stats['frames_used']}/"
                             f"{stats['frames_collected']} clip={stats['clipped_fraction_max_channel']:.4f} af={stats['af_state']}")
    return lines


def summarize_alignment(data: dict) -> list[str]:
    """Строки сводки `product_alignment`: ожидание AF, перекос меток и применённые параметры каждой пары."""
    if "pairs" not in data:
        return []
    lines = [f"alignment: af_wait={data.get('af_wait')} applied_after_ms={data.get('applied_after_ms')} "
             f"wide_exposure_factor={data.get('wide_exposure_factor')}"]
    for pair in data["pairs"]:
        frames = pair.get("frames", {})
        parts = [f"{label}: clip={meta['clipped_fraction']:.4f} mean={meta['mean_level_black_subtracted']:.1f} "
                 f"diopt={meta['result']['focus_distance_diopters']} af={meta['result']['af_state']}"
                 for label, meta in frames.items()]
        lines.append(f"  pair{pair['index']}: skew_ms={pair.get('skew_ms')} " + " | ".join(parts) + f" {pair.get('error', '')}")
    return lines


def summarize_dual(record: dict) -> list[str]:
    """Строки сводки сценария `dual` (сессия 13F): по одной на ногу/попытку."""
    data = record.get("data", {})
    lines = []
    for entry in data.get("open") or []:
        lines.append(f"open cam{entry.get('camera')}: {entry.get('status')} {entry.get('code_name', '')} {round(entry.get('ms', 0))} ms")
    for entry in data.get("session") or []:
        if isinstance(entry, dict):
            lines.append(f"session cam{entry.get('camera')}: {entry.get('status')} {round(entry.get('ms', 0))} ms")
    if data.get("device_errors_after_open") or data.get("device_errors_final"):
        lines.append(f"device_errors: {data.get('device_errors_after_open')} -> {data.get('device_errors_final')}")
    for camera in data.get("cameras") or []:
        results = camera.get("results", {})
        for leg in camera.get("legs", []):
            timing = leg.get("timing", {})
            fps = round(timing["fps_median"], 1) if timing.get("status") == "computed" else None
            lines.append(f"leg {leg['leg']}: frames={leg['frames']} fps={fps} drops={timing.get('estimated_dropped_frames')} "
                         f"acq_err={leg['acquire_errors']}")
        lines.append(f"  results cam{camera['camera']}: n={results.get('count')} failures={results.get('failures')} "
                     f"lost={results.get('buffers_lost')} af={results.get('af_transitions')} physical={results.get('physical_result_counts')}")
    if data.get("steps") is not None:
        lines.extend(summarize_sensitivity(data))
    lines.extend(summarize_alignment(data))
    if data.get("current"):
        lines.append(f"current: {json.dumps(data['current'], ensure_ascii=False)}")
    if data.get("switch_ms_summary"):
        for variant, stat in data["switch_ms_summary"].items():
            if stat:
                lines.append(f"switch {variant}: median={stat['median']:.0f} min={stat['min']:.0f} max={stat['max']:.0f} ms (n={stat['count']})")
            else:
                lines.append(f"switch {variant}: нет успешных попыток")
        for attempt in data.get("attempts", []):
            if attempt.get("failed_at") or attempt.get("exception"):
                lines.append(f"  rep{attempt['repetition']} {attempt['variant']}: сбой {attempt.get('failed_at')} {attempt.get('exception', '')}")
    if data.get("total_ms_summary"):
        stat = data["total_ms_summary"]
        lines.append(f"standby open->first frame: median={stat['median']:.0f} min={stat['min']:.0f} max={stat['max']:.0f} ms (n={stat['count']})")
        for attempt in data.get("attempts", []):
            lines.append(f"  rep{attempt['repetition']}: open={attempt.get('open_ms')} session={attempt.get('session_done_ms')} "
                         f"first={attempt.get('first_frame_ms')} {attempt.get('failed_at', '')}")
    for window in data.get("windows") or []:
        cur = window.get("current", {})
        med = (cur.get("current_now_raw") or {}).get("median")
        lines.append(f"window{window['index']} {window['kind']}: median={med} distinct={cur.get('distinct_values')} "
                     f"frames={window.get('frames_main')}/{window.get('frames_wide')}")
    if "b_minus_a_of_means" in data:
        lines.append(f"ABBA: {data.get('window_median_summary')} B-A={data['b_minus_a_of_means']}")
    if data.get("skipped"):
        lines.append(f"skipped: {data['skipped']}")
    for key in ("failed_step", "exception", "stream_exception", "stage_reached"):
        if data.get(key):
            lines.append(f"{key}: {data[key]}")
    return lines


def print_run_summary(result: RunResult) -> None:
    status = "done" if result.finished else "НЕТ done (таймаут или падение процесса)"
    print(f"run {result.run_id}: {status}, записей {len(result.records)}, {result.elapsed_s:.1f} c")
    for record in result.records:
        kind, name = record.get("kind"), record.get("name")
        if kind == "error":
            print(f"  ERROR {name}: {json.dumps(record.get('data'), ensure_ascii=False)}")
        elif name in ("stream", "import_stream"):
            print("  " + json.dumps(summarize_stream(record), ensure_ascii=False))
        elif name == "dual":
            print("\n".join("  " + line for line in summarize_dual(record)))


def table(directory: Path) -> None:
    rows = []
    for path in sorted(directory.glob("*.json")):
        saved = json.loads(path.read_text())
        for record in saved["records"]:
            if record.get("name") == "dual":
                print(f"## {saved.get('label') or path.stem} ({saved.get('params')})")
                print("\n".join("- " + line for line in summarize_dual(record)))
            if record.get("name") in ("stream", "import_stream"):
                rows.append((saved.get("flavor"), saved.get("label"), summarize_stream(record)))
    header = "| flavor | label | cam | fmt | size | session | ae_fps | keys | stage | img_fps | res_fps | drops | frame_dur | vendor |"
    print(header)
    print("|" + "---|" * (header.count("|") - 1))
    for flavor, label, row in rows:
        print(f"| {flavor} | {label} | {row['camera']}{('/' + row['physical']) if row['physical'] else ''} | {row['format']} | "
              f"{row['size']} | {row['session_type']} | {row['ae_fps']} | {'; '.join(row['keys'])} | "
              f"{row['stage']}/{row['session']} | {row['img_fps']} | {row['res_fps']} | {row['drops']} | "
              f"{row['frame_dur']} | {json.dumps(row['vendor'], ensure_ascii=False)} |")


# --- CLI --------------------------------------------------------------------------


def parse_params(pairs: list[str]) -> dict[str, str]:
    params = {}
    for pair in pairs:
        if "=" not in pair:
            raise ProbeError(f"параметр '{pair}' ожидается как ключ=значение")
        key, value = pair.split("=", 1)
        params[key] = value
    return params


def main() -> int:
    parser = argparse.ArgumentParser(description="ПК-сторона зонда Fasticamera")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("build")
    install_parser = sub.add_parser("install")
    install_parser.add_argument("flavor", choices=FLAVOR_PACKAGES)
    install_parser.add_argument("--skip-native", action="store_true")
    sub.add_parser("uninstall").add_argument("flavor", choices=FLAVOR_PACKAGES)
    run_parser = sub.add_parser("run")
    run_parser.add_argument("flavor", choices=FLAVOR_PACKAGES)
    run_parser.add_argument("scenario")
    run_parser.add_argument("params", nargs="*")
    run_parser.add_argument("--label")
    run_parser.add_argument("--timeout", type=int, default=DEFAULT_RUN_TIMEOUT_S)
    matrix_parser = sub.add_parser("matrix")
    matrix_parser.add_argument("flavor", choices=FLAVOR_PACKAGES)
    matrix_parser.add_argument("file", type=Path)
    matrix_parser.add_argument("--only", help="метки через запятую")
    pull_parser = sub.add_parser("pull-captures")
    pull_parser.add_argument("flavor", choices=FLAVOR_PACKAGES)
    pull_parser.add_argument("destination", type=Path, nargs="?", default=DEFAULT_OUTPUT_DIR.parent / "c7-reference")
    align_parser = sub.add_parser("pull-align")
    align_parser.add_argument("flavor", choices=FLAVOR_PACKAGES)
    align_parser.add_argument("run_id")
    align_parser.add_argument("destination", type=Path)
    table_parser = sub.add_parser("table")
    table_parser.add_argument("directory", type=Path)
    args = parser.parse_args()

    try:
        if args.command == "table":
            table(args.directory)
            return 0
        if args.command == "build":
            gradle(*(f":app:assemble{flavor.capitalize()}Debug" for flavor in FLAVOR_PACKAGES))
            return 0
        ensure_single_device()
        if args.command == "install":
            install(args.flavor, args.skip_native)
        elif args.command == "uninstall":
            uninstall(args.flavor)
        elif args.command == "pull-captures":
            pull_captures(args.flavor, args.destination)
        elif args.command == "pull-align":
            pull_align(args.flavor, args.run_id, args.destination)
        elif args.command == "run":
            params = parse_params(args.params)
            result = run_scenario(args.flavor, args.scenario, params, args.timeout)
            path = save_run(result, args.flavor, args.scenario, params, args.label)
            print_run_summary(result)
            print(f"сохранено: {path}")
            return 0 if result.finished else 2
        elif args.command == "matrix":
            experiments = json.loads(args.file.read_text())
            only = set(args.only.split(",")) if args.only else None
            for experiment in experiments:
                if only and experiment["label"] not in only:
                    continue
                params = {key: str(value) for key, value in experiment["params"].items()}
                print(f"== {experiment['label']}: {experiment.get('why', '')}")
                result = run_scenario(args.flavor, experiment.get("scenario", "stream"), params,
                                      int(experiment.get("timeout_s", DEFAULT_RUN_TIMEOUT_S)))
                path = save_run(result, args.flavor, experiment.get("scenario", "stream"), params, experiment["label"])
                print_run_summary(result)
                print(f"   сохранено: {path}")
        return 0
    except ProbeError as error:
        print(f"ошибка: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
