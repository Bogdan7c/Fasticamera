//! Настройки видоискателя камеры и цвета экрана (сессия 07).
//!
//! JSON-ключи — `display_color` и `viewfinder` в конфиге потока рендера; отсутствующие поля —
//! из [`Default`], неизвестные — ошибка. Зеркала типов `camera-contract`/`gpu-vulkan` здесь нужны
//! потому, что те не читаются serde (словарь камеры только пишет JSON); перевод — методами `to_*`.

use camera_contract::viewfinder_crop::{CropAspect, ViewfinderCrop, ZoomFactor};
use isp_vulkan::quality::VideoHeadroomConfig;
use isp_vulkan::viewfinder::ViewfinderIspConfig;
use serde::{Deserialize, Serialize};

use super::auto_exposure::{AutoExposureConfig, AutoExposureConfigError};
use super::white_balance::{WhiteBalanceConfig, WhiteBalanceConfigError};
use crate::viewfinder::ViewfinderPainter;

/// Наибольшее окно статистики времени GPU видоискателя, кадров (~28 минут при 60 к/с).
pub const MAX_VIEWFINDER_GPU_TIME_WINDOW_FRAMES: usize = 100_000;

/// Какое цветовое пространство просить у цепочки показа.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayColorConfig {
    /// Display P3 + 10 бит, если экран предлагает; иначе sRGB. По умолчанию: экран OnePlus 11 и
    /// эталон iPhone работают в P3 (решение 25), ISP кодирует выход под выбранное пространство.
    #[default]
    PreferDisplayP3,
    /// Только sRGB (сравнения, отладка).
    SrgbOnly,
}

/// Соотношение сторон кадрирования (зеркало `CropAspect` для конфига).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CropAspectConfig {
    /// Как у полезной области сенсора.
    #[default]
    SensorNative,
    /// Длинная сторона : короткая (`long ≥ short > 0`).
    LongToShort {
        /// Длинная сторона.
        long: u32,
        /// Короткая сторона.
        short: u32,
    },
}

/// Кадрирование видоискателя (зеркало `ViewfinderCrop`). По умолчанию — вся полезная область без зума.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ViewfinderCropConfig {
    /// Соотношение сторон.
    pub aspect: CropAspectConfig,
    /// Цифровой зум (конечный, ≥ 1).
    pub zoom: f32,
}

impl Default for ViewfinderCropConfig {
    fn default() -> Self {
        Self {
            aspect: CropAspectConfig::SensorNative,
            zoom: 1.0,
        }
    }
}

impl ViewfinderCropConfig {
    /// Проверенное кадрирование словаря камеры.
    pub fn to_crop(&self) -> Result<ViewfinderCrop, ViewfinderConfigError> {
        let zoom = ZoomFactor::new(self.zoom).map_err(|_| ViewfinderConfigError::InvalidZoom(self.zoom))?;
        let aspect = match self.aspect {
            CropAspectConfig::SensorNative => CropAspect::SensorNative,
            CropAspectConfig::LongToShort { long, short } => {
                if short == 0 || long < short {
                    return Err(ViewfinderConfigError::InvalidAspect { long, short });
                }
                CropAspect::LongToShort { long, short }
            }
        };
        Ok(ViewfinderCrop { aspect, zoom })
    }
}

/// Где на экране лежит видоискатель, пока экран камеры не прислал свой прямоугольник командой
/// `SetFraming` (решение 93): изображение по ширине логической цели (как у системных камер в портрете).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewfinderPlacement {
    /// По ширине цели, по центру по вертикали. По умолчанию.
    #[default]
    FitWidthCentered,
    /// По ширине цели, прижато к верху.
    FitWidthTop,
}

