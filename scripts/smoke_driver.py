"""Драйвер шагов smoke-сценариев: на стадию из logcat выполняет действие на телефоне.

Rust-часть сценария пишет строки ``FASTICAMERA_STAGE {json}`` и ждёт, пока скрипт сделает своё
(свернёт приложение, нажмёт кнопку диалога). Что делать на какой стадии — данные в ``SCENARIO_STEPS``,
а не ``if`` по коду. Устройство отделено интерфейсом ``DeviceActions``: настоящий вариант
(``AdbDeviceActions``) гоняет ``adb``, тесты подставляют фейк.

Устройство:
* ``Step`` / ``SCENARIO_STEPS`` — таблица «сценарий → стадия → шаги»;
* ``AdbDeviceActions`` — шаги через ``adb -s <serial>`` (serial всегда явный);
* ``StageDriver`` — получает стадию, находит шаги, выполняет их по порядку.
"""

from __future__ import annotations

import re
import subprocess
import sys
import time
from collections.abc import Callable
from dataclasses import dataclass
from enum import Enum
from typing import Protocol

from smoke_ui import TapPoint, UiDumpParseError, find_deny_button

STAGE_MARKER = "FASTICAMERA_STAGE "
"""Префикс строки стадии; контракт с Rust (``crates/fasticamera-app/src/scenario/``)."""

MAIN_ACTIVITY = "dev.fasticamera.MainActivity"
"""Activity приложения; namespace не зависит от суффикса пакета (как ``MAIN_ACTIVITY`` в common.sh)."""

BACKGROUND_PAUSE_SECONDS = 1.5
"""Сколько приложение остаётся свёрнутым перед возвратом: дать системе довести Paused/SurfaceDestroyed."""

SCREEN_TRANSITION_PAUSE_SECONDS = 1.0
"""Пауза после гашения и включения экрана: дать системе довести экран до нового состояния (это ввод, не смена настроек)."""

WAKEFULNESS_TIMEOUT_SECONDS = 3.0
"""Сколько ждать смены состояния питания после нажатия (по факту телефона surface отпускается за секунды)."""

WAKEFULNESS_POLL_INTERVAL_SECONDS = 0.2
"""Период опроса ``dumpsys power`` при ожидании смены состояния питания."""

WAKEFULNESS_AWAKE = "Awake"
"""Значение ``mWakefulness`` у включённого экрана; остальные (Asleep, Dozing, Dreaming) — не «бодрствует»."""

DIALOG_TIMEOUT_SECONDS = 10.0
"""Сколько ждать появления системного диалога разрешения."""

DIALOG_POLL_INTERVAL_SECONDS = 0.5
"""Период опроса дампа иерархии окна."""

ADB_COMMAND_TIMEOUT_SECONDS = 30.0
"""Потолок одной команды adb: зависший adb не должен вешать драйвер навсегда."""


class DriverError(Exception):
    """Шаг не удалось выполнить; сообщение уходит в вердикт сценария (код выхода 1)."""


class StepKind(Enum):
    """Виды шагов драйвера."""

    PRESS_HOME = "press_home"
    PAUSE = "pause"
    RETURN_TO_APP = "return_to_app"
    DENY_PERMISSION_DIALOG = "deny_permission_dialog"
    SCREEN_OFF = "screen_off"
    SCREEN_ON = "screen_on"
    TAP_SCREEN = "tap_screen"


@dataclass(frozen=True)
class Step:
    """Один шаг; ``seconds`` нужен только шагу ``PAUSE``.

    ``only_if_true`` — имя булева поля стадии: шаг выполняется, только если поле есть и равно
    ``true`` (иначе пропускается). Так стадия, общая для разных ситуаций, управляет шагом данными.
    """

    kind: StepKind
    seconds: float = 0.0
    only_if_true: str | None = None


