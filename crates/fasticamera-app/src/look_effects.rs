//! Исполнение намерений по виду картинки на потоке рендера (решения 99, 178).
//!
//! Экран шлёт [`LookIntent`] эффектом `Effect::Look`. Здесь он превращается в дело:
//! 1. намерение применяет библиотека пресетов ([`LookHandle::apply`]);
//! 2. изменились итоговые значения ручек → рендеру уходит `ViewfinderCommand::SetLook` с ПОЛНЫМ набором значений
//!    (а не «что поменялось»: потерянная команда не оставляет рендер на устаревшем виде, следующая его чинит);
//! 3. изменилось сохраняемое состояние → просьба записать файл (`PresetPersistence`, отдельный поток: рендер
//!    диск не ждёт);
//! 4. ошибка библиотеки (`PresetError`) НЕ глотается: счётчик по виду ошибки + `tracing::warn!`; команда не уходит.
//!
//! Библиотеки может не быть (сборка без хранилища пресетов: ПК-сценарии, тесты экрана): это явное состояние, а не
//! ошибка. Намерение тогда считается ([`LookCounters::without_library`]), рендеру ничего не уходит, а запрос
//! снимка получает нейтральный вид.
//!
//! Итоги записи (`SaveOutcome`) опрашиваются на каждом кадре ([`LookEffects::poll_save_outcomes`]) и считаются;
//! сбой записи громко пишется в лог. Вид запроса снимка берётся здесь же ([`LookEffects::current_look`]) в момент
//! обработки `Effect::Capture`: исполнитель идёт по эффектам одного кадра по очереди, поэтому «вид при нажатии» —
//! тот, что действовал после всех намерений, пришедших ДО нажатия.

use std::sync::Arc;

use look_contract::{LookIntent, LookSettings, PresetError};
use look_presets::{SaveOutcome, SaveRequestOutcome};
use render_loop::viewfinder::ViewfinderCommand;

use crate::look_service::{LookApplied, LookHandle, LookStartup};
use crate::viewfinder_control::{ControlDelivery, RenderAdjustment, RenderControl};

const LOG_TARGET: &str = "fasticamera::look";

/// Отказы библиотеки по виду ошибки (виды не сливаются: у каждого своя реакция экрана в 14b).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LookRejectionCounters {
    /// Имя пресета уже занято.
    pub name_taken: u64,
    /// Пресета с таким идентификатором нет.
    pub not_found: u64,
    /// Попытка изменить встроенный пресет.
    pub builtin_is_read_only: u64,
    /// Сбой хранилища при применении намерения (например, кончились идентификаторы).
    pub storage: u64,
}

/// Куда делась команда `SetLook` рендеру.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LookRenderCounters {
    /// Команда лежит в ящике потока рендера.
    pub delivered: u64,
    /// Поток рендера уже завершён.
    pub render_gone: u64,
    /// Рендер не поднимался.
    pub render_unavailable: u64,
}

/// Что стало с просьбами записать файл.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LookSaveRequestCounters {
    /// Принята в пустой ящик потока записи.
    pub queued: u64,
    /// Принята вместо прежней, ещё не записанной (запись склеена).
    pub replaced_pending: u64,
    /// Поток записи закрыт или записи нет вообще: просьба отброшена.
    pub closed: u64,
    /// Поток записи умер от паники: просьба отброшена, записей больше не будет.
    pub writer_died: u64,
    /// Хранилище отказывается писать (непрочитанный файл не затираем): просьба отброшена.
    pub refused: u64,
}

/// Исходы записей файла, которые поток записи уже отдал.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LookWriteCounters {
    /// Файл записан.
    pub saved: u64,
    /// Запись не удалась (прежний файл цел).
    pub failed: u64,
    /// Файл заменён, но сброс каталога на диск не подтверждён (данные не потеряны).
    pub saved_directory_unconfirmed: u64,
}

/// Все счётчики вида картинки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LookCounters {
    /// Намерения, после которых итоговые значения ручек изменились (рендеру отправлена команда).
    pub look_changed: u64,
    /// Намерения, изменившие только сохраняемое состояние (имя, список пресетов): рендеру слать нечего.
    pub stored_state_only: u64,
    /// Намерения, ничего не изменившие.
    pub unchanged: u64,
    /// Намерения, пришедшие, когда библиотеки пресетов в сборке нет: отброшены.
    pub without_library: u64,
    /// Отказы библиотеки по видам.
    pub rejected: LookRejectionCounters,
    /// Доставка `SetLook` рендеру.
    pub render: LookRenderCounters,
    /// Просьбы записать файл.
    pub save_requests: LookSaveRequestCounters,
    /// Итоги записей.
    pub writes: LookWriteCounters,
    /// Как прошёл запуск хранилища пресетов (факт, не счётчик).
    pub startup: LookStartup,
}

/// Исполнитель намерений по виду. Живёт внутри исполнителя эффектов экрана; библиотеку берёт по общей ручке.
pub struct LookEffects {
    /// `None` — библиотеки в сборке нет (см. доку модуля).
    handle: Option<LookHandle>,
    counters: LookCounters,
}

