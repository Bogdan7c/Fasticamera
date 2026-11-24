"""Фоновые жесты драйвера: нажатие, которое длится дольше, чем драйвер вправе ждать (сессия 16a).

Строка ``display_120hz`` сценария ``viewfinder-stage-budget`` просит удерживать палец на экране целое окно замера
(минуту). Драйвер выполняет шаги СИНХРОННО из цикла чтения logcat, поэтому обычное ``input swipe x y x y 60000``
заблокировало бы чтение на минуту: следующие стадии сценария копились бы в очереди, а потолок одной команды adb
(``ADB_COMMAND_TIMEOUT_SECONDS`` = 30 с) обрубил бы её на полпути.

Здесь команда запускается ФОНОВЫМ процессом (одна команда ``input`` на всё нажатие — повторные запуски ``input``
загрязняли бы ток всплесками CPU), драйвер сразу возвращается к logcat, а в конце прогона ``wait_all`` дожидается
процессов и проверяет, что нажатие завершилось успешно, а не оборвалось. Модуль не знает adb: ему отдают готовую
командную строку и способ её запустить, поэтому он проверяется на ПК без телефона.
"""

from __future__ import annotations

import subprocess
import time
from collections.abc import Callable
from dataclasses import dataclass
from typing import Protocol


class BackgroundGestureError(Exception):
    """Фоновое нажатие не удалось (процесс упал или завис); драйвер превращает её в ``DriverError``."""


class BackgroundProcess(Protocol):
    """Что нужно от запущенного процесса (``subprocess.Popen`` подходит; тесты подставляют фейк)."""

    def poll(self) -> int | None: ...

    def wait(self, timeout: float | None = None) -> int: ...

    def kill(self) -> None: ...

    def stderr_text(self) -> str: ...


SpawnProcess = Callable[[list[str]], BackgroundProcess]
"""Запуск командной строки в фоне: ``Popen`` без ожидания."""


class _PopenProcess:
    """Обёртка над ``subprocess.Popen``: добавляет чтение stderr для сообщения об ошибке."""

    def __init__(self, command: list[str]) -> None:
        self._popen = subprocess.Popen(  # noqa: S603 (команда собрана драйвером из чисел и serial)
            command, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, errors="replace"
        )

    def poll(self) -> int | None:
        return self._popen.poll()

    def wait(self, timeout: float | None = None) -> int:
        return self._popen.wait(timeout=timeout)

    def kill(self) -> None:
        self._popen.kill()

    def stderr_text(self) -> str:
        if self._popen.stderr is None:
            return ""
        return self._popen.stderr.read().strip()


def spawn_subprocess(command: list[str]) -> BackgroundProcess:
    """Настоящий запуск: ``Popen`` без ожидания."""
    return _PopenProcess(command)


@dataclass
class _Running:
    process: BackgroundProcess
    description: str
    deadline: float
    """Момент (по часам драйвера), к которому нажатие обязано кончиться: старт + длительность + запас."""


class BackgroundGestures:
    """Фоновые нажатия одного прогона: запустить, не потерять сбой, дождаться в конце."""

    GRACE_SECONDS = 15.0
    """Запас сверх длительности нажатия: запуск ``input`` на телефоне и задержка adb (по факту — секунды)."""

    def __init__(
        self,
        spawn: SpawnProcess = spawn_subprocess,
        clock: Callable[[], float] = time.monotonic,
    ) -> None:
        self._spawn = spawn
        self._clock = clock
        self._running: list[_Running] = []

    def start(self, command: list[str], description: str, duration_seconds: float) -> None:
        """Запускает ``command`` в фоне. Сначала проверяет, что прежние нажатия не упали: сбой нельзя терять молча."""
        self.raise_if_any_failed()
        process = self._spawn(command)
        self._running.append(
            _Running(process, description, self._clock() + duration_seconds + self.GRACE_SECONDS)
        )

    def raise_if_any_failed(self) -> None:
        """Завершившееся с ошибкой нажатие — ``BackgroundGestureError``; завершившееся успешно забывается."""
        still_running: list[_Running] = []
        for running in self._running:
            code = running.process.poll()
            if code is None:
                still_running.append(running)
            elif code != 0:
                raise BackgroundGestureError(self._failure_text(running, code))
        self._running = still_running

    def wait_all(self) -> None:
        """Конец прогона: дожидается всех нажатий. Зависшее дольше срока убивается и считается сбоем."""
        pending, self._running = self._running, []
        for running in pending:
            remaining = max(running.deadline - self._clock(), 0.0)
            try:
                code = running.process.wait(timeout=remaining)
            except subprocess.TimeoutExpired as error:
                running.process.kill()
                raise BackgroundGestureError(
                    f"{running.description}: не закончилось к сроку (+{self.GRACE_SECONDS:g} с запаса), процесс убит"
                ) from error
            if code != 0:
                raise BackgroundGestureError(self._failure_text(running, code))

    @staticmethod
    def _failure_text(running: _Running, code: int) -> str:
        detail = running.process.stderr_text()
        suffix = f": {detail}" if detail else ""
        return f"{running.description}: фоновая команда завершилась с кодом {code}{suffix}"
