//! Отпечаток рецепта вида: короткое стабильное число, по которому лог и разбор в `scripts/` узнают «тот же вид».
//!
//! Зачем. Сессия 14o: рецепт вида пишется в release-лог в точках фиксации, а снимок пишет отпечаток рецепта, с
//! которым ушёл. Разбор сверяет отпечатки (снимок = видоискатель по виду) и не таскает в сверку десятки чисел.
//!
//! Алгоритм: FNV-1a, 64 бита, поверх ФИКСИРОВАННОЙ раскладки байтов (ниже). Раскладка — контракт: её менять
//! нельзя без смены версии (`FINGERPRINT_LAYOUT_VERSION`), иначе старые логи перестанут сходиться с новыми.
//! Не криптография и не защита: только «совпало / не совпало».
//!
//! Раскладка (все числа little-endian):
//! 1. версия раскладки, 1 байт;
//! 2. основа: 1 байт (Agx = 1, Aces = 2, Filmic = 3, Legacy = 4; явная таблица, а не номер варианта в коде);
//! 3. восемь глобальных ручек в порядке: контраст, средние, тени, света, насыщенность, сочность, теплота, оттенок;
//!    каждая — `f32::to_bits`, 4 байта;
//! 4. число полос `u32`, затем по каждой полосе в каноническом порядке набора: длина имени `u32`, байты UTF-8
//!    имени, оттенок, насыщенность, яркость (`f32::to_bits`);
//! 5. четыре колеса в порядке Lift, Gamma, Gain, Offset: толчок `a`, толчок `b`, кольцо (`f32::to_bits`).
//! 6. (с версии 2, решения 339–343) двенадцать полосок по каналам: зоны в порядке `WheelZone::ALL` (Lift, Gamma,
//!    Gain, Offset), внутри зоны каналы в порядке `PrimaryChannel::ALL` (R, G, B); каждая — `f32::to_bits`.
//!
//! `f32` берётся по битам: `-0.0` и `0.0` дают разные отпечатки (значения ручек из одной библиотеки нормализованы,
//! поэтому на практике это не мешает, а «тот же рецепт» остаётся строгим).

use crate::{LookRecipe, PrimaryChannel, ToneBaseModel, WheelZone};

/// Версия раскладки байтов; входит в хеш первым байтом.
const FINGERPRINT_LAYOUT_VERSION: u8 = 2;

/// Начальное значение FNV-1a (64 бита).
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
/// Простое число FNV-1a (64 бита).
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Накопитель FNV-1a: принимает байты и числа в фиксированном порядке.
struct FnvHasher(u64);

impl FnvHasher {
    fn new() -> Self {
        Self(FNV_OFFSET_BASIS)
    }

    fn byte(&mut self, value: u8) {
        self.0 ^= u64::from(value);
        self.0 = self.0.wrapping_mul(FNV_PRIME);
    }

    fn bytes(&mut self, values: &[u8]) {
        for &value in values {
            self.byte(value);
        }
    }

    fn count(&mut self, value: u32) {
        self.bytes(&value.to_le_bytes());
    }

    fn float(&mut self, value: f32) {
        self.bytes(&value.to_bits().to_le_bytes());
    }
}

/// Номер основы в раскладке отпечатка (явная таблица: порядок вариантов в коде на неё не влияет).
const fn base_tag(base: ToneBaseModel) -> u8 {
    match base {
        ToneBaseModel::Agx => 1,
        ToneBaseModel::Aces => 2,
        ToneBaseModel::Filmic => 3,
        ToneBaseModel::Legacy => 4,
    }
}

impl LookRecipe {
    /// Стабильный отпечаток рецепта (основа и ВСЕ настройки: ручки, полосы, колёса, полоски каналов). Равные рецепты дают равные
    /// отпечатки; изменение любого поля меняет отпечаток (проверено тестами по каждому полю).
    pub fn fingerprint(&self) -> u64 {
        let mut hasher = FnvHasher::new();
        hasher.byte(FINGERPRINT_LAYOUT_VERSION);
        hasher.byte(base_tag(self.base));
        let settings = &self.settings;
        for knob in [
            settings.contrast,
            settings.midtones,
            settings.shadows,
            settings.highlights,
            settings.saturation,
            settings.vibrance,
            settings.warmth,
            settings.tint,
        ] {
            hasher.float(knob.get());
        }
        let band_count = settings.bands.iter().count();
        // Полос в наборе единицы; переполнение `u32` невозможно, но молча урезать не станем.
        hasher.count(u32::try_from(band_count).unwrap_or(u32::MAX));
        for adjustment in settings.bands.iter() {
            let name = adjustment.band.as_str().as_bytes();
            hasher.count(u32::try_from(name.len()).unwrap_or(u32::MAX));
            hasher.bytes(name);
            hasher.float(adjustment.hue.get());
            hasher.float(adjustment.saturation.get());
            hasher.float(adjustment.luminance.get());
        }
        for zone in WheelZone::ALL {
            let wheel = settings.wheels.wheel(zone);
            hasher.float(wheel.push.a());
            hasher.float(wheel.push.b());
            hasher.float(wheel.ring.get());
        }
        for zone in WheelZone::ALL {
            for channel in PrimaryChannel::ALL {
                hasher.float(settings.primaries.value(zone, channel).get());
            }
        }
        hasher.0
    }

    /// Отпечаток как 16 шестнадцатеричных цифр (формат строки лога `fp=`).
    pub fn fingerprint_hex(&self) -> String {
        format!("{:016x}", self.fingerprint())
    }
}

#[cfg(test)]
#[path = "fingerprint_tests.rs"]
mod tests;
