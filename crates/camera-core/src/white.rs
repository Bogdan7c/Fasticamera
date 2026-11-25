//! Публичная граница владельца белого (план `user/white-balance`, сессия 01).
//!
//! Что здесь: конфиг, то, что движок получает при сборке ([`WhiteWiring`]), и то, что он сообщает наружу
//! ([`WhiteStatus`], итоги заявок). Сама машина владельца — `engine/white`; словарь, который ходит между
//! движком и рендером, — `camera_contract::white`.
//!
//! Инварианты границы:
//! * владелец белого один — движок. Экран и рендер своей белой не хранят;
//! * белая есть всегда: запомненная, а если памяти нет — запасной свет из конфига;
//! * источник белого только отвечает «какой здесь свет»; применить ответ или нет решает владелец;
//! * исходы заявки различимы ([`WhiteRequestOutcome`]): «не знаю», «источника нет», «занят», «сломался»,
//!   «срок вышел» и «отменено» не сливаются в один флаг.

use camera_contract::control::{ColorTemperature, InvalidControlValue, Tint, WhiteBalanceSetting};
use camera_contract::three_a::DirectiveSlot;
use camera_contract::white::{
    WhiteAnswerChannel, WhiteConfidence, WhiteDirective, WhiteRequestId, WhiteSourceFailure, WhiteSourceId,
    WhiteUnavailableReason, WhiteUnknownReason,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::three_a_status::ControlAdjustment;

pub use camera_contract::white::HeldWhite;

pub use crate::white_follow::{
    InvalidWhiteFollowSpeed, WhiteFollowConfig, WhiteFollowRecord, WhiteFollowSpeed, WhiteFollowState,
    WhiteFollowStatus, WhiteKeeping,
};
pub use crate::white_resume::{ResumeRuleChoice, WhiteResumeRule};
pub use crate::white_take::{WhiteTakeReport, WhiteTakeState};

/// Настройки белого (секция `three_a.white` конфига движка). Все числа — данные, обоснование рядом.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WhiteConfig {
    /// Нижний предел белой, К. Ниже — свеча и пламя; шкала экрана и память ручного к нему прижимаются.
    pub min_kelvin: f64,
    /// Верхний предел белой, К. Выше — чистое небо в тени; дальше цветность уходит за область, где
    /// «температура + Duv» осмысленна.
    pub max_kelvin: f64,
    /// Предел оттенка по модулю, Duv (расстояние от локуса Планка). Те же ±0,05, что держал прежний ручной путь.
    pub max_abs_duv: f64,
    /// Запасной свет, К: белая, пока никто её не называл (память пуста или не читается). Дневной свет.
    pub fallback_kelvin: f64,
    /// Оттенок запасного света, Duv. Ноль — на локусе.
    pub fallback_duv: f64,
    /// Сколько ждать ответа источника, мс, **от первого кадра потока** (не от выдачи заявки):
    /// открытие камеры и диалог разрешения могут длиться секундами, и срок не должен сгорать до кадров.
    /// После срока ответ чужой и отбрасывается. Решение владельца WB23: 3 с.
    pub answer_timeout_ms: u64,
    /// За сколько мс плавно принять ответ, пришедший после того, как кадры уже показаны (WB23: 0,3 с).
    /// Ноль — принимать сразу.
    pub late_transition_ms: u64,
    /// Правило возобновления белого по умолчанию (решение владельца WB37: `take_from_source`). Выбор человека
    /// (команда `SetWhiteResumeRule`, запомненный в памяти) сильнее этого значения.
    #[serde(deserialize_with = "deserialize_resume_rule_field")]
    pub resume_rule: WhiteResumeRule,
    /// Слежение за белым источника (WB44–WB56).
    pub follow: WhiteFollowConfig,
}

/// Разбор поля `resume_rule` с именем поля в тексте ошибки. Сам serde на неизвестное значение отвечает только
/// «unknown variant `x`, expected ...» без пути: по такому тексту опечатку в конфиге не найти.
fn deserialize_resume_rule_field<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<WhiteResumeRule, D::Error> {
    WhiteResumeRule::deserialize(deserializer)
        .map_err(|error| serde::de::Error::custom(format!("three_a.white.resume_rule: {error}")))
}

