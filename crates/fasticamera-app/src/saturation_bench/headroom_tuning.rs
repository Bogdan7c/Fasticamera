//! Опыты над цепочкой V4 (стенд «запас под видео»): размер рабочей группы, проходы Menon-пробы, отключённые стадии вывода.
//!
//! Это ДАННЫЕ каталога ([`HeadroomExperiment`]), GPU они не знают: в настройки работы их превращает
//! `gpu_executor::headroom_config`. Продукт ничего из этого не использует — только стенд насыщения. Любой опыт кроме
//! [`HeadroomExperiment::PRODUCT`] даёт бессмысленную картинку: это замер цены, а не рендер.

use serde::Serialize;

use super::viewfinder_tuning::WorkgroupShape;

/// Чем заменены проходы Menon (зеркало `isp_vulkan::quality::DemosaicProbe`: каталог GPU не знает).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DemosaicChoice {
    /// Настоящие проходы Menon 2007 (восемь).
    Real,
    /// Пустые проходы: цена запуска групп и барьеров.
    ProbeEmpty,
    /// Те же чтения и записи плоскостей f32, без математики.
    ProbeCopyF32,
    /// Те же чтения и записи, но отсчёты 16-битные (трафик вдвое меньше).
    ProbeCopyHalf,
}

/// Отключённые стадии вывода (зеркало `isp_vulkan::quality::OutputStageSkips`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct OutputSkipChoice {
    pub camera_matrix: bool,
    pub tone_base: bool,
    pub look: bool,
    pub output_gamut: bool,
    pub oetf: bool,
}

impl OutputSkipChoice {
    /// Всё включено, как в продукте.
    pub const NONE: Self = Self {
        camera_matrix: false,
        tone_base: false,
        look: false,
        output_gamut: false,
        oetf: false,
    };
}

/// Как выбирается модель основы тона на выходе (зеркало `isp_vulkan::quality::OutputToneModel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToneModelChoice {
    /// Читается из блока основы на лету (продукт): в шейдере живут все модели.
    Dynamic,
    /// Пайплайн собран только под AgX.
    AgxOnly,
}

/// Макет перепроекции стабилизатора (зеркало `isp_vulkan::quality::StabilizerMock`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StabilizerChoice {
    /// Нет макета.
    Off,
    /// Перепроекция слита с проходом вывода.
    MergedWithOutput,
}

/// Набор опытных настроек поверх продуктовой цепочки V4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct HeadroomExperiment {
    /// Размер группы всех проходов (`None` — продуктовый 16×16).
    pub workgroup: Option<WorkgroupShape>,
    /// Что стоит на месте проходов Menon.
    pub demosaic: DemosaicChoice,
    /// Какие стадии вывода выключены.
    pub output_skips: OutputSkipChoice,
    /// Как выбирается модель основы тона на выходе.
    pub tone_model: ToneModelChoice,
    /// Макет перепроекции стабилизатора в выводе.
    pub stabilizer: StabilizerChoice,
}

impl HeadroomExperiment {
    /// Продукт без изменений.
    pub const PRODUCT: Self = Self {
        workgroup: None,
        demosaic: DemosaicChoice::Real,
        output_skips: OutputSkipChoice::NONE,
        tone_model: ToneModelChoice::Dynamic,
        stabilizer: StabilizerChoice::Off,
    };

    /// Изменить размер группы.
    pub const fn workgroup(self, x: u32, y: u32) -> Self {
        Self {
            workgroup: Some(WorkgroupShape { x, y }),
            ..self
        }
    }

    /// Заменить проходы Menon.
    pub const fn demosaic(self, demosaic: DemosaicChoice) -> Self {
        Self { demosaic, ..self }
    }

    /// Выбрать способ задания модели основы тона.
    pub const fn tone_model(self, tone_model: ToneModelChoice) -> Self {
        Self { tone_model, ..self }
    }

    /// Включить макет перепроекции стабилизатора.
    pub const fn stabilizer(self, stabilizer: StabilizerChoice) -> Self {
        Self { stabilizer, ..self }
    }

    /// Выключить стадии вывода.
    pub const fn skip_output(self, output_skips: OutputSkipChoice) -> Self {
        Self { output_skips, ..self }
    }
}
