"""Чтение logcat smoke-прогона, которое переживает обрыв беспроводного ``adb``.

Зачем: в сессии 07 беспроводной ``adb`` потерял телефон посреди 10-минутного стенда V4.
Подпроцесс ``adb logcat`` закончился, ожидание решило «строки результата нет», и прогон пропал,
хотя приложение на телефоне продолжало работать и позже записало результат в logcat.

Что делает модуль (только логика, без подпроцессов — их передаёт вызывающий):
* читает «отрезок» logcat, пока поток не закончится или не истечёт общий дедлайн;
* если поток закончился раньше дедлайна — это обрыв: ждёт возвращения устройства
  и открывает новый отрезок с ``-T <последняя увиденная метка>``;
* строки на границе отрезков не дублирует: ``-T`` отдаёт записи начиная с метки
  включительно, поэтому уже увиденные строки с той же меткой пропускаются.

Ограничение (честно): буфер logcat на телефоне — 256 КиБ, его быстро вытесняет камера.
Если связь вернулась слишком поздно, ранние строки могли пропасть. Модуль восстанавливает
всё, что ещё лежит в буфере, но не больше.
"""

from __future__ import annotations

import re
import sys
import time
from collections.abc import Callable, Iterator
from dataclasses import dataclass, field

SegmentOpener = Callable[[str, float], Iterator[str]]
"""Открыть отрезок logcat: (метка ``-T`` «секунды.миллисекунды», дедлайн по ``time.monotonic``) → строки
до конца потока или до дедлайна. Отрезок обязан сам закрыть свой подпроцесс при выходе."""

DeviceWaiter = Callable[[float], bool]
"""Дождаться устройства не дольше заданных секунд: ``True`` — вернулось, ``False`` — нет."""

EPOCH_PREFIX_PATTERN = re.compile(r"^\s*(?P<epoch>[0-9]+\.[0-9]{3})\s")
"""Метка строки формата ``logcat -v epoch``: секунды эпохи и три цифры миллисекунд в начале строки."""

RECONNECT_PAUSE_SECONDS = 1.0
"""Пауза перед повторным открытием отрезка. Без неё устройство в странном состоянии (``adb`` отвечает,
а ``logcat`` сразу выходит) крутило бы цикл подпроцессов до дедлайна без передышки."""


@dataclass
class LogcatCursor:
    """Где мы остановились: самая свежая метка и строки с этой меткой, уже отданные наружу.

    Владелец — один вызов ``resumable_lines``; снаружи состояние не меняется.
    """

    epoch: str
    lines_at_epoch: set[str] = field(default_factory=set)

    def remember(self, line: str) -> None:
        """Сдвинуть курсор по отданной строке.

        Внутри отрезка logcat изредка отдаёт строки не строго по времени: более старая строка курсор
        назад не двигает, но и не теряется — фильтр применяется только на стыке отрезков.
        """
        match = EPOCH_PREFIX_PATTERN.match(line)
        if match is None:
            # Служебные строки logcat без метки («--------- beginning of main») курсор не двигают.
            return
        line_epoch = match.group("epoch")
        if line_epoch == self.epoch:
            self.lines_at_epoch.add(line)
        elif _epoch_key(line_epoch) > _epoch_key(self.epoch):
            self.epoch = line_epoch
            self.lines_at_epoch = {line}

    def boundary_duplicates(self) -> frozenset[str]:
        """Строки, которые новый отрезок с ``-T <epoch>`` отдаст повторно: ``-T`` включает саму метку."""
        return frozenset(self.lines_at_epoch)


def _epoch_key(epoch: str) -> tuple[int, int]:
    """Метку «секунды.миллисекунды» сравниваем как пару целых: строки сравнивались бы неверно (999 > 1000)."""
    seconds, milliseconds = epoch.split(".")
    return int(seconds), int(milliseconds)


def resumable_lines(
    open_segment: SegmentOpener,
    wait_for_device: DeviceWaiter,
    since_epoch: str,
    deadline: float,
    clock: Callable[[], float] = time.monotonic,
    pause: Callable[[float], None] = time.sleep,
    report: Callable[[str], None] | None = None,
) -> Iterator[str]:
    """Строки logcat с ``since_epoch`` до ``deadline``; обрыв потока — ждём устройство и продолжаем.

    Конец итерации означает одно из двух: истёк дедлайн или устройство не вернулось до дедлайна.
    Различать их не нужно: в обоих случаях у ожидания нет строки результата (``EXIT_NO_RESULT``),
    а причина уже напечатана в ``report``.
    """
    say = report if report is not None else _report_to_stderr
    cursor = LogcatCursor(since_epoch)
    while True:
        # Первый отрезок повторов не содержит (пустое множество), следующие — строки на метке стыка.
        already_seen = cursor.boundary_duplicates()
        for line in open_segment(cursor.epoch, deadline):
            if line in already_seen:
                continue
            cursor.remember(line)
            yield line
        remaining = deadline - clock()
        if remaining <= 0:
            return
        say(f"logcat оборвался (обрыв adb?), до дедлайна {remaining:.0f} с; жду устройство")
        if not wait_for_device(remaining):
            say("устройство не вернулось до дедлайна")
            return
        pause(min(RECONNECT_PAUSE_SECONDS, max(deadline - clock(), 0.0)))
        if deadline - clock() <= 0:
            return
        say(f"устройство снова на связи, читаю logcat с метки {cursor.epoch}")


def _report_to_stderr(message: str) -> None:
    """Сообщения о переподключении — в stderr: stdout скрипта занят вердиктом и строками отчёта."""
    print(f"smoke_logcat: {message}", file=sys.stderr, flush=True)