/// Секция `viewfinder`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ViewfinderConfig {
    /// Чем рисовать на старте (меняется на ходу командой). По умолчанию — ISP.
    pub painter: ViewfinderPainter,
    /// ISP видоискателя. `frame_slots` здесь НЕ действует: слотов столько, сколько кадров в полёте
    /// у цепочки показа (`frames_in_flight`), — слоты ISP обязаны совпадать со слотами кольца кадров.
    pub isp: ViewfinderIspConfig,
    /// Кадрирование на старте (меняется на ходу командой).
    pub crop: ViewfinderCropConfig,
    /// Положение на экране.
    pub placement: ViewfinderPlacement,
    /// Окно статистики времени GPU (p50/p95/max), кадров. 600 — 10 с при 60 к/с, как окно
    /// пейсинга: хватает для медианы и p95 таблицы «стадия → мс GPU» (решение 45).
    pub gpu_time_window_frames: usize,
    /// Стенд «запас под видео» (V4): размер рабочей группы, шумодав, формат выхода. Сам стенд на
    /// старте всегда выключен (включается командой `SetHeadroomBench`). `frame_slots` здесь НЕ
    /// действует: слотов столько же, сколько кадров в полёте у цепочки показа (как у ISP).
    pub headroom_bench: VideoHeadroomConfig,
    /// Наш AWB (сессия 08): оценка света, скрытие первых кадров, прореживание статистики.
    pub white_balance: WhiteBalanceConfig,
    /// Наш AE (сессия 08a): замер, контроллер цели, предел остаточной подтяжки. Действует, только если
    /// композиция передала бэкенду ящики AE (`with_auto_exposure`); иначе AE — вендорский.
    pub auto_exposure: AutoExposureConfig,
}

impl Default for ViewfinderConfig {
    fn default() -> Self {
        Self {
            painter: ViewfinderPainter::default(),
            isp: ViewfinderIspConfig::default(),
            crop: ViewfinderCropConfig::default(),
            placement: ViewfinderPlacement::default(),
            gpu_time_window_frames: 600,
            headroom_bench: VideoHeadroomConfig::default(),
            white_balance: WhiteBalanceConfig::default(),
            auto_exposure: AutoExposureConfig::default(),
        }
    }
}

impl ViewfinderConfig {
    /// Проверяет секцию. Слоты ISP не проверяются (см. поле `isp`): их задаёт цепочка показа.
    pub fn validate(&self) -> Result<(), ViewfinderConfigError> {
        self.crop.to_crop()?;
        if self.gpu_time_window_frames == 0 || self.gpu_time_window_frames > MAX_VIEWFINDER_GPU_TIME_WINDOW_FRAMES {
            return Err(ViewfinderConfigError::GpuTimeWindowOutOfRange(
                self.gpu_time_window_frames,
            ));
        }
        self.white_balance.validate()?;
        self.auto_exposure.validate()?;
        let with_any_valid_slots = ViewfinderIspConfig {
            frame_slots: 1,
            ..self.isp
        };
        with_any_valid_slots
            .validate()
            .map_err(|error| ViewfinderConfigError::Isp(error.to_string()))?;
        let bench_with_any_valid_slots = VideoHeadroomConfig {
            frame_slots: 1,
            ..self.headroom_bench
        };
        bench_with_any_valid_slots
            .validate()
            .map_err(|error| ViewfinderConfigError::HeadroomBench(error.to_string()))
    }
}

