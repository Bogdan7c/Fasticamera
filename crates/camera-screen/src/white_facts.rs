//! Факты о белом: что экран знает о белом балансе от движка (white-balance 02, решения WB21, WB31, WB33).
//!
//! Экран белое НЕ хранит: всё, что здесь лежит, приложение переводит из статуса движка и доски показаний. Тип
//! нейтральный (как остальные факты): `camera-core` экрану неизвестен.

use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::white::{HeldWhite, WhiteOrigin};
use camera_contract::white_reading::WhiteReading;

/// Пределы белого, которые принимает движок (экран только показывает по ним шкалы и проверяет кнопки света).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WhiteRange {
    /// Нижний предел цветовой температуры, К.
    pub min_kelvin: f64,
    /// Верхний предел цветовой температуры, К.
    pub max_kelvin: f64,
    /// Предел оттенка по модулю, Duv (симметричный: от `-max_abs_duv` до `+max_abs_duv`).
    pub max_abs_duv: f64,
}

impl WhiteRange {
    /// Пределы до первой публикации движка: те же, что у движка по умолчанию (2000–12000 К, ±0,05 Duv).
    pub const STARTING: WhiteRange = WhiteRange {
        min_kelvin: 2_000.0,
        max_kelvin: 12_000.0,
        max_abs_duv: 0.05,
    };

    /// Лежит ли белое внутри пределов (включая границы).
    pub fn contains(&self, white: WhiteBalanceSetting) -> bool {
        let kelvin = white.temperature.kelvin();
        (self.min_kelvin..=self.max_kelvin).contains(&kelvin) && white.tint.duv().abs() <= self.max_abs_duv
    }
}

/// Состояние СВОЕЙ команды «взять белое» (по отчёту движка о последней команде).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhiteTakeState {
    /// Источник спрошен, ответа ещё нет.
    Waiting,
    /// Источник ответил, белое взято.
    Taken,
    /// Белое не взято: источник «не знает», его нет или сломался, либо движок не стал спрашивать.
    Refused,
    /// Заявку отменила другая команда (ручное сильнее) или смена потока.
    Cancelled,
}

/// Исход последней команды «взять» с номером команды движка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WhiteTakeFacts {
    /// Номер команды движка: по нему экран отличает новый исход от уже виденного.
    pub serial: u64,
    pub state: WhiteTakeState,
}

/// Слежение за белым телефона (WB44) глазами экрана. Включено ли слежение — факт движка: экран режим не
/// хранит, а рисует то, что сообщил движок.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WhiteFollowFacts {
    /// Слежения нет: белое держится само.
    #[default]
    Off,
    /// Слежение включено. Сюда же входит «включено, но заморожено блокировкой AE/AF» (WB46): для человека режим
    /// тот же, «Auto» остаётся горящей.
    On,
}

impl WhiteFollowFacts {
    /// Включено ли слежение.
    pub const fn is_on(self) -> bool {
        matches!(self, Self::On)
    }
}

/// Факты о белом (поле [`crate::facts::CameraFacts::white`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WhiteFacts {
    /// Действующее белое с происхождением (число кнопки сверху, положение шкал, подсветка).
    pub held: HeldWhite,
    /// Пределы белого.
    pub range: WhiteRange,
    /// Исход последней своей команды «взять»; `None` — команд не было.
    pub take: Option<WhiteTakeFacts>,
    /// Показание источника «телефон» для кнопки («~ 3100»); `None` — не показано.
    pub phone_reading: Option<WhiteReading>,
    /// Слежение за белым телефона (кнопка «Auto», число у затвора).
    pub follow: WhiteFollowFacts,
}

impl WhiteFacts {
    /// Факты до первого сообщения движка: дневной свет 5600 К, запасное происхождение, пределы по умолчанию.
    pub const STARTING: WhiteFacts = WhiteFacts {
        held: HeldWhite {
            setting: WhiteBalanceSetting {
                temperature: ColorTemperature::DAYLIGHT,
                tint: Tint::NEUTRAL,
            },
            origin: WhiteOrigin::Fallback,
        },
        range: WhiteRange::STARTING,
        take: None,
        phone_reading: None,
        follow: WhiteFollowFacts::Off,
    };
}

impl Default for WhiteFacts {
    fn default() -> Self {
        Self::STARTING
    }
}
