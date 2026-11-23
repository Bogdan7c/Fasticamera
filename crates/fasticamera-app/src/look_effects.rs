//! Исполнение намерений по виду картинки на потоке рендера (решения 99, 178).
//!
//! Экран шлёт [`LookIntent`] эффектом `Effect::Look`. Здесь он превращается в дело:
//! 1. намерение применяет библиотека пресетов ([`LookHandle::apply`]);
//! 2. рендеру уходит то, что отличается от ПОСЛЕДНЕГО ДОСТАВЛЕННОГО: изменились значения ручек →
//!    `ViewfinderCommand::SetLook` с ПОЛНЫМ набором значений (а не «что поменялось»); изменилась модель основы →
//!    `ViewfinderCommand::SetToneBase` (решение 195); не изменилось ничего → ничего. Недоставленная команда не
//!    помечается отправленной, поэтому следующее намерение её повторит: рендер не остаётся на устаревшем виде;
//! 3. изменилось сохраняемое состояние → просьба записать файл (`PresetPersistence`, отдельный поток: рендер
//!    диск не ждёт);
//! 4. ошибка библиотеки (`PresetError`) НЕ глотается: счётчик по виду ошибки + `tracing::warn!`; команда не уходит;
//!    она же и сбои записи файла становятся [`LookProblem`] для экрана ([`LookEffects::latest_problem`]).
//!
//! # Чистый вид (решение 189)
//! [`LookEffects::set_clean_view`] — режим ПОКАЗА, библиотеки он не касается. `ShowBase` посылает рендеру нейтральные
//! ручки (основа остаётся действующей), `ShowEdited` — правленые значения. Пока `ShowBase` зажат: смена основы
//! уходит рендеру сразу, смена ручек откладывается до отпускания (рендер продолжает показывать нейтраль, а на
//! `ShowEdited` получает последние активные значения). Снимок чистый вид не видит: он берёт рецепт библиотеки.
//!
//! Библиотеки может не быть (сборка без хранилища пресетов: ПК-сценарии, тесты экрана): это явное состояние, а не
//! ошибка. Намерение тогда считается ([`LookCounters::without_library`]), рендеру ничего не уходит, а запрос
//! снимка получает нейтральный вид.
//!
//! Итоги записи (`SaveOutcome`) опрашиваются на каждом кадре ([`LookEffects::poll_save_outcomes`]) и считаются;
//! сбой записи громко пишется в лог. Рецепт запроса снимка берётся здесь же ([`LookEffects::current_recipe`]) в момент
//! обработки `Effect::Capture`: исполнитель идёт по эффектам одного кадра по очереди, поэтому «вид при нажатии» —
//! тот, что действовал после всех намерений, пришедших ДО нажатия.

use std::sync::Arc;

use look_contract::{
    CleanView, LookIntent, LookProblem, LookProblemKind, LookRecipe, LookSettings, PresetError, ToneBaseModel,
};
use look_presets::{SaveOutcome, SaveRequestOutcome};
use render_loop::config::ToneBaseChoice;
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
    /// Сброс органа управления, которого нет (полоса вне каталога).
    pub unknown_control: u64,
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
    /// Доставка команд вида (`SetLook`, `SetToneBase`) рендеру.
    pub render: LookRenderCounters,
    /// Сколько раз рендеру ушла смена модели основы (`SetToneBase`, решение 195).
    pub tone_base_changed: u64,
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
    /// Что рендер показывает ПО НАШИМ доставленным командам (стартовое значение — активный рецепт, с которым
    /// собран рендер). С ним сравнивается то, что должно быть показано, чтобы слать только разницу. Значение
    /// «неизвестно» ([`RenderShown::Unknown`]) — после недоставленной команды: пока доставка не удалась, мы НЕ знаем,
    /// что на рендере, и не должны считать, что он «совпадает».
    shown: ShownOnRender,
    /// Режим показа (чистый вид); библиотеки не касается.
    clean_view: CleanView,
    /// Последняя проблема и номер, с которого начнётся следующая.
    latest_problem: Option<LookProblem>,
}

/// Вид, который рендеру уже доставлен: значения ручек и модель основы (каждое знает свою доставку отдельно).
#[derive(Debug, Clone, PartialEq)]
struct ShownOnRender {
    settings: RenderShown<LookSettings>,
    base: RenderShown<ToneBaseModel>,
}

/// Что известно о показанном на рендере значении: либо оно доставлено, либо мы не знаем.
#[derive(Debug, Clone, PartialEq)]
enum RenderShown<Value> {
    /// Доставка не подтверждена (не удалась, либо новый исполнитель ещё не слал): рендер может показывать что угодно.
    Unknown,
    /// Рендер показывает именно это (команда лежит в его ящике).
    Shown(Value),
}