/// Что не так с секцией `viewfinder`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ViewfinderConfigError {
    /// Зум NaN, бесконечный или меньше 1.
    #[error("viewfinder.crop.zoom должен быть конечным и не меньше 1, а не {0}")]
    InvalidZoom(f32),
    /// Соотношение сторон с нулём или короткой стороной длиннее длинной.
    #[error("viewfinder.crop.aspect {long}:{short}: нужно long ≥ short > 0")]
    InvalidAspect {
        /// Длинная сторона.
        long: u32,
        /// Короткая сторона.
        short: u32,
    },
    /// Окно статистики пустое или абсурдно большое.
    #[error("viewfinder.gpu_time_window_frames должен лежать в 1..={MAX_VIEWFINDER_GPU_TIME_WINDOW_FRAMES}, а не {0}")]
    GpuTimeWindowOutOfRange(usize),
    /// Настройки ISP отвергнуты самим ISP.
    #[error("viewfinder.isp: {0}")]
    Isp(String),
    /// Настройки стенда V4 отвергнуты самим конвейером.
    #[error("viewfinder.headroom_bench: {0}")]
    HeadroomBench(String),
    /// Настройки AWB отвергнуты.
    #[error(transparent)]
    WhiteBalance(#[from] WhiteBalanceConfigError),
    /// Настройки AE отвергнуты.
    #[error(transparent)]
    AutoExposure(#[from] AutoExposureConfigError),
}

#[cfg(test)]
mod tests {
    use isp_vulkan::viewfinder::WorkgroupSizeConfig;

    use super::*;

    #[test]
    fn defaults_are_valid_isp_full_sensor_centered_and_p3() {
        let config = ViewfinderConfig::default();
        assert_eq!(config.validate(), Ok(()));
        assert_eq!(config.painter, ViewfinderPainter::Isp);
        assert_eq!(config.crop.to_crop(), Ok(ViewfinderCrop::FULL_SENSOR));
        assert_eq!(config.placement, ViewfinderPlacement::FitWidthCentered);
        assert_eq!(DisplayColorConfig::default(), DisplayColorConfig::PreferDisplayP3);
    }

    #[test]
    fn json_round_trip_and_unknown_fields_are_rejected() {
        let parsed: ViewfinderConfig = serde_json::from_str(
            r#"{"painter":"debug_mosaic","crop":{"aspect":{"kind":"long_to_short","long":16,"short":9},"zoom":2.0},"placement":"fit_width_top"}"#,
        )
        .expect("секция читается");
        assert_eq!(parsed.painter, ViewfinderPainter::DebugMosaic);
        assert_eq!(parsed.placement, ViewfinderPlacement::FitWidthTop);
        let crop = parsed.crop.to_crop().expect("кадрирование корректно");
        assert_eq!(crop.aspect, CropAspect::SIXTEEN_BY_NINE);
        assert_eq!(crop.zoom.value(), 2.0);
        assert!(serde_json::from_str::<ViewfinderConfig>(r#"{"paintr":"isp"}"#).is_err());
        assert!(serde_json::from_str::<ViewfinderConfig>(r#"{"isp":{"frame_slot":2}}"#).is_err());
    }

    #[test]
    fn invalid_values_are_rejected_with_specific_errors() {
        let base = ViewfinderConfig::default();
        let zoom_below_one = ViewfinderConfig {
            crop: ViewfinderCropConfig {
                zoom: 0.5,
                ..ViewfinderCropConfig::default()
            },
            ..base.clone()
        };
        assert_eq!(zoom_below_one.validate(), Err(ViewfinderConfigError::InvalidZoom(0.5)));
        let tall_aspect = ViewfinderConfig {
            crop: ViewfinderCropConfig {
                aspect: CropAspectConfig::LongToShort { long: 3, short: 4 },
                zoom: 1.0,
            },
            ..base.clone()
        };
        assert_eq!(
            tall_aspect.validate(),
            Err(ViewfinderConfigError::InvalidAspect { long: 3, short: 4 })
        );
        let empty_window = ViewfinderConfig {
            gpu_time_window_frames: 0,
            ..base.clone()
        };
        assert_eq!(
            empty_window.validate(),
            Err(ViewfinderConfigError::GpuTimeWindowOutOfRange(0))
        );
        let broken_isp = ViewfinderConfig {
            isp: ViewfinderIspConfig {
                workgroup_size: WorkgroupSizeConfig { x: 0, y: 8 },
                ..base.isp
            },
            ..base
        };
        assert!(matches!(broken_isp.validate(), Err(ViewfinderConfigError::Isp(_))));
    }

    #[test]
    fn headroom_bench_section_is_read_validated_and_ignores_frame_slots() {
        let parsed: ViewfinderConfig =
            serde_json::from_str(r#"{"headroom_bench":{"frame_slots":0,"workgroup_size":{"x":8,"y":4}}}"#)
                .expect("секция стенда читается");
        assert_eq!(
            (
                parsed.headroom_bench.workgroup_size.x,
                parsed.headroom_bench.workgroup_size.y
            ),
            (8, 4)
        );
        assert_eq!(parsed.validate(), Ok(()), "слоты стенда задаёт цепочка показа");
        assert!(serde_json::from_str::<ViewfinderConfig>(r#"{"headroom_bench":{"slots":2}}"#).is_err());
        let broken = ViewfinderConfig {
            headroom_bench: isp_vulkan::quality::VideoHeadroomConfig {
                workgroup_size: isp_vulkan::quality::WorkgroupSizeConfig { x: 64, y: 32 },
                ..isp_vulkan::quality::VideoHeadroomConfig::default()
            },
            ..ViewfinderConfig::default()
        };
        assert!(matches!(
            broken.validate(),
            Err(ViewfinderConfigError::HeadroomBench(_))
        ));
    }

    #[test]
    fn isp_frame_slots_from_config_do_not_matter() {
        // Слоты задаёт цепочка показа: даже «неверное» значение здесь не ломает секцию.
        let config = ViewfinderConfig {
            isp: ViewfinderIspConfig {
                frame_slots: 0,
                ..ViewfinderIspConfig::default()
            },
            ..ViewfinderConfig::default()
        };
        assert_eq!(config.validate(), Ok(()));
    }
}
