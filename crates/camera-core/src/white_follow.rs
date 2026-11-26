//! Слежение за белым источника (план `user/white-balance`, сессия 04, WB44–WB56): публичные типы.
//!
//! «Взять разово и держать» остаётся поведением по умолчанию; слежение — второе поведение рядом. Включает его
//! человек (команда `FollowWhite`, кнопка «Auto»), выключает любое его действие с белой: взять разово, кнопка
//! света, шкала. Выбор и скорость запоминаются. Сама машина — `engine/white/follow.rs`.

use std::time::Duration;

use camera_contract::white::{WhiteSourceId, WhiteTransition};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::white::{WhiteConfigError, WhiteRequestOutcome};

/// Что белая делает сама, без действий человека.
///
/// В JSON — простая строка, как у `WhiteResumeRule`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhiteKeeping {
    /// Стоит неподвижно, пока её не сменит человек (или правило возобновления).
    #[default]
    Hold,
    /// Следит за источником.
    FollowSource,
}

/// За сколько белая плавно доезжает до нового света при слежении. Ноль — сразу.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct WhiteFollowSpeed {
    millis: u64,
}

/// Время переезда вне допустимого.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("скорость слежения {requested_ms} мс: нужно не больше {max_ms} мс", max_ms = WhiteFollowSpeed::MAX_MILLIS)]
pub struct InvalidWhiteFollowSpeed {
    pub requested_ms: u64,
}

impl WhiteFollowSpeed {
    /// Предел, мс: тот же, что у прочих плавных переходов белой.
    pub const MAX_MILLIS: u64 = 10_000;
    /// Умолчание (решение владельца WB45): быстро, 0,3 с.
    pub const DEFAULT: WhiteFollowSpeed = WhiteFollowSpeed { millis: 300 };

    pub fn from_millis(millis: u64) -> Result<Self, InvalidWhiteFollowSpeed> {
        if millis > Self::MAX_MILLIS {
            return Err(InvalidWhiteFollowSpeed { requested_ms: millis });
        }
        Ok(Self { millis })
    }

    pub const fn millis(self) -> u64 {
        self.millis
    }

    /// Переход для рендера.
    pub const fn transition(self) -> WhiteTransition {
        match self.millis {
            0 => WhiteTransition::Immediate,
            millis => WhiteTransition::Over(Duration::from_millis(millis)),
        }
    }
}

impl TryFrom<u64> for WhiteFollowSpeed {
    type Error = InvalidWhiteFollowSpeed;

    fn try_from(millis: u64) -> Result<Self, Self::Error> {
        Self::from_millis(millis)
    }
}

impl From<WhiteFollowSpeed> for u64 {
    fn from(speed: WhiteFollowSpeed) -> Self {
        speed.millis
    }
}

/// Настройки слежения (секция `three_a.white.follow` конфига движка).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WhiteFollowConfig {
    /// Как часто спрашивать источник, мс по меткам кадров сенсора. Отсчёт по кадрам, а не по часам: нет кадров —
    /// спрашивать незачем, и поток движка ради слежения лишний раз не просыпается.
    pub ask_interval_ms: u64,
    /// Скорость, пока человек не выбрал свою (WB45: 300).
    pub transition_ms: u64,
    /// Сколько после снимка со вспышкой не спрашивать, мс по меткам кадров. Кадры со светодиодом источник не видит, но
    /// автоматика телефона мерит и их, и после вспышки ещё возвращается к свету сцены: `[measured: OnePlus 11]` на
    /// двух снимках 15 майред сразу после вспышки и в пределах мёртвой зоны через 0,3–0,5 с.
    pub settle_after_flash_ms: u64,
    /// Мёртвая зона по температуре, майреды: меньший сдвиг белую не двигает (WB53).
    pub dead_zone_mired: f64,
    /// Мёртвая зона по оттенку, Duv (WB53).
    pub dead_zone_duv: f64,
}

impl Default for WhiteFollowConfig {
    /// `ask_interval_ms` `[estimate]`: не реже самой быстрой полезной скорости (0,1 с, WB56), иначе переезд шёл бы
    /// ступеньками «доехал — постоял». Мёртвая зона — слово владельца, дрожь оценки проверяется на телефоне.
    fn default() -> Self {
        Self {
            ask_interval_ms: 100,
            transition_ms: WhiteFollowSpeed::DEFAULT.millis(),
            settle_after_flash_ms: 1_000,
            dead_zone_mired: 5.0,
            dead_zone_duv: 0.002,
        }
    }
}

impl WhiteFollowConfig {
    /// Предел периода опроса и паузы после вспышки, мс.
    const MAX_WAIT_MS: u64 = 60_000;

