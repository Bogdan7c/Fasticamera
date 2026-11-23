//! Ручки по полосам оттенка (решение 99): «небо», «зелень», «кожа» и т.д.
//!
//! Словарь не знает, какие полосы существуют и где их центры: это модель вида в `isp-model`.
//! Здесь только проверенный идентификатор и значения трёх ручек полосы.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::knob::LookKnob;

/// Максимальная длина идентификатора полосы в символах.
const BAND_ID_MAX_LEN: usize = 32;

/// Ошибка идентификатора полосы.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HueBandIdError {
    /// Пустая строка.
    #[error("идентификатор полосы пуст")]
    Empty,
    /// Длиннее допустимого.
    #[error("идентификатор полосы длиннее {max} символов: {actual}")]
    TooLong {
        /// Допустимый максимум.
        max: usize,
        /// Фактическая длина.
        actual: usize,
    },
    /// Символ вне набора `[a-z0-9_-]`.
    #[error("идентификатор полосы содержит недопустимый символ {found:?}")]
    InvalidCharacter {
        /// Первый недопустимый символ.
        found: char,
    },
}

/// Проверенное имя полосы оттенка: ascii `[a-z0-9_-]`, 1..=32 символа.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct HueBandId(String);

impl HueBandId {
    /// Проверяет и строит идентификатор.
    pub fn new(raw: impl Into<String>) -> Result<Self, HueBandIdError> {
        let raw = raw.into();
        if raw.is_empty() {
            return Err(HueBandIdError::Empty);
        }
        // Набор ascii-символов: длина в байтах == длина в символах, пока нет чужих символов;
        // чужие символы отсекаются ниже, поэтому считаем именно символы.
        let actual = raw.chars().count();
        if actual > BAND_ID_MAX_LEN {
            return Err(HueBandIdError::TooLong {
                max: BAND_ID_MAX_LEN,
                actual,
            });
        }
        if let Some(found) = raw
            .chars()
            .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_' || *c == '-'))
        {
            return Err(HueBandIdError::InvalidCharacter { found });
        }
        Ok(Self(raw))
    }

    /// Идентификатор как строка.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for HueBandId {
    type Error = HueBandIdError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::new(raw)
    }
}

impl From<HueBandId> for String {
    fn from(id: HueBandId) -> Self {
        id.0
    }
}

/// Три ручки одной полосы оттенка. Нейтраль — все три в нуле.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandAdjustment {
    /// Какая полоса.
    pub band: HueBandId,
    /// Сдвиг оттенка (осознанное исключение из решения 47).
    pub hue: LookKnob,
    /// Насыщенность полосы.
    pub saturation: LookKnob,
    /// Яркость полосы.
    pub luminance: LookKnob,
}

impl BandAdjustment {
    /// Нейтральная запись для полосы: ничего не меняет.
    pub fn neutral(band: HueBandId) -> Self {
        Self {
            band,
            hue: LookKnob::NEUTRAL,
            saturation: LookKnob::NEUTRAL,
            luminance: LookKnob::NEUTRAL,
        }
    }

    /// Все три ручки в нуле.
    pub fn is_neutral(&self) -> bool {
        self.hue.is_neutral() && self.saturation.is_neutral() && self.luminance.is_neutral()
    }
}

/// Ошибка набора полос.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BandAdjustmentsError {
    /// Одна полоса указана дважды: непонятно, какая запись главная.
    #[error("повтор идентификатора полосы {band:?}")]
    DuplicateBand {
        /// Повторившийся идентификатор.
        band: String,
    },
}

/// Набор правок по полосам с уникальными идентификаторами.
///
/// Каноническая форма (чтобы `==` значило «вид одинаковый»): записи отсортированы по
/// идентификатору, НЕЙТРАЛЬНЫЕ записи отбрасываются. На входе нейтральные записи допустимы
/// (экран может прислать «полоса тронута и возвращена в ноль»), но после нормализации
/// «нет записи» и «запись из нулей» — одно и то же состояние.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(try_from = "Vec<BandAdjustment>", into = "Vec<BandAdjustment>")]
pub struct BandAdjustments(Vec<BandAdjustment>);

impl BandAdjustments {
    /// Нет правок ни по одной полосе.
    pub const NONE: BandAdjustments = BandAdjustments(Vec::new());

    /// Проверяет уникальность идентификаторов и приводит набор к канонической форме.
    pub fn new(adjustments: Vec<BandAdjustment>) -> Result<Self, BandAdjustmentsError> {
        // Повтор ищем ДО отбрасывания нейтральных: дубль — ошибка вызывающего в любом случае.
        let mut seen = BTreeSet::new();
        for adjustment in &adjustments {
            if !seen.insert(adjustment.band.clone()) {
                return Err(BandAdjustmentsError::DuplicateBand {
                    band: adjustment.band.as_str().to_owned(),
                });
            }
        }
        let mut canonical: Vec<BandAdjustment> = adjustments
            .into_iter()
            .filter(|adjustment| !adjustment.is_neutral())
            .collect();
        canonical.sort_by(|left, right| left.band.cmp(&right.band));
        Ok(Self(canonical))
    }

    /// Правка полосы; нейтральная, если записи нет.
    pub fn for_band(&self, band: &HueBandId) -> BandAdjustment {
        self.0
            .iter()
            .find(|adjustment| &adjustment.band == band)
            .cloned()
            .unwrap_or_else(|| BandAdjustment::neutral(band.clone()))
    }