SCENARIO_STEPS: dict[str, dict[str, tuple[Step, ...]]] = {
    # При подключении частота экрана нестабильна: OxygenOS держит для приложения 60 Гц ДО ПЕРВОГО
    # касания (проверено dumpsys на OnePlus 11), и если цель выше (120 Гц), экран «медленнее цели».
    # Пока частота не сошлась, Rust раз в ``render_refresh_retry_ms`` пишет стадию ``awaiting_refresh``;
    # касание в центр экрана делается ТОЛЬКО когда ``observed_slower_than_expected`` истинно
    # (при цели 60 Гц и в случае «экран быстрее цели» касание ничего не даст и не шлётся).
    # Каждая повторная стадия отрабатывается заново: касание, не снявшее колпак, повторяется.
    # Rust не видит, было ли касание: оно записано здесь и в отчёте (``refresh.synthetic_touch_requested``).
    "render": {
        "awaiting_refresh": (Step(StepKind.TAP_SCREEN, only_if_true="observed_slower_than_expected"),),
    },
    "lifecycle": {
        "ready": (Step(StepKind.PRESS_HOME),),
        "backgrounded": (Step(StepKind.PAUSE, BACKGROUND_PAUSE_SECONDS), Step(StepKind.RETURN_TO_APP)),
    },
    "permission": {
        "dialog_expected": (Step(StepKind.DENY_PERMISSION_DIALOG),),
    },
    # Rust ведёт счёт циклов и на каждом возобновлении показа пишет стадию ``presenting_*``;
    # после подтверждённого освобождения surface — ``backgrounded_*``.
    "render-lifecycle": {
        "presenting_home": (Step(StepKind.PRESS_HOME),),
        "backgrounded_home": (Step(StepKind.PAUSE, BACKGROUND_PAUSE_SECONDS), Step(StepKind.RETURN_TO_APP)),
        "presenting_screen": (Step(StepKind.SCREEN_OFF),),
        "backgrounded_screen": (Step(StepKind.PAUSE, BACKGROUND_PAUSE_SECONDS), Step(StepKind.SCREEN_ON)),
    },
}
"""Сценарий → стадия → шаги. Стадии вне таблицы (например ``no_dialog_expected``) ничего не вызывают."""


class DeviceActions(Protocol):
    """Что драйвер умеет делать с телефоном (границу подменяют в тестах)."""

    def press_home(self) -> None: ...

    def pause(self, seconds: float) -> None: ...

    def return_to_app(self) -> None: ...

    def deny_permission_dialog(self) -> None: ...

    def screen_off(self) -> None: ...

    def screen_on(self) -> None: ...

    def tap_screen(self) -> None: ...


def parse_wakefulness(output: str) -> str:
    """Значение ``mWakefulness=`` из вывода ``dumpsys power`` (Awake / Asleep / Dozing / Dreaming)."""
    match = re.search(r"mWakefulness=(\w+)", output)
    if match is None:
        raise DriverError("не удалось разобрать mWakefulness из вывода dumpsys power")
    return match.group(1)


@dataclass(frozen=True)
class AdbResult:
    """Итог одной команды adb."""

    returncode: int
    stdout: str
    stderr: str


AdbRunner = Callable[[list[str]], AdbResult]
"""Выполнитель команд: полная командная строка (с ``adb``) → результат."""


def run_adb_subprocess(command: list[str]) -> AdbResult:
    """Настоящий выполнитель: подпроцесс с потолком времени."""
    try:
        completed = subprocess.run(
            command, capture_output=True, text=True, errors="replace", timeout=ADB_COMMAND_TIMEOUT_SECONDS
        )
    except subprocess.TimeoutExpired as error:
        raise DriverError(f"команда зависла (> {ADB_COMMAND_TIMEOUT_SECONDS:g} с): {' '.join(command)}") from error
    return AdbResult(completed.returncode, completed.stdout, completed.stderr)


def parse_wm_size(output: str) -> tuple[int, int]:
    """Размер экрана из вывода ``wm size``: ``Override size`` важнее ``Physical size``."""
    sizes: dict[str, tuple[int, int]] = {}
    for line in output.splitlines():
        match = re.fullmatch(r"\s*(Physical|Override) size:\s*(\d+)x(\d+)\s*", line)
        if match:
            sizes[match.group(1)] = (int(match.group(2)), int(match.group(3)))
    size = sizes.get("Override") or sizes.get("Physical")
    if size is None or size[0] <= 0 or size[1] <= 0:
        raise DriverError(f"не удалось разобрать размер экрана из вывода wm size: {output.strip()!r}")
    return size