impl Default for WhiteConfig {
    fn default() -> Self {
        Self {
            min_kelvin: 2_000.0,
            max_kelvin: 12_000.0,
            max_abs_duv: 0.05,
            fallback_kelvin: ColorTemperature::DAYLIGHT.kelvin(),
            fallback_duv: 0.0,
            answer_timeout_ms: 3_000,
            late_transition_ms: 300,
            resume_rule: WhiteResumeRule::default(),
            follow: WhiteFollowConfig::default(),
        }
    }
}

/// Пределы белой, как их видит экран: по ним строится шкала и прижим числа (WB31). Берутся из [`WhiteConfig`],
/// а не из умолчаний экрана, поэтому правка конфига двигает шкалу без правки экрана.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WhiteRange {
    /// Самая тёплая белая (нижний предел температуры, `min_kelvin`).
    pub min_temperature: ColorTemperature,
    /// Самая холодная белая (верхний предел температуры, `max_kelvin`).
    pub max_temperature: ColorTemperature,
    /// Предел оттенка по модулю: допустимо `-max_abs_tint..=+max_abs_tint` (`max_abs_duv`).
    pub max_abs_tint: Tint,
}

/// Конфиг белого некорректен: поле названо.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WhiteConfigError {
    #[error("three_a.white.{field}: {problem}")]
    Invalid { field: &'static str, problem: &'static str },
}

impl WhiteConfig {
    /// Предел любого ожидания белой, мс (как у прочих ожиданий 3A).
    pub const MAX_WAIT_MS: u64 = 60_000;
    /// Нижний край таблицы температур (Робертсон, 600 майред): ниже белая в цветность не переводится, и рендер
    /// молча держал бы прежнюю. Верхнего края у таблицы нет (0 майред — бесконечная температура).
    pub const LOWEST_KELVIN: f64 = 1_667.0;
    /// Предел плавного перехода, мс.
    pub const MAX_TRANSITION_MS: u64 = 10_000;

    /// Проверка: пределы упорядочены, запасной свет лежит в них, сроки ограничены.
    pub fn validate(&self) -> Result<(), WhiteConfigError> {
        let invalid = |field, problem| Err(WhiteConfigError::Invalid { field, problem });
        if !(self.min_kelvin.is_finite() && self.min_kelvin >= Self::LOWEST_KELVIN) {
            return invalid(
                "min_kelvin",
                "нужно конечное число не ниже 1667 К (край таблицы температур)",
            );
        }
        if !(self.max_kelvin.is_finite() && self.max_kelvin > self.min_kelvin) {
            return invalid("max_kelvin", "нужно конечное число больше min_kelvin");
        }
        if !(self.max_abs_duv.is_finite() && self.max_abs_duv >= 0.0) {
            return invalid("max_abs_duv", "нужно конечное неотрицательное число");
        }
        if !(self.fallback_kelvin.is_finite() && (self.min_kelvin..=self.max_kelvin).contains(&self.fallback_kelvin)) {
            return invalid(
                "fallback_kelvin",
                "запасной свет должен лежать в пределах min_kelvin..=max_kelvin",
            );
        }
        if !(self.fallback_duv.is_finite() && self.fallback_duv.abs() <= self.max_abs_duv) {
            return invalid("fallback_duv", "запасной оттенок должен лежать в пределах ±max_abs_duv");
        }
        if self.answer_timeout_ms == 0 || self.answer_timeout_ms > Self::MAX_WAIT_MS {
            return invalid("answer_timeout_ms", "нужно от 1 до 60000 мс");
        }
        if self.late_transition_ms > Self::MAX_TRANSITION_MS {
            return invalid("late_transition_ms", "нужно не больше 10000 мс");
        }
        self.follow.validate()
    }