impl LookEffects {
    /// Исполнитель поверх ручки на библиотеку.
    pub fn new(handle: Option<LookHandle>) -> Self {
        let counters = LookCounters {
            startup: handle.as_ref().map(LookHandle::startup).unwrap_or_default(),
            ..LookCounters::default()
        };
        Self { handle, counters }
    }

    /// Счётчики на данный момент.
    pub fn counters(&self) -> LookCounters {
        self.counters
    }

    /// Вид, действующий СЕЙЧАС: значения ручек для запроса снимка. Не ссылка на пресет (инвариант 3).
    pub fn current_look(&self) -> LookSettings {
        self.handle
            .as_ref()
            .map_or(LookSettings::NEUTRAL, LookHandle::active_settings)
    }

    /// Применяет намерение экрана и раздаёт последствия: команда рендеру, просьба записи, счётчики.
    pub fn run_intent(&mut self, intent: LookIntent, render: &dyn RenderControl) {
        let Some(handle) = &self.handle else {
            self.counters.without_library += 1;
            tracing::warn!(target: LOG_TARGET, "библиотеки пресетов в сборке нет: намерение по виду отброшено");
            return;
        };
        match handle.apply(intent) {
            Ok(applied) => self.follow_up(applied, render),
            Err(error) => self.count_rejection(&error),
        }
    }

    /// Опрашивает итоги записей файла: считает и громко логирует сбои. Вызывается на каждом кадре.
    pub fn poll_save_outcomes(&mut self) {
        let Some(handle) = &self.handle else {
            return;
        };
        for outcome in handle.drain_save_outcomes() {
            match outcome {
                SaveOutcome::Saved => self.counters.writes.saved += 1,
                SaveOutcome::SavedDirectoryUnconfirmed(failure) => {
                    self.counters.writes.saved_directory_unconfirmed += 1;
                    tracing::warn!(target: LOG_TARGET, %failure, "файл пресетов заменён, каталог на диск не подтверждён");
                }
                SaveOutcome::Failed(error) => {
                    self.counters.writes.failed += 1;
                    tracing::warn!(target: LOG_TARGET, %error, "файл пресетов не записан");
                }
            }
        }
    }

    fn follow_up(&mut self, applied: LookApplied, render: &dyn RenderControl) {
        let LookApplied {
            change,
            look_for_render,
            save_request,
        } = applied;
        match (look_for_render, change.needs_save()) {
            (Some(look), _) => {
                self.counters.look_changed += 1;
                self.send_look_to_render(look, render);
            }
            (None, true) => self.counters.stored_state_only += 1,
            (None, false) => self.counters.unchanged += 1,
        }
        if let Some(request) = save_request {
            let counters = &mut self.counters.save_requests;
            match request {
                SaveRequestOutcome::Queued => counters.queued += 1,
                SaveRequestOutcome::ReplacedPending => counters.replaced_pending += 1,
                SaveRequestOutcome::WriterDied => {
                    counters.writer_died += 1;
                    tracing::error!(target: LOG_TARGET, "поток записи пресетов умер: изменения останутся только в памяти");
                }
                SaveRequestOutcome::Refused => {
                    counters.refused += 1;
                    tracing::warn!(target: LOG_TARGET, "хранилище пресетов отказывается писать: изменения останутся только в памяти");
                }
                SaveRequestOutcome::Closed => {
                    counters.closed += 1;
                    tracing::warn!(target: LOG_TARGET, "поток записи пресетов закрыт: изменения останутся только в памяти");
                }
            }
        }
    }

    fn send_look_to_render(&mut self, look: Arc<LookSettings>, render: &dyn RenderControl) {
        let delivery = render.adjust(RenderAdjustment::Viewfinder(ViewfinderCommand::SetLook(look)));
        let counters = &mut self.counters.render;
        match delivery {
            ControlDelivery::Delivered => counters.delivered += 1,
            ControlDelivery::RenderGone => {
                counters.render_gone += 1;
                tracing::warn!(target: LOG_TARGET, "поток рендера завершён: вид картинки не доставлен");
            }
            ControlDelivery::RenderUnavailable { reason } => {
                counters.render_unavailable += 1;
                tracing::warn!(target: LOG_TARGET, %reason, "рендер недоступен: вид картинки не доставлен");
            }
        }
    }

    fn count_rejection(&mut self, error: &PresetError) {
        let counters = &mut self.counters.rejected;
        match error {
            PresetError::NameTaken => counters.name_taken += 1,
            PresetError::NotFound(_) => counters.not_found += 1,
            PresetError::BuiltinIsReadOnly(_) => counters.builtin_is_read_only += 1,
            PresetError::Storage(_) => counters.storage += 1,
        }
        tracing::warn!(target: LOG_TARGET, %error, "намерение по виду отвергнуто библиотекой пресетов");
    }
}

#[cfg(test)]
#[path = "look_effects_tests.rs"]
mod tests;