impl<Value: PartialEq> RenderShown<Value> {
    /// Подтверждено ли, что рендер показывает ровно `wanted`. Неизвестное — нет.
    fn is_confirmed_as(&self, wanted: &Value) -> bool {
        matches!(self, Self::Shown(shown) if shown == wanted)
    }

    fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

impl LookEffects {
    /// Исполнитель поверх ручки на библиотеку.
    pub fn new(handle: Option<LookHandle>) -> Self {
        let counters = LookCounters {
            startup: handle.as_ref().map(LookHandle::startup).unwrap_or_default(),
            ..LookCounters::default()
        };
        // Рендер собран с активным рецептом библиотеки (`with_initial_look` + `with_initial_tone_base`): он и есть
        // «уже показанное». Без библиотеки рендер остаётся на нейтрали и основе конфига по умолчанию.
        let shown = handle
            .as_ref()
            .map(LookHandle::active_recipe)
            .unwrap_or_else(unlibraried_recipe);
        Self {
            handle,
            counters,
            shown: ShownOnRender {
                settings: RenderShown::Shown(shown.settings),
                base: RenderShown::Shown(shown.base),
            },
            clean_view: CleanView::default(),
            latest_problem: None,
        }
    }

    /// Последняя проблема вида для экрана (`None` — проблем ещё не было). Каждая новая проблема получает номер на
    /// единицу больше прежней: две одинаковые подряд различимы, экран покажет обе.
    pub fn latest_problem(&self) -> Option<LookProblem> {
        self.latest_problem.clone()
    }

    /// Счётчики на данный момент.
    pub fn counters(&self) -> LookCounters {
        self.counters
    }

    /// Вид, действующий СЕЙЧАС: рецепт (основа и значения ручек) для запроса снимка. Не ссылка на пресет
    /// (инвариант 3). Режим чистого вида на него НЕ влияет: снимок берёт правленый вид библиотеки. Без библиотеки —
    /// нейтраль на основе конфига по умолчанию.
    pub fn current_recipe(&self) -> LookRecipe {
        self.handle
            .as_ref()
            .map_or_else(unlibraried_recipe, LookHandle::active_recipe)
    }

    /// Включает или выключает показ чистого вида (решение 189); рендеру уходит только разница. Библиотеки не
    /// касается. Повтор того же режима команд не шлёт.
    pub fn set_clean_view(&mut self, view: CleanView, render: &dyn RenderControl) {
        self.clean_view = view;
        self.sync_render(render);
    }

    /// Экран поставлен (новый исполнитель): режим показа — `ShowEdited`, а рендеру БЕЗУСЛОВНО уходят действующие
    /// основа и ручки библиотеки. Безусловно, потому что новый исполнитель не знает, что показывал прежний: если
    /// прежний экран снят в тот момент, когда «чистяк» был зажат (нейтраль у рендера), `shown` этого исполнителя
    /// («рендер показывает активный рецепт») был бы ложью, и рендер остался бы нейтральным навсегда. Библиотеки
    /// нет — слать нечего (рендер собран на нейтрали конфига).
    pub fn release_clean_view_on_install(&mut self, render: &dyn RenderControl) {
        self.clean_view = CleanView::ShowEdited;
        if self.handle.is_none() {
            return;
        }
        // Что показывает рендер, мы не знаем (см. доку выше), пока доставка не удастся: при отказе остаётся
        // `Unknown`, и покадровый повтор ([`Self::retry_if_render_unsure`]) дошлёт вид.
        self.shown = ShownOnRender {
            settings: RenderShown::Unknown,
            base: RenderShown::Unknown,
        };
        self.sync_render(render);
    }

    /// Дёшево на каждом кадре: если рендер не подтвердил доставку (новый исполнитель или отказ на отпускании
    /// чистого вида), повторяет синхронизацию; иначе ничего не делает и библиотеку не трогает. Без него рендер,
    /// не принявший команду, оставался бы нейтральным, пока владелец случайно не пошлёт новое намерение.
    pub fn retry_if_render_unsure(&mut self, render: &dyn RenderControl) {
        if self.shown.base.is_unknown() || self.shown.settings.is_unknown() {
            self.sync_render(render);
        }
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
        let outcomes = handle.drain_save_outcomes();
        for outcome in outcomes {
            match outcome {
                SaveOutcome::Saved => self.counters.writes.saved += 1,
                SaveOutcome::SavedDirectoryUnconfirmed(failure) => {
                    self.counters.writes.saved_directory_unconfirmed += 1;
                    tracing::warn!(target: LOG_TARGET, %failure, "файл пресетов заменён, каталог на диск не подтверждён");
                }
                SaveOutcome::Failed(error) => {
                    self.counters.writes.failed += 1;
                    tracing::warn!(target: LOG_TARGET, %error, "файл пресетов не записан");
                    self.report_save_failure(error);
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
        match (look_for_render.is_some(), change.needs_save()) {
            (true, _) => self.counters.look_changed += 1,
            (false, true) => self.counters.stored_state_only += 1,
            (false, false) => self.counters.unchanged += 1,
        }
        // Что слать рендеру, решает сравнение с уже показанным (смена ручек, смена основы или обе), а не тип намерения.
        self.sync_render(render);
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
                    self.record_problem(LookProblemKind::SavesRefused);
                    tracing::warn!(target: LOG_TARGET, "хранилище пресетов отказывается писать: изменения останутся только в памяти");
                }
                SaveRequestOutcome::Closed => {
                    counters.closed += 1;
                    tracing::warn!(target: LOG_TARGET, "поток записи пресетов закрыт: изменения останутся только в памяти");
                }
            }
        }
    }