    /// Запасной свет как значение белой. `Err` — только у непроверенного конфига (см. [`Self::validate`]).
    pub fn fallback_setting(&self) -> Result<WhiteBalanceSetting, InvalidControlValue> {
        Ok(WhiteBalanceSetting {
            temperature: ColorTemperature::from_kelvin(self.fallback_kelvin)?,
            tint: Tint::from_duv(self.fallback_duv)?,
        })
    }
}

impl WhiteConfig {
    /// Пределы для статуса. Непроверенный конфиг ([`Self::validate`] ловит его при сборке движка) не доходит
    /// сюда; на всякий случай нечитаемое число заменяется встроенным дневным светом / нулём, а не паникой.
    pub fn range(&self) -> WhiteRange {
        WhiteRange {
            min_temperature: ColorTemperature::from_kelvin(self.min_kelvin).unwrap_or(ColorTemperature::DAYLIGHT),
            max_temperature: ColorTemperature::from_kelvin(self.max_kelvin).unwrap_or(ColorTemperature::DAYLIGHT),
            max_abs_tint: Tint::from_duv(self.max_abs_duv).unwrap_or(Tint::NEUTRAL),
        }
    }

    /// Запасной свет из конфига, а у непроверенного конфига (ошибка журналируется) — встроенный дневной свет.
    ///
    /// Нужен потому, что `ColorTemperature` создаётся только через `Result`, а владельцу белого запасной свет
    /// нужен всегда. Конфиг, прошедший [`Self::validate`], до встроенного значения не доходит.
    pub fn fallback_setting_or_daylight(&self) -> WhiteBalanceSetting {
        match self.fallback_setting() {
            Ok(setting) => setting,
            Err(error) => {
                tracing::error!(target: "fasticamera::camera::white", %error, "запасной свет из конфига неверен, взят встроенный дневной");
                BUILTIN_DAYLIGHT
            }
        }
    }
}

/// Встроенный дневной свет: 5600 К, Duv 0. Последний запас, а не настройка: настройка — `fallback_*` в конфиге.
const BUILTIN_DAYLIGHT: WhiteBalanceSetting = WhiteBalanceSetting {
    temperature: ColorTemperature::DAYLIGHT,
    tint: Tint::NEUTRAL,
};

/// Откуда взято запомненное значение. Имя источника не хранится: после перезапуска запомненное «от источника»
/// считается взятым у источника открытия (он один), а новый источник не обязан читать чужие записи.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RememberedOrigin {
    /// Взято у источника белого.
    Source,
    /// Выставлено рукой.
    Manual,
}

/// Запись памяти: что помнить о белой между запусками.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WhiteMemoryRecord {
    pub setting: WhiteBalanceSetting,
    pub origin: RememberedOrigin,
}

/// Что память отдала при старте.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum WhiteMemoryLoad {
    /// Записи нет (первый запуск или память пуста). Белая — запасной свет.
    #[default]
    NothingRemembered,
    Remembered(WhiteMemoryRecord),
    /// Запись есть, но не читается (битый файл, значение вне домена). Белая — запасной свет, причина видна
    /// в [`WhiteMemoryState`].
    Unreadable {
        reason: String,
    },
}

/// Запись памяти. Метод **не блокирует**: реализация кладёт запись в свой поток записи, а ошибки записи
/// остаются видны у реализации (диагностика памяти), не у движка. Движок зовёт его на каждое изменение
/// белой с происхождением «источник» или «ручное»; запасной свет не сохраняется.
///
/// Реализация вправе откладывать запись на диск (правка шкалы под пальцем идёт десятки раз в секунду), но обязана
/// уложить последнее значение за свой ограниченный срок; [`WhiteMemory::flush`] требует положить его сразу.
pub trait WhiteMemory: Send {
    fn save(&mut self, record: WhiteMemoryRecord);

    /// Запомнить выбор правила возобновления, сделанный человеком. Те же обещания, что у [`Self::save`]: не
    /// блокирует, запись вправе отложиться, [`Self::flush`] кладёт её сразу. Белую и правило память хранит
    /// независимо: запись одного не стирает другое.
    fn save_resume_rule(&mut self, rule: WhiteResumeRule);

