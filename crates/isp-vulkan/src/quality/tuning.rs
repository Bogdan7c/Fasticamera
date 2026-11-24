//! Опыты стенда V4 над цепочкой «запас под видео» (сессия 16a, пакет V): что заменить пробой или отключить, чтобы
//! увидеть, чем занято время кадра. Продукт ничего из этого не включает: умолчание ([`HeadroomTuning::PRODUCT`]) —
//! прежняя цепочка без изменений, а любое отклонение делает картинку бессмысленной (это замер цены, не рендер).
//!
//! # Что можно менять
//! * [`DemosaicProbe`] — заменить все 8 проходов Menon «пробами»: проход той же формы (те же плоскости на входе и
//!   выходе, те же группы), но без математики. Цена пробы = цена трафика памяти и запуска групп; сравнение с
//!   настоящими проходами показывает, сколько стоит их математика (и упирается ли цепочка в полосу памяти).
//! * [`OutputStageSkips`] — отключить стадии выходного прохода (матрица камеры, основа тона, вид, выходное сжатие
//!   охвата, OETF): их цена = разница времени прохода.

use serde::{Deserialize, Serialize};

use crate::shaders::blobs;

/// Чем заменить проходы Menon в замере.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DemosaicProbe {
    /// Проходы пустые: ни чтения, ни записи. Цена = запуск групп + барьеры.
    Empty,
    /// Читает и пишет плоскости f32 как настоящие проходы (по одному обращению на пиксель и плоскость), без математики.
    CopyF32,
    /// То же, но отсчёты 16-битные (`float16_t`): трафик памяти ровно вдвое меньше — «если бы плоскости были fp16».
    CopyHalf,
}

impl DemosaicProbe {
    /// SPIR-V пробы для прохода с `reads` читаемыми и `writes` записываемыми плоскостями.
    ///
    /// `None` — для такой формы прохода пробы нет (все формы настоящих проходов Menon покрыты, см. тест).
    pub(crate) fn spirv(self, reads: usize, writes: usize) -> Option<&'static [u8]> {
        match (self, reads, writes) {
            (Self::Empty, _, _) => Some(blobs::V4_PROBE_EMPTY_COMP),
            (Self::CopyF32, 1, 2) => Some(blobs::V4_PROBE_F32_R1W2_COMP),
            (Self::CopyF32, 2, 2) => Some(blobs::V4_PROBE_F32_R2W2_COMP),
            (Self::CopyF32, 3, 2) => Some(blobs::V4_PROBE_F32_R3W2_COMP),
            (Self::CopyF32, 4, 2) => Some(blobs::V4_PROBE_F32_R4W2_COMP),
            (Self::CopyF32, 4, 1) => Some(blobs::V4_PROBE_F32_R4W1_COMP),
            (Self::CopyHalf, 1, 2) => Some(blobs::V4_PROBE_HALF_R1W2_COMP),
            (Self::CopyHalf, 2, 2) => Some(blobs::V4_PROBE_HALF_R2W2_COMP),
            (Self::CopyHalf, 3, 2) => Some(blobs::V4_PROBE_HALF_R3W2_COMP),
            (Self::CopyHalf, 4, 2) => Some(blobs::V4_PROBE_HALF_R4W2_COMP),
            (Self::CopyHalf, 4, 1) => Some(blobs::V4_PROBE_HALF_R4W1_COMP),
            _ => None,
        }
    }
}

/// Какой демозаик стоит в цепочке.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DemosaicVariant {
    /// Прежние восемь проходов Menon (продукт до слияния).
    #[default]
    Reference,
    /// Пробы вместо проходов Menon: замер цены памяти и запуска групп, картинка бессмысленна.
    Probe(DemosaicProbe),
}

/// Какие стадии выходного прохода отключены (биты — константы `SKIP_*` в `v4_output_main.glsl`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct OutputStageSkips {
    /// Матрица «камера → XYZ D50» (вместо неё — сами отсчёты камеры).
    pub camera_matrix: bool,
    /// Основа тона AgX.
    pub tone_base: bool,
    /// Цвет вида и таблица вида.
    pub look: bool,
    /// Выходное сжатие охвата.
    pub output_gamut: bool,
    /// Кодирование OETF.
    pub oetf: bool,
}

impl OutputStageSkips {
    /// Ничего не отключено.
    pub const NONE: Self = Self {
        camera_matrix: false,
        tone_base: false,
        look: false,
        output_gamut: false,
        oetf: false,
    };

    /// Хоть одна стадия отключена (нужен шейдер стенда с маской, а не продуктовый).
    pub fn any(self) -> bool {
        self.mask() != 0
    }

