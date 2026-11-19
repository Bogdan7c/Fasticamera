//! Таблица «событие элемента → хаптика» (`haptics.md` раздел 3). Одна таблица на весь экран:
//! элемент сообщает событие, паттерн берётся отсюда.
//!
//! Правило раздела 3: вибрация — только на дискретные события, которые палец «чувствует»
//! (прилипание, переход опорного значения, срабатывание блокировки). Кнопки и затвор — без вибрации.
//! Все строки таблицы — `[estimate]`: Apple события штатной камеры не описывает.

use serde::{Deserialize, Serialize};
use shell_contract::haptics::HapticPattern;

/// Событие интерфейса, на которое может быть вибрация.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HapticEvent {
    /// Проход обычного тика шкалы (колесо, линейка, шаги ползунка).
    ScaleTick,
    /// Проход опорного значения шкалы (упор колеса, ноль ползунка).
    ScaleReference,
    /// Карусель прилипла к новому элементу.
    CarouselSelection,
    /// Блокировка сработала (долгий тап).
    LockEngaged,
    /// Операция не удалась (например, снимок не сохранён).
    OperationFailed,
    /// Тап по кнопке.
    ButtonTap,
    /// Затвор: вибрация в момент экспозиции смазывает кадр.
    ShutterRelease,
    /// Меню открылось или закрылось.
    MenuToggle,
    /// Последние секунды отсчёта.
    TimerCountdown,
}

impl HapticEvent {
    /// Все события — для тестов полноты таблицы.
    pub const ALL: [HapticEvent; 9] = [
        HapticEvent::ScaleTick,
        HapticEvent::ScaleReference,
        HapticEvent::CarouselSelection,
        HapticEvent::LockEngaged,
        HapticEvent::OperationFailed,
        HapticEvent::ButtonTap,
        HapticEvent::ShutterRelease,
        HapticEvent::MenuToggle,
        HapticEvent::TimerCountdown,
    ];
}

/// Что играть на событие: тишина или паттерн оболочки. Список паттернов не копируется —
/// это сам [`HapticPattern`] из `shell-contract` (правило «без дублирования», решение 84).
/// В конфиге: `"silent"` или `{"play": "selection_tick"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HapticSetting {
    /// Без вибрации.
    Silent,
    /// Сыграть паттерн.
    Play(HapticPattern),
}

impl HapticSetting {
    /// Паттерн оболочки; `None` — вибрации нет.
    pub const fn pattern(self) -> Option<HapticPattern> {
        match self {
            HapticSetting::Silent => None,
            HapticSetting::Play(pattern) => Some(pattern),
        }
    }
}

/// Таблица «событие → хаптика». Поля — по одному на [`HapticEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct HapticTable {
    /// `[estimate]` selection: HIG «series of discrete values».
    pub scale_tick: HapticSetting,
    /// `[estimate]` rigid: отличается от тика (CLICK против TICK).
    pub scale_reference: HapticSetting,
    /// `[estimate]` selection.
    pub carousel_selection: HapticSetting,
    /// `[estimate]` medium: «view snaps into place».
    pub lock_engaged: HapticSetting,
    /// `[estimate]` notification error.
    pub operation_failed: HapticSetting,
    /// `[estimate]` без вибрации: HIG «avoid overusing».
    pub button_tap: HapticSetting,
    /// `[estimate]` без вибрации: HIG «don't disrupt … the camera».
    pub shutter_release: HapticSetting,
    /// `[estimate]` без вибрации.
    pub menu_toggle: HapticSetting,
    /// `[estimate]` без вибрации: отсчёт виден на экране.
    pub timer_countdown: HapticSetting,
}

impl Default for HapticTable {
    fn default() -> Self {
        Self {
            scale_tick: HapticSetting::Play(HapticPattern::SelectionTick),
            scale_reference: HapticSetting::Play(HapticPattern::RigidImpact),
            carousel_selection: HapticSetting::Play(HapticPattern::SelectionTick),
            lock_engaged: HapticSetting::Play(HapticPattern::MediumImpact),
            operation_failed: HapticSetting::Play(HapticPattern::Error),
            button_tap: HapticSetting::Silent,
            shutter_release: HapticSetting::Silent,
            menu_toggle: HapticSetting::Silent,
            timer_countdown: HapticSetting::Silent,
        }
    }
}

impl HapticTable {
    /// Что играть на событие (`None` — тишина).
    pub const fn pattern_for(&self, event: HapticEvent) -> Option<HapticPattern> {
        let setting = match event {
            HapticEvent::ScaleTick => self.scale_tick,
            HapticEvent::ScaleReference => self.scale_reference,
            HapticEvent::CarouselSelection => self.carousel_selection,
            HapticEvent::LockEngaged => self.lock_engaged,
            HapticEvent::OperationFailed => self.operation_failed,
            HapticEvent::ButtonTap => self.button_tap,
            HapticEvent::ShutterRelease => self.shutter_release,
            HapticEvent::MenuToggle => self.menu_toggle,
            HapticEvent::TimerCountdown => self.timer_countdown,
        };
        setting.pattern()
    }
}