    /// Возвращает копию набора, где правка этой полосы заменена (или добавлена, или убрана,
    /// если новая правка нейтральна).
    pub fn with_adjustment(&self, adjustment: BandAdjustment) -> Self {
        let mut rest: Vec<BandAdjustment> = self
            .0
            .iter()
            .filter(|existing| existing.band != adjustment.band)
            .cloned()
            .collect();
        rest.push(adjustment);
        // Уникальность гарантирована: прежнюю запись этой полосы мы только что убрали.
        let mut canonical: Vec<BandAdjustment> = rest.into_iter().filter(|a| !a.is_neutral()).collect();
        canonical.sort_by(|left, right| left.band.cmp(&right.band));
        Self(canonical)
    }

    /// Ни одна полоса не тронута.
    pub fn is_neutral(&self) -> bool {
        self.0.is_empty()
    }

    /// Ненейтральные правки в порядке идентификаторов.
    pub fn iter(&self) -> impl Iterator<Item = &BandAdjustment> {
        self.0.iter()
    }
}

impl TryFrom<Vec<BandAdjustment>> for BandAdjustments {
    type Error = BandAdjustmentsError;

    fn try_from(adjustments: Vec<BandAdjustment>) -> Result<Self, Self::Error> {
        Self::new(adjustments)
    }
}

impl From<BandAdjustments> for Vec<BandAdjustment> {
    fn from(adjustments: BandAdjustments) -> Self {
        adjustments.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(raw: &str) -> HueBandId {
        HueBandId::new(raw).unwrap()
    }

    fn sky(hue: f32) -> BandAdjustment {
        BandAdjustment {
            hue: LookKnob::new(hue).unwrap(),
            ..BandAdjustment::neutral(id("sky"))
        }
    }

    #[test]
    fn band_id_accepts_allowed_alphabet_and_length_edges() {
        assert!(HueBandId::new("a").is_ok());
        assert!(HueBandId::new("skin-tone_2").is_ok());
        assert!(HueBandId::new("a".repeat(32)).is_ok());
    }

    #[test]
    fn band_id_rejects_each_violation_distinctly() {
        assert_eq!(HueBandId::new(""), Err(HueBandIdError::Empty));
        assert_eq!(
            HueBandId::new("a".repeat(33)),
            Err(HueBandIdError::TooLong { max: 32, actual: 33 })
        );
        assert_eq!(
            HueBandId::new("Sky"),
            Err(HueBandIdError::InvalidCharacter { found: 'S' })
        );
        assert_eq!(
            HueBandId::new("не-ascii"),
            Err(HueBandIdError::InvalidCharacter { found: 'н' })
        );
    }

    #[test]
    fn band_id_serde_rejects_invalid() {
        assert!(serde_json::from_str::<HueBandId>("\"sky\"").is_ok());
        assert!(serde_json::from_str::<HueBandId>("\"Sky!\"").is_err());
        assert!(serde_json::from_str::<HueBandId>("\"\"").is_err());
    }

    #[test]
    fn duplicate_ids_are_rejected_even_if_neutral() {
        let result = BandAdjustments::new(vec![sky(0.5), BandAdjustment::neutral(id("sky"))]);
        assert_eq!(result, Err(BandAdjustmentsError::DuplicateBand { band: "sky".into() }));
    }

    #[test]
    fn neutral_entries_are_accepted_and_equal_to_absence() {
        let with_neutral = BandAdjustments::new(vec![BandAdjustment::neutral(id("sky"))]).unwrap();
        assert_eq!(with_neutral, BandAdjustments::NONE);
        assert!(with_neutral.is_neutral());
    }

    #[test]
    fn for_band_returns_neutral_when_absent_and_value_when_present() {
        let set = BandAdjustments::new(vec![sky(0.5)]).unwrap();
        assert_eq!(set.for_band(&id("sky")).hue.get(), 0.5);
        assert!(set.for_band(&id("grass")).is_neutral());
    }

    #[test]
    fn order_of_input_does_not_matter_for_equality() {
        let grass = BandAdjustment {
            saturation: LookKnob::new(-0.3).unwrap(),
            ..BandAdjustment::neutral(id("grass"))
        };
        let first = BandAdjustments::new(vec![sky(0.5), grass.clone()]).unwrap();
        let second = BandAdjustments::new(vec![grass, sky(0.5)]).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn with_adjustment_replaces_and_neutral_removes() {
        let set = BandAdjustments::new(vec![sky(0.5)]).unwrap();
        let replaced = set.with_adjustment(sky(-0.2));
        assert_eq!(replaced.for_band(&id("sky")).hue.get(), -0.2);
        assert_eq!(replaced.iter().count(), 1);
        let cleared = replaced.with_adjustment(BandAdjustment::neutral(id("sky")));
        assert!(cleared.is_neutral());
    }

    #[test]
    fn serde_rejects_duplicate_ids_and_out_of_range_values() {
        let duplicate = r#"[
            {"band":"sky","hue":0.1,"saturation":0,"luminance":0},
            {"band":"sky","hue":0.2,"saturation":0,"luminance":0}]"#;
        let error = serde_json::from_str::<BandAdjustments>(duplicate).unwrap_err();
        assert!(error.to_string().contains("повтор"), "{error}");

        let out_of_range = r#"[{"band":"sky","hue":3,"saturation":0,"luminance":0}]"#;
        assert!(serde_json::from_str::<BandAdjustments>(out_of_range).is_err());
    }

    #[test]
    fn serde_roundtrip_keeps_values() {
        let set = BandAdjustments::new(vec![sky(0.5)]).unwrap();
        let json = serde_json::to_string(&set).unwrap();
        assert_eq!(serde_json::from_str::<BandAdjustments>(&json).unwrap(), set);
    }
}