    /// Маска для specialization-константы шейдера (раскладка битов — `v4_output_main.glsl`).
    pub fn mask(self) -> u32 {
        [
            (self.camera_matrix, 1_u32),
            (self.tone_base, 2),
            (self.look, 4),
            (self.output_gamut, 8),
            (self.oetf, 16),
        ]
        .into_iter()
        .filter(|(skipped, _)| *skipped)
        .fold(0, |mask, (_, bit)| mask | bit)
    }
}

/// Как выбирается модель основы тона в выходном проходе.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputToneModel {
    /// Модель читается из блока основы на лету (продукт): в шейдере живут все ветки (AgX, filmic, ACES, прежняя).
    #[default]
    Dynamic,
    /// Пайплайн собран только под AgX: остальные ветки выброшены на этапе создания пайплайна. Если кадр придёт с
    /// другой моделью, результат неверен — для боевого использования нужен выбор варианта по модели кадра.
    AgxOnly,
}

impl OutputToneModel {
    /// Значение specialization-константы `FIXED_TONE_MODEL` (`v4_output_main.glsl`).
    pub(crate) fn specialization_value(self) -> u32 {
        match self {
            Self::Dynamic => u32::MAX,
            // Номер AgX — `TONE_MODEL_AGX` в `tone_base.glsl`.
            Self::AgxOnly => 1,
        }
    }
}

/// Макет перепроекции стабилизатора в выходном проходе (оценка цены, не продукт).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StabilizerMock {
    /// Нет макета (продукт).
    #[default]
    Off,
    /// Выходной пиксель берёт цвет камеры из точки повёрнутого кадра со сдвигом строки (билинейно, 12 чтений плоскостей
    /// вместо 3): перепроекция слита с проходом вывода.
    MergedWithOutput,
}

/// Опыты стенда V4 поверх продуктовой цепочки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct HeadroomTuning {
    /// Какой демозаик стоит в цепочке.
    pub demosaic: DemosaicVariant,
    /// Отключённые стадии вывода.
    pub output_skips: OutputStageSkips,
    /// Как выбирается модель основы тона на выходе.
    pub output_tone_model: OutputToneModel,
    /// Макет перепроекции стабилизатора в выводе.
    pub stabilizer_mock: StabilizerMock,
}

impl HeadroomTuning {
    /// Продукт: ничего не заменено и не отключено.
    pub const PRODUCT: Self = Self {
        demosaic: DemosaicVariant::Reference,
        output_skips: OutputStageSkips::NONE,
        output_tone_model: OutputToneModel::Dynamic,
        stabilizer_mock: StabilizerMock::Off,
    };

    /// Опыт затрагивает выходной проход (нужен шейдер стенда со specialization-константами, а не продуктовый).
    pub fn is_output_experiment(self) -> bool {
        self.output_skips.any()
            || self.output_tone_model != OutputToneModel::Dynamic
            || self.stabilizer_mock != StabilizerMock::Off
    }

    /// Хоть что-то отличается от продукта (картинка такого прогона не имеет смысла).
    pub fn is_experiment(self) -> bool {
        matches!(self.demosaic, DemosaicVariant::Probe(_))
            || self.output_skips.any()
            || self.output_tone_model != OutputToneModel::Dynamic
            || self.stabilizer_mock != StabilizerMock::Off
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::demosaic::{MenonPass, PlaneAccess};

    #[test]
    fn skip_mask_collects_exactly_the_chosen_stages() {
        assert_eq!(OutputStageSkips::NONE.mask(), 0);
        assert!(!OutputStageSkips::NONE.any());
        let skips = OutputStageSkips {
            tone_base: true,
            oetf: true,
            ..OutputStageSkips::NONE
        };
        assert_eq!(skips.mask(), 2 | 16);
        assert!(skips.any());
    }

    #[test]
    fn product_tuning_is_not_an_experiment_and_any_probe_is() {
        assert!(!HeadroomTuning::PRODUCT.is_experiment());
        let probe = HeadroomTuning {
            demosaic: DemosaicVariant::Probe(DemosaicProbe::CopyF32),
            ..HeadroomTuning::PRODUCT
        };
        assert!(probe.is_experiment());
    }

    #[test]
    fn every_menon_pass_shape_has_a_probe_of_every_kind() {
        for pass in MenonPass::ORDER {
            let accesses = pass.plane_accesses();
            let reads = accesses.iter().filter(|a| matches!(a, PlaneAccess::Read(_))).count();
            let writes = accesses.len() - reads;
            for probe in [DemosaicProbe::Empty, DemosaicProbe::CopyF32, DemosaicProbe::CopyHalf] {
                let spirv = probe.spirv(reads, writes);
                assert!(spirv.is_some_and(|bytes| !bytes.is_empty()), "{pass:?} {probe:?}");
            }
        }
    }
}
