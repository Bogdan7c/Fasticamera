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

import subprocess
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


@dataclass(frozen=True)
class Step:
    """Один шаг; ``seconds`` нужен только шагу ``PAUSE``."""

    kind: StepKind
    seconds: float = 0.0


SCENARIO_STEPS: dict[str, dict[str, tuple[Step, ...]]] = {
    "lifecycle": {
        "ready": (Step(StepKind.PRESS_HOME),),
        "backgrounded": (Step(StepKind.PAUSE, BACKGROUND_PAUSE_SECONDS), Step(StepKind.RETURN_TO_APP)),
    },
    "permission": {
        "dialog_expected": (Step(StepKind.DENY_PERMISSION_DIALOG),),
    },
}
"""Сценарий → стадия → шаги. Стадии вне таблицы (например ``no_dialog_expected``) ничего не вызывают."""


class DeviceActions(Protocol):
    """Что драйвер умеет делать с телефоном (границу подменяют в тестах)."""

    def press_home(self) -> None: ...

    def pause(self, seconds: float) -> None: ...

    def return_to_app(self) -> None: ...

    def deny_permission_dialog(self) -> None: ...


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
        else:  # pragma: no cover - новый StepKind без ветки должен падать громко
            raise DriverError(f"неизвестный шаг {step.kind!r}")
