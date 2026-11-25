//! Словарь белого: что ходит между владельцем белого (движок камеры) и рендером.
//!
//! Разделение ответственности (план `user/white-balance`, сессия 01):
//! - **владелец белого** (`camera-core`) — единственный, кто хранит текущую белую и её происхождение и решает,
//!   когда её менять: по команде или по правилу открытия. Он же держит срок ожидания ответа;
//! - **источник белого** (исполняется на потоке рендера, рядом с кадрами) отвечает только на вопрос «какой здесь
//!   свет». Применять ли ответ, решает владелец;
//! - **рендер** применяет к кадрам ровно ту белую, которую назвал владелец.
//!
//! Между ними два ящика:
//! - `DirectiveSlot<WhiteDirective>` (владелец → рендер): какую белую держать, как к ней переехать и какая заявка
//!   источнику сейчас действует;
//! - [`WhiteAnswerChannel`] (рендер → владелец): ответ источника с толчком движку.
//!
//! Белая — **цветность сцены** (температура + Duv), а не нейтраль камеры: одно значение годится всем линзам,
//! в нейтраль своей камеры его переводит DNG-профиль потока.

use std::time::Duration;

use serde::Serialize;

use crate::auto_exposure::{LateBoundRefreshSignal, SignalDelivery};
use crate::auto_exposure_carry::CameraTag;
use crate::capture_result::SensorTimestamp;
use crate::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use crate::three_a::{DirectiveGeneration, DirectiveSlot};

/// Имя источника белого. Не перечисление: новый источник (нейросеть) встаёт своим файлом, словарь не правится.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct WhiteSourceId(&'static str);

impl WhiteSourceId {
    /// Белая штатной автоматики телефона (подсказки цвета в результате кадра).
    pub const PHONE: WhiteSourceId = WhiteSourceId("phone");

    /// Имя нового источника.
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    pub const fn name(self) -> &'static str {
        self.0
    }
}

/// Кто поставил действующую белую.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "source")]
pub enum WhiteOrigin {
    /// Ответ источника (сегодня — «телефон»).
    Source(WhiteSourceId),
    /// Человек: кнопка света или шкала. Сильнее любого источника.
    Manual,
    /// Запасной свет из конфига: белую ещё никто не называл.
    Fallback,
}

/// Действующая белая с происхождением.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct HeldWhite {
    pub setting: WhiteBalanceSetting,
    pub origin: WhiteOrigin,
}

/// Правка одной оси белой рукой. Вторая ось остаётся ровно той, что держит владелец: экран, чьи факты отстали на кадр,
/// не затирает её своим старым числом.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "axis", content = "value")]
pub enum WhiteAdjustment {
    Temperature(ColorTemperature),
    Tint(Tint),
}

impl WhiteAdjustment {
    /// Белая `held` с заменённой осью.
    pub fn applied_to(self, held: WhiteBalanceSetting) -> WhiteBalanceSetting {
        match self {
            WhiteAdjustment::Temperature(temperature) => WhiteBalanceSetting { temperature, ..held },
            WhiteAdjustment::Tint(tint) => WhiteBalanceSetting { tint, ..held },
        }
    }
}

/// Как переехать к новой белой (задел WB17: видео захочет «плавно»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "duration")]
pub enum WhiteTransition {
    /// Со следующего же кадра.
    Immediate,
    /// Плавно за это время (по меткам кадров сенсора).
    Over(Duration),
}

/// Номер заявки источнику. Выдаёт владелец белого; по номеру и только по нему опознаётся ответ: отмена, ручная
/// команда и новый поток делают прежний номер чужим.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct WhiteRequestId(pub u64);

/// Что показывать, пока ответа на заявку нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WhileAwaitingWhite {
    /// Не показывать новые кадры (открытие камеры); предел скрытия — у рендера, по кадрам.
    HideFrames,
    /// Показывать с прежней белой (команда «взять белое» при видимой картинке).
    KeepShowing,
}

/// Действующая заявка источнику.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WhiteSourceRequest {
    pub id: WhiteRequestId,
    pub source: WhiteSourceId,
    pub while_awaiting: WhileAwaitingWhite,
}

/// Указание владельца белого рендеру.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WhiteDirective {
    /// Белая, которую надо держать.
    pub white: WhiteBalanceSetting,
    /// Как переехать к ней, если она отличается от прежней.
    pub transition: WhiteTransition,
    /// Заявка источнику; `None` — спрашивать нечего (и прежняя заявка отменена).
    pub request: Option<WhiteSourceRequest>,
}