    /// Проверка чисел; имя поля — с приставкой секции.
    pub fn validate(&self) -> Result<(), WhiteConfigError> {
        let invalid = |field, problem| Err(WhiteConfigError::Invalid { field, problem });
        if self.ask_interval_ms == 0 || self.ask_interval_ms > Self::MAX_WAIT_MS {
            return invalid("follow.ask_interval_ms", "нужно от 1 до 60000 мс");
        }
        if self.transition_ms > WhiteFollowSpeed::MAX_MILLIS {
            return invalid("follow.transition_ms", "нужно не больше 10000 мс");
        }
        if self.settle_after_flash_ms > Self::MAX_WAIT_MS {
            return invalid("follow.settle_after_flash_ms", "нужно не больше 60000 мс");
        }
        if !(self.dead_zone_mired.is_finite() && self.dead_zone_mired >= 0.0) {
            return invalid("follow.dead_zone_mired", "нужно конечное неотрицательное число");
        }
        if !(self.dead_zone_duv.is_finite() && self.dead_zone_duv >= 0.0) {
            return invalid("follow.dead_zone_duv", "нужно конечное неотрицательное число");
        }
        Ok(())
    }

    /// Скорость по умолчанию. Непроверенный конфиг ([`Self::validate`] ловит его при сборке движка) получает
    /// встроенное умолчание, а не панику.
    pub fn default_speed(&self) -> WhiteFollowSpeed {
        WhiteFollowSpeed::from_millis(self.transition_ms).unwrap_or(WhiteFollowSpeed::DEFAULT)
    }
}

/// Запись памяти о слежении: что выбрал человек.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct WhiteFollowRecord {
    pub keeping: WhiteKeeping,
    /// Скорость, выбранная человеком; `None` — не выбирал, действует конфиг.
    pub speed: Option<WhiteFollowSpeed>,
}

/// Что слежение делает сейчас.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum WhiteFollowState {
    Off,
    /// Белая идёт за источником.
    Following {
        source: WhiteSourceId,
    },
    /// Слежение включено, но белая заморожена блокировкой AE/AF (WB46).
    HeldByLock {
        source: WhiteSourceId,
    },
    /// Слежение включено, но поставлено на паузу командой `SetWhiteFollowPause` (14n, WB57): пока открыта секция
    /// CALIBRATE, белая стоит на месте. Если одновременно висит блокировка AE/AF, показывается именно `Paused`:
    /// пауза — явное действие человека над слежением, и снимает её только он (блокировка снимается отдельно, и
    /// эти два замка независимы, см. `WhiteFollowPause`).
    Paused {
        source: WhiteSourceId,
    },
}

impl WhiteFollowState {
    /// Включено ли слежение (в том числе замороженное блокировкой).
    pub const fn is_on(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// Пауза слежения, которую ставит экран на время, пока открыта секция CALIBRATE (14n, решение 289).
///
/// Это замок, независимый от блокировки AE/AF: оба могут висеть одновременно, и каждый снимается только тем, кто
/// его поставил. Пауза не выключает режим «Auto» и не трогает память белой: режим остаётся включённым и
/// запомненным, слежение лишь молчит. Уже начатый переезд (до 0,3 с) рендер доводит до конца (как при блокировке).
/// Не сохраняется между запусками: процесс всегда стартует в режиме фото, где секции CALIBRATE нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WhiteFollowPause {
    /// Слежение идёт как обычно (если включён «Auto»).
    #[default]
    Running,
    /// Слежение молчит: источник не спрашивают, ждущий вопрос снят.
    Paused,
}

/// Слежение для экрана и диагностики.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WhiteFollowStatus {
    pub state: WhiteFollowState,
    /// Действующая скорость: выбор человека, а если его нет — конфиг.
    pub speed: WhiteFollowSpeed,
    /// Сколько раз источник спрошен ради слежения.
    pub asked: u64,
    /// Сколько ответов сдвинули белую (остальные — внутри мёртвой зоны или отказы).
    pub moved: u64,
    /// Чем кончился последний вопрос, если ответа не вышло: «не знаю», «источника нет», срок. `None` — ответ был.
    pub last_refusal: Option<WhiteRequestOutcome>,
}

impl WhiteFollowStatus {
    /// Слежение выключено, спросить ещё не успели.
    pub const fn off(speed: WhiteFollowSpeed) -> Self {
        Self {
            state: WhiteFollowState::Off,
            speed,
            asked: 0,
            moved: 0,
            last_refusal: None,
        }
    }
}

#[cfg(test)]
#[path = "white_follow_tests.rs"]
mod tests;