    /// Приводит показ рендера к тому, что должно быть показано сейчас: основа — активная всегда (решение 189:
    /// смена основы видна даже при чистом виде), ручки — активные при `ShowEdited` и нейтраль при `ShowBase`.
    /// Шлёт только отличающееся от доставленного; без библиотеки показывать нечего.
    fn sync_render(&mut self, render: &dyn RenderControl) {
        let Some(handle) = self.handle.clone() else {
            return;
        };
        let active = handle.active_recipe();
        let wanted_settings = match self.clean_view {
            CleanView::ShowEdited => active.settings,
            CleanView::ShowBase => LookSettings::NEUTRAL,
        };
        if !self.shown.base.is_confirmed_as(&active.base) {
            let command = ViewfinderCommand::SetToneBase(active.base);
            if self.deliver(command, render) {
                self.shown.base = RenderShown::Shown(active.base);
                self.counters.tone_base_changed += 1;
            } else {
                // Не доставили: дальше не знаем, что на рендере (он мог остаться на старом или нет).
                self.shown.base = RenderShown::Unknown;
            }
        }
        if !self.shown.settings.is_confirmed_as(&wanted_settings) {
            let command = ViewfinderCommand::SetLook(Arc::new(wanted_settings.clone()));
            if self.deliver(command, render) {
                self.shown.settings = RenderShown::Shown(wanted_settings);
            } else {
                self.shown.settings = RenderShown::Unknown;
            }
        }
    }

    /// Отдаёт команду рендеру и считает исход; `true` — команда лежит в ящике рендера (иначе её повторит следующая
    /// синхронизация: недоставленное не считается показанным).
    fn deliver(&mut self, command: ViewfinderCommand, render: &dyn RenderControl) -> bool {
        let delivery = render.adjust(RenderAdjustment::Viewfinder(command));
        let counters = &mut self.counters.render;
        match delivery {
            ControlDelivery::Delivered => {
                counters.delivered += 1;
                true
            }
            ControlDelivery::RenderGone => {
                counters.render_gone += 1;
                tracing::warn!(target: LOG_TARGET, "поток рендера завершён: вид картинки не доставлен");
                false
            }
            ControlDelivery::RenderUnavailable { reason } => {
                counters.render_unavailable += 1;
                tracing::warn!(target: LOG_TARGET, %reason, "рендер недоступен: вид картинки не доставлен");
                false
            }
        }
    }

    /// Запоминает проблему с номером на единицу больше прежнего.
    fn record_problem(&mut self, kind: LookProblemKind) {
        let serial = self.latest_problem.as_ref().map_or(1, |problem| problem.serial + 1);
        self.latest_problem = Some(LookProblem { serial, kind });
    }

    /// Сбой записи файла — проблема для экрана. Только вариант «хранилище»: другие ошибки библиотеки при записи
    /// невозможны, но если придут, они всё равно не теряются (`Rejected`).
    fn report_save_failure(&mut self, error: PresetError) {
        let kind = match error {
            PresetError::Storage(failure) => LookProblemKind::SaveFailed(failure),
            other => LookProblemKind::Rejected(other),
        };
        self.record_problem(kind);
    }

    fn count_rejection(&mut self, error: &PresetError) {
        let counters = &mut self.counters.rejected;
        match error {
            PresetError::NameTaken => counters.name_taken += 1,
            PresetError::NotFound(_) => counters.not_found += 1,
            PresetError::BuiltinIsReadOnly(_) => counters.builtin_is_read_only += 1,
            PresetError::UnknownControl(_) => counters.unknown_control += 1,
            PresetError::Storage(_) => counters.storage += 1,
        }
        tracing::warn!(target: LOG_TARGET, %error, "намерение по виду отвергнуто библиотекой пресетов");
        self.record_problem(LookProblemKind::Rejected(error.clone()));
    }
}

/// Рецепт, когда библиотеки в сборке нет: нейтральные ручки на основе конфига по умолчанию (её же берёт рендер).
fn unlibraried_recipe() -> LookRecipe {
    LookRecipe {
        base: ToneBaseChoice::default().into(),
        settings: LookSettings::NEUTRAL,
    }
}

#[cfg(test)]
#[path = "look_effects_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "look_effects_render_sync_tests.rs"]
mod render_sync_tests;