/// Свет сцены в ответе источника. Расширяется вариантами (несколько источников света, карта по кадру);
/// вызывающий, не умеющий нового варианта, обязан честно отказаться, поэтому `non_exhaustive`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "white")]
#[non_exhaustive]
pub enum SceneIlluminant {
    /// Одна белая на весь кадр.
    Uniform(WhiteBalanceSetting),
}

/// Уверенность ответа, `0..=1`.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct WhiteConfidence(f32);

impl WhiteConfidence {
    pub const FULL: WhiteConfidence = WhiteConfidence(1.0);

    /// Уверенность из доли; вне `0..=1` прижимается, NaN — полная неуверенность.
    pub fn from_fraction(fraction: f32) -> Self {
        Self(if fraction.is_nan() {
            0.0
        } else {
            fraction.clamp(0.0, 1.0)
        })
    }

    pub fn fraction(self) -> f32 {
        self.0
    }
}

/// Почему источник не знает ответа (он жив, но свет по этим кадрам назвать не может).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WhiteUnknownReason {
    /// Автоматика на этом потоке свет ещё не мерила: отдаёт значение прошлого сеанса (темно, пересвет).
    NoFreshMeasurement,
    /// По кадрам свет не определить (одноцветная сцена и подобное).
    SceneNotMeasurable,
}

/// Почему источника нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WhiteUnavailableReason {
    /// Источник с таким именем не зарегистрирован.
    NotRegistered,
    /// Источник заявил вход, который рендер не готовит.
    InputNotProvided,
    /// Камера не отдаёт подсказок цвета или её автоматика белого мертва (усиления ровно `[1, 1, 1, 1]`).
    AutomationSilent,
    /// У потока нет пригодной цветовой калибровки: подсказку не перевести в свет сцены.
    NoColorProfile,
}

/// Как сломался источник.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WhiteSourceFailure {
    /// Подсказка есть, но в цветность не переводится.
    HintNotConvertible,
    /// Внутренний сбой исполнителя источника.
    Executor,
}

/// Исход заявки источнику. Исходы различимы намеренно: «не знаю» лечится другим кадром, «источника нет» — нет.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum WhiteSourceOutcome {
    Ready {
        illuminant: SceneIlluminant,
        confidence: WhiteConfidence,
    },
    /// Занят прежней заявкой и новую не принял.
    Busy,
    Unknown {
        reason: WhiteUnknownReason,
    },
    Unavailable {
        reason: WhiteUnavailableReason,
    },
    Failed {
        failure: WhiteSourceFailure,
    },
}

/// Ответ источника владельцу белого.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WhiteAnswer {
    /// Чья это заявка.
    pub request: WhiteRequestId,
    /// Камера кадра, по которому дан ответ (факт для диагностики, не опознание).
    pub camera: CameraTag,
    /// Кадр, по которому дан ответ; `None` — ответ не по кадру (отказ до первого кадра).
    pub measured_frame: Option<SensorTimestamp>,
    pub outcome: WhiteSourceOutcome,
}

/// Ящик ответов источника с толчком (рендер → движок). Последний ответ побеждает: действует одна заявка, а ответ
/// на прежнюю владельцу всё равно не нужен. Клоны делят одно состояние.
#[derive(Debug, Clone, Default)]
pub struct WhiteAnswerChannel {
    slot: DirectiveSlot<WhiteAnswer>,
    signal: LateBoundRefreshSignal,
}

impl WhiteAnswerChannel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Положить ответ и толкнуть движок.
    pub fn publish(&self, answer: WhiteAnswer) -> (DirectiveGeneration, SignalDelivery) {
        let generation = self.slot.publish(answer);
        (generation, self.signal.signal())
    }

    /// Ответ новее `seen`.
    pub fn newer_than(&self, seen: Option<DirectiveGeneration>) -> Option<(DirectiveGeneration, WhiteAnswer)> {
        self.slot.newer_than(seen)
    }

    /// Толчок для привязки получателя композицией.
    pub fn refresh_signal(&self) -> &LateBoundRefreshSignal {
        &self.signal
    }
}

#[cfg(test)]
#[path = "white_tests.rs"]
mod tests;
