//! Вид полосок BARS (решение 343): цвета подписей каналов R, G, B.
//!
//! Числа — `[estimate]`: подбор на телефоне. Что полоска делает с картинкой, считает рендер; здесь только то, КАК
//! подписана строка.

use look_contract::PrimaryChannel;
use serde::{Deserialize, Serialize};

/// Цвета подписей каналов, sRGB 8 бит на канал.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ChannelColors {
    /// Красный. `[estimate]`.
    pub red: [u8; 3],
    /// Зелёный. `[estimate]`.
    pub green: [u8; 3],
    /// Синий: светлее чистого (0, 0, 255), чтобы читался на тёмном стекле панели. `[estimate]`.
    pub blue: [u8; 3],
}

impl Default for ChannelColors {
    fn default() -> Self {
        Self {
            red: [255, 90, 80],
            green: [80, 220, 100],
            blue: [90, 150, 255],
        }
    }
}

impl ChannelColors {
    /// Цвет канала.
    pub fn of(&self, channel: PrimaryChannel) -> [u8; 3] {
        match channel {
            PrimaryChannel::Red => self.red,
            PrimaryChannel::Green => self.green,
            PrimaryChannel::Blue => self.blue,
        }
    }
}

/// Вид полосок.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BarsLookConfig {
    /// Цвета подписей каналов.
    pub channel_colors: ChannelColors,
}