    /// Запомнить выбор человека о слежении (режим и скорость одной записью). Те же обещания, что у [`Self::save`];
    /// белую и правило возобновления запись не трогает.
    fn save_follow(&mut self, record: WhiteFollowRecord);

    /// «Пиши немедленно»: всё принятое, но ещё не записанное, должно лечь на диск без ожидания очередного
    /// интервала. Метод **не ждёт диска** на потоке движка: он только снимает отсрочку у потока записи. Без
    /// принятого и несброшенного — ничего не делает. Движок зовёт его, когда приложение уходит в фон: процесс
    /// в фоне могут убить без предупреждения.
    fn flush(&mut self);
}

/// Память, которой нет (тесты, стенды, сборки без файла).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoWhiteMemory;

impl WhiteMemory for NoWhiteMemory {
    fn save(&mut self, _record: WhiteMemoryRecord) {}

    fn save_resume_rule(&mut self, _rule: WhiteResumeRule) {}

    fn save_follow(&mut self, _record: WhiteFollowRecord) {}

    fn flush(&mut self) {}
}

/// Всё, что владельцу белого дают при сборке движка.
pub struct WhiteWiring {
    /// Сюда движок кладёт указание рендеру (читает поток рендера). Публикуется сразу при создании движка.
    pub directives: DirectiveSlot<WhiteDirective>,
    /// Отсюда движок читает ответы источника по толчку (тот же толчок, что у AE).
    pub answers: WhiteAnswerChannel,
    /// Что помнила память к старту (читает корень композиции до создания движка).
    pub remembered: WhiteMemoryLoad,
    /// Правило возобновления, которое человек выбрал раньше и память сохранила (`None` — выбора в памяти нет,
    /// действует конфиг). Хранится отдельно от [`Self::remembered`]: выбор правила возможен до первой белой.
    pub remembered_resume_rule: Option<WhiteResumeRule>,
    /// Слежение, которое человек выбрал раньше (умолчание — выключено, скорость из конфига).
    pub remembered_follow: WhiteFollowRecord,
    /// Запись памяти.
    pub memory: Box<dyn WhiteMemory>,
    /// Источник, которого спрашивают при каждом возобновлении приложения (WB22). Сегодня — «телефон».
    pub opening_source: WhiteSourceId,
}

impl Default for WhiteWiring {
    /// Для тестов и стендов: свои ящики, ничего не помнится, память пуста, источник открытия — «телефон».
    fn default() -> Self {
        Self {
            directives: DirectiveSlot::new(),
            answers: WhiteAnswerChannel::new(),
            remembered: WhiteMemoryLoad::NothingRemembered,
            remembered_resume_rule: None,
            remembered_follow: WhiteFollowRecord::default(),
            memory: Box::new(NoWhiteMemory),
            opening_source: WhiteSourceId::PHONE,
        }
    }
}

impl std::fmt::Debug for WhiteWiring {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WhiteWiring")
            .field("remembered", &self.remembered)
            .field("remembered_resume_rule", &self.remembered_resume_rule)
            .field("remembered_follow", &self.remembered_follow)
            .field("opening_source", &self.opening_source)
            .finish_non_exhaustive()
    }
}

/// Кто отменил заявку.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WhiteCancelCause {
    /// Белую выставили рукой: рука сильнее.
    ManualWhite,
    /// Приложение ушло в фон.
    Background,
    /// Блокировка AE/AF при слежении: белая заморожена, заявка открытия снята (WB46, WB54).
    AeAfLock,
    /// Пришла новая заявка (команда «взять белое» или возобновление).
    ReplacedByNewRequest,
}

