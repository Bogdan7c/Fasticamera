//! Опыты над проявлением снимка (группа `still_tuning`): высота полосы и форма рабочей группы.
//!
//! [`StillTuning`] — ДАННЫЕ каталога, GPU они не знают: в настройки работы их превращает [`StillTuning::configure`].
//! Продукт ничего из этого не использует — только стенд насыщения.
//!
//! Высота полосы — поведение продукта (полоса — это то, что ставится в один кадр видоискателя), поэтому у работ
//! стенда считаются ДВЕ цифры: цена снимка целиком (пропускная способность) и цена образцовой полосы (метки
//! `photo::timing`, сумма проходов). Менять продуктовое умолчание это не меняет: итог — предложение владельцу.

use isp_vulkan::photo::ToneModelSpecialization;
use isp_vulkan::quality::WorkgroupSizeConfig;
use isp_vulkan::saturation::StillSaturationConfig;
use look_contract::ToneBaseModel;
use serde::Serialize;

/// Размер рабочей группы (нитей по X и Y).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct StillWorkgroup {
    pub x: u32,
    pub y: u32,
}

/// Как выходной проход снимка получает модель основы тона (зеркало `ToneModelSpecialization`: данные каталога
/// не тянут тип GPU-слоя, им нужны `Hash` и `Serialize`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StillOutputPath {
    /// Вариант пайплайна на модель (продуктовое умолчание).
    PerModel,
    /// Прежний путь: один пайплайн, модель читается из блока на лету.
    Runtime,
}

impl From<StillOutputPath> for ToneModelSpecialization {
    fn from(path: StillOutputPath) -> Self {
        match path {
            StillOutputPath::PerModel => Self::PerModel,
            StillOutputPath::Runtime => Self::Runtime,
        }
    }
}

/// Набор опытных настроек поверх продуктового проявления.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct StillTuning {
    /// Высота полосы, строки (`None` — продуктовая, 256).
    pub slice_rows: Option<u32>,
    /// Размер рабочей группы всех проходов (`None` — продуктовый).
    pub workgroup: Option<StillWorkgroup>,
    /// Модель основы тона кадра (`None` — как у вида продукта). Доходит до шейдера через параметры кадра.
    pub tone_model: Option<ToneBaseModel>,
    /// Путь выходного прохода (`None` — продуктовый).
    pub output_path: Option<StillOutputPath>,
}

impl StillTuning {
    /// Продукт без изменений.
    pub const PRODUCT: Self = Self {
        slice_rows: None,
        workgroup: None,
        tone_model: None,
        output_path: None,
    };

    /// Изменить высоту полосы.
    pub const fn slice_rows(self, rows: u32) -> Self {
        Self {
            slice_rows: Some(rows),
            ..self
        }
    }

    /// Изменить размер рабочей группы.
    pub const fn workgroup(self, x: u32, y: u32) -> Self {
        Self {
            workgroup: Some(StillWorkgroup { x, y }),
            ..self
        }
    }

    /// Снять кадр с основой тона `model` и выходным проходом `path`.
    pub const fn tone_model(self, model: ToneBaseModel, path: StillOutputPath) -> Self {
        Self {
            tone_model: Some(model),
            output_path: Some(path),
            ..self
        }
    }

    /// Настройки работы: `base` (продуктовые) с изменёнными полями.
    pub fn configure(self, base: StillSaturationConfig) -> StillSaturationConfig {
        let mut config = base;
        if let Some(rows) = self.slice_rows {
            config.photo.slice_rows = rows;
        }
        if let Some(shape) = self.workgroup {
            config.photo.workgroup_size = WorkgroupSizeConfig { x: shape.x, y: shape.y };
        }
        if let Some(path) = self.output_path {
            config.photo.tone_model_specialization = path.into();
        }
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_tuning_keeps_the_base_config() {
        let base = StillSaturationConfig::default();
        let configured = StillTuning::PRODUCT.configure(base);
        assert_eq!(configured.photo, base.photo);
    }

    #[test]
    fn tuning_overrides_only_the_requested_fields() {
        let base = StillSaturationConfig::default();
        let configured = StillTuning::PRODUCT.slice_rows(512).workgroup(128, 2).configure(base);
        assert_eq!(configured.photo.slice_rows, 512);
        assert_eq!(configured.photo.workgroup_size, WorkgroupSizeConfig { x: 128, y: 2 });
        assert_eq!(
            configured.photo.thumbnail_long_side_px,
            base.photo.thumbnail_long_side_px
        );
        let only_rows = StillTuning::PRODUCT.slice_rows(128).configure(base);
        assert_eq!(only_rows.photo.workgroup_size, base.photo.workgroup_size);
    }

    #[test]
    fn tone_model_tuning_selects_the_output_path_and_keeps_the_rest() {
        let base = StillSaturationConfig::default();
        assert_eq!(base.photo.tone_model_specialization, ToneModelSpecialization::PerModel);
        let tuning = StillTuning::PRODUCT.tone_model(ToneBaseModel::Aces, StillOutputPath::Runtime);
        let configured = tuning.configure(base);
        assert_eq!(tuning.tone_model, Some(ToneBaseModel::Aces));
        assert_eq!(
            configured.photo.tone_model_specialization,
            ToneModelSpecialization::Runtime
        );
        assert_eq!(configured.photo.slice_rows, base.photo.slice_rows);
    }
}