class AdbDeviceActions:
    """Шаги на телефоне через ``adb -s <serial>``. Все команды строятся в одном месте (``adb_command``)."""

    def __init__(
        self,
        serial: str,
        package: str,
        run_adb: AdbRunner = run_adb_subprocess,
        sleep: Callable[[float], None] = time.sleep,
        clock: Callable[[], float] = time.monotonic,
    ) -> None:
        self._serial = serial
        self._package = package
        self._run_adb = run_adb
        self._sleep = sleep
        self._clock = clock

    def adb_command(self, *arguments: str) -> list[str]:
        """Командная строка adb с явным serial: на столе может лежать второй телефон."""
        return ["adb", "-s", self._serial, *arguments]

    def press_home(self) -> None:
        """Сворачивает приложение кнопкой «домой»."""
        result = self._run_adb(self.adb_command("shell", "input", "keyevent", "KEYCODE_HOME"))
        self._require_success(result, "KEYCODE_HOME")

    def pause(self, seconds: float) -> None:
        self._sleep(seconds)

    def screen_off(self) -> None:
        """Гасит экран кнопкой питания (``KEYCODE_POWER``): приложение остаётся в задаче.

        Факт OnePlus 11 / OxygenOS: ``KEYCODE_SLEEP`` через ``input keyevent`` молча игнорируется
        (``mWakefulness`` остаётся ``Awake``, surface не уничтожается), а ``KEYCODE_POWER`` работает.
        Но POWER — переключатель: на уже погасшем экране он его ВКЛЮЧИТ, поэтому нажимаем только
        когда устройство бодрствует. Сценарий просит погасить экран в момент, когда он обязан быть
        включён; если это не так (Asleep/Dozing), это сбой стенда, а не «уже готово»: тихий возврат
        дал бы вводящий в заблуждение таймаут Rust-сценария позже. Поэтому — ``DriverError`` и без
        нажатия. После нажатия ждём, пока состояние реально перестанет быть Awake, иначе
        ``DriverError``.
        """
        wakefulness = self._read_wakefulness()
        if wakefulness != WAKEFULNESS_AWAKE:
            raise DriverError(
                f"экран нельзя погасить: он уже не включён (mWakefulness={wakefulness}); "
                "KEYCODE_POWER не нажат, чтобы не включить его обратно"
            )
        result = self._run_adb(self.adb_command("shell", "input", "keyevent", "KEYCODE_POWER"))
        self._require_success(result, "KEYCODE_POWER")
        self._wait_for_wakefulness(awake=False)
        self._sleep(SCREEN_TRANSITION_PAUSE_SECONDS)

    def screen_on(self) -> None:
        """Включает экран (``KEYCODE_WAKEUP``, идемпотентна) и ждёт ``mWakefulness=Awake``.

        При блокировке экрана приложение (showWhenLocked) заново получает surface поверх keyguard.
        """
        result = self._run_adb(self.adb_command("shell", "input", "keyevent", "KEYCODE_WAKEUP"))
        self._require_success(result, "KEYCODE_WAKEUP")
        self._wait_for_wakefulness(awake=True)
        self._sleep(SCREEN_TRANSITION_PAUSE_SECONDS)

    def _read_wakefulness(self) -> str:
        """Состояние питания из ``dumpsys power``; не нашли строку — ``DriverError`` (гадать нельзя: POWER переключает)."""
        result = self._run_adb(self.adb_command("shell", "dumpsys", "power"))
        self._require_success(result, "dumpsys power")
        return parse_wakefulness(result.stdout)

    def _wait_for_wakefulness(self, awake: bool) -> None:
        """Опрашивает состояние питания, пока оно не станет нужным (``awake``) или не выйдет срок."""
        deadline = self._clock() + WAKEFULNESS_TIMEOUT_SECONDS
        while True:
            wakefulness = self._read_wakefulness()
            if (wakefulness == WAKEFULNESS_AWAKE) == awake:
                return
            if self._clock() >= deadline:
                wanted = "Awake" if awake else "не Awake"
                raise DriverError(
                    f"за {WAKEFULNESS_TIMEOUT_SECONDS:g} с состояние питания не стало {wanted} (mWakefulness={wakefulness})"
                )
            self._sleep(WAKEFULNESS_POLL_INTERVAL_SECONDS)

    def tap_screen(self) -> None:
        """Касание в центр экрана (``input tap``), размер берётся из ``wm size``.

        Центр безопасен: приложение на весь экран не имеет кнопок, касание попадает в нашу
        surface и нужно только системе (первое касание снимает ограничение 60 Гц). Если задан
        ``Override size`` (``wm size WxH``), он важнее физического размера. Не разобрали вывод —
        ``DriverError``, а не касание «наугад».
        """
        result = self._run_adb(self.adb_command("shell", "wm", "size"))
        self._require_success(result, "wm size")
        width_px, height_px = parse_wm_size(result.stdout)
        self._tap(TapPoint(width_px // 2, height_px // 2))

    def return_to_app(self) -> None:
        """Возвращает приложение лаунчер-интентом БЕЗ ``-S``: поднимается существующая задача,
        ``onCreate`` не повторяется (это и проверяет сценарий через ``activity_recreated``)."""
        result = self._run_adb(
            self.adb_command(
                "shell", "am", "start", "-W",
                "-a", "android.intent.action.MAIN",
                "-c", "android.intent.category.LAUNCHER",
                "-n", f"{self._package}/{MAIN_ACTIVITY}",
            )
        )
        self._require_success(result, "возврат приложения (am start)")
        # am start печатает ошибки в stdout и при нулевом коде возврата.
        if any(line.startswith(("Error", "Exception")) for line in result.stdout.splitlines()):
            raise DriverError(f"am start не вернул приложение: {result.stdout.strip()}")

    def deny_permission_dialog(self) -> None:
        """Опрашивает дамп окна, пока не появится кнопка отказа, и нажимает её.

        Дамп уходит в stdout (``/dev/tty``) и нигде не сохраняется: в нём может быть чужой текст экрана.
        """
        deadline = self._clock() + DIALOG_TIMEOUT_SECONDS
        last_problem = "диалог не появлялся"
        while True:
            tap_point, last_problem = self._look_for_deny_button(last_problem)
            if tap_point is not None:
                self._tap(tap_point)
                return
            if self._clock() >= deadline:
                break
            self._sleep(DIALOG_POLL_INTERVAL_SECONDS)
        raise DriverError(
            f"кнопка отказа в диалоге разрешения не найдена за {DIALOG_TIMEOUT_SECONDS:g} с ({last_problem}); "
            "экран заблокирован? диалог другого вида?"
        )

    def _look_for_deny_button(self, previous_problem: str) -> tuple[TapPoint | None, str]:
        """Одна попытка найти кнопку. Возвращает (точка | None, причина неудачи для сообщения)."""
        result = self._run_adb(self.adb_command("exec-out", "uiautomator", "dump", "/dev/tty"))
        if result.returncode != 0:
            return None, f"uiautomator dump завершился с кодом {result.returncode}: {result.stderr.strip()}"
        try:
            tap_point = find_deny_button(result.stdout)
        except UiDumpParseError as error:
            return None, str(error)
        if tap_point is None:
            return None, "кнопки отказа нет в иерархии окна"
        return tap_point, previous_problem

    def _tap(self, tap_point: TapPoint) -> None:
        result = self._run_adb(self.adb_command("shell", "input", "tap", str(tap_point.x), str(tap_point.y)))
        self._require_success(result, f"нажатие в ({tap_point.x}, {tap_point.y})")

    @staticmethod
    def _require_success(result: AdbResult, what: str) -> None:
        if result.returncode != 0:
            raise DriverError(f"{what}: adb завершился с кодом {result.returncode}: {result.stderr.strip()}")


class StageDriver:
    """Выполняет шаги сценария на его стадиях. Стадии чужих сценариев и неизвестные игнорирует."""

    def __init__(
        self,
        scenario: str,
        actions: DeviceActions,
        steps_table: dict[str, dict[str, tuple[Step, ...]]] | None = None,
    ) -> None:
        table = SCENARIO_STEPS if steps_table is None else steps_table
        self._scenario = scenario
        self._actions = actions
        self._steps_by_stage = table.get(scenario, {})

    def handle_stage(self, stage_event: dict) -> None:
        """Обработчик одной стадии (уже разобранный JSON). Ошибка шага — ``DriverError``."""
        if stage_event.get("scenario") != self._scenario:
            return
        for step in self._steps_by_stage.get(str(stage_event.get("stage")), ()):
            if step.only_if_true is not None and stage_event.get(step.only_if_true) is not True:
                continue
            self._run_step(step)

    def _run_step(self, step: Step) -> None:
        if step.kind is StepKind.PRESS_HOME:
            self._actions.press_home()
        elif step.kind is StepKind.PAUSE:
            self._actions.pause(step.seconds)
        elif step.kind is StepKind.RETURN_TO_APP:
            self._actions.return_to_app()
        elif step.kind is StepKind.DENY_PERMISSION_DIALOG:
            self._actions.deny_permission_dialog()
        elif step.kind is StepKind.SCREEN_OFF:
            self._actions.screen_off()
        elif step.kind is StepKind.SCREEN_ON:
            self._actions.screen_on()
        elif step.kind is StepKind.TAP_SCREEN:
            self._actions.tap_screen()
        else:  # pragma: no cover - новый StepKind без ветки должен падать громко
            raise DriverError(f"неизвестный шаг {step.kind!r}")