/// Чем кончилась заявка источнику. Белая меняется только при [`WhiteRequestOutcome::Applied`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum WhiteRequestOutcome {
    /// Ответ принят: белая взята у источника.
    Applied {
        confidence: WhiteConfidence,
    },
    /// Источник занят прежней заявкой.
    Busy,
    /// Источник жив, но свет назвать не может.
    Unknown {
        reason: WhiteUnknownReason,
    },
    /// Источника нет или он не может работать на этом телефоне.
    Unavailable {
        reason: WhiteUnavailableReason,
    },
    /// Источник сломался.
    Failed {
        failure: WhiteSourceFailure,
    },
    /// Ответ получен, но владелец его не понимает (вариант света новее владельца).
    NotUnderstood,
    /// Срок ожидания вышел.
    TimedOut,
    Cancelled {
        by: WhiteCancelCause,
    },
}

/// Итог заявки с её номером и источником.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WhiteRequestResult {
    pub request: WhiteRequestId,
    pub source: WhiteSourceId,
    pub result: WhiteRequestOutcome,
}

/// Заявка источнику сейчас.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum WhiteRequestState {
    /// Спрашивать нечего; `last` — чем кончилась прошлая заявка (`None` — заявок ещё не было).
    Idle { last: Option<WhiteRequestResult> },
    /// Ждём ответа на заявку.
    Waiting {
        request: WhiteRequestId,
        source: WhiteSourceId,
    },
}

/// Как память отработала при старте.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum WhiteMemoryStartup {
    /// Ничего не помнилось: белая — запасной свет.
    NothingRemembered,
    /// Белая взята из памяти (возможно, прижата к пределам: см. `WhiteStatus::limits`).
    Restored,
    /// Запись не прочиталась; белая — запасной свет. Причина — как её назвала память.
    Unreadable { reason: String },
}

/// Состояние памяти белой.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WhiteMemoryState {
    pub startup: WhiteMemoryStartup,
    /// Сколько записей движок передал памяти за свою жизнь: белой, правила и слежения вместе (успех записи видит
    /// реализация). Все они лежат в одном файле.
    pub saves_requested: u64,
}

/// Снимок белого для экрана и диагностики.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WhiteStatus {
    /// Действующая белая и кто её поставил.
    pub held: HeldWhite,
    /// Поправки к последнему названному значению (прижим к пределам). Пусто — применено как есть.
    pub limits: Vec<ControlAdjustment>,
    /// Пределы белой из конфига (для шкалы экрана); известны с первого снимка.
    pub range: WhiteRange,
    pub request: WhiteRequestState,
    /// Исход последней команды «взять белую» (`None` — таких команд ещё не было). Заявка открытия приложения
    /// сюда не попадает: она видна в [`Self::request`].
    pub last_take: Option<WhiteTakeReport>,
    pub memory: WhiteMemoryState,
    /// Действующее правило возобновления: выбор человека, а если его нет — умолчание конфига. Для экрана настроек.
    pub resume_rule: WhiteResumeRule,
    /// Слежение за источником. Его собственные вопросы источнику видны только здесь: в [`Self::request`] и
    /// [`Self::last_take`] они не попадают.
    pub follow: WhiteFollowStatus,
    /// Сколько ответов отброшено как чужих (номер не действующей заявки): гонки видны в диагностике.
    pub stale_answers: u64,
}

impl WhiteStatus {
    /// Снимок до первого состояния движка: запасной свет по умолчанию, заявок нет.
    pub fn initial() -> Self {
        Self {
            held: HeldWhite {
                setting: WhiteConfig::default().fallback_setting_or_daylight(),
                origin: camera_contract::white::WhiteOrigin::Fallback,
            },
            limits: Vec::new(),
            range: WhiteConfig::default().range(),
            request: WhiteRequestState::Idle { last: None },
            last_take: None,
            memory: WhiteMemoryState {
                startup: WhiteMemoryStartup::NothingRemembered,
                saves_requested: 0,
            },
            resume_rule: WhiteConfig::default().resume_rule,
            follow: WhiteFollowStatus::off(WhiteConfig::default().follow.default_speed()),
            stale_answers: 0,
        }
    }
}

#[cfg(test)]
#[path = "white_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "white_runtime_tests.rs"]
mod runtime_tests;
