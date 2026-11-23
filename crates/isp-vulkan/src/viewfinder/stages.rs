//! Выключатели стадий ISP видоискателя ([`IspStageSet`]) — specialization-константы шейдера.
//!
//! Решение владельца (07): продукт работает ОДНИМ слитым проходом; цену каждой стадии меряем A/B:
//! тот же проход без стадии. Выключенная стадия вырезается компилятором драйвера (константа известна
//! при создании пайплайна), поэтому разница во времени — это и есть цена стадии.
//!
//! # Что делает выключенная стадия (шейдер `viewfinder_isp.glsl`)
//! | Стадия | Выключена |
//! |---|---|
//! | [`IspStage::Levels`] | `raw / white` без вычитания чёрного (чёрное «приподнято») |
//! | [`IspStage::LensShading`] | виньетирование не корректируется (углы темнее центра), карту шейдер не читает |
//! | [`IspStage::WhiteBalance`] | без усилений и без обрезки пересвета |
//! | [`IspStage::Demosaic`] | отсчёт позиции 0 ячейки во все три канала (без статистики читается один отсчёт вместо четырёх) |
//! | [`IspStage::Color`] | RGB камеры идёт дальше как свет экрана (без обеих матриц) |
//! | [`IspStage::GamutCompression`] | свет как есть (отрицательные каналы обнулит тон или OETF) |
//! | [`IspStage::Tone`] | свет сцены как есть (без S-кривой и сохранения оттенка) |
//! | [`IspStage::Oetf`] | линейный свет пишется как есть (UNORM-картинка обрежет до 0…1) |
//! | [`IspStage::AwbStatistics`] | статистика не считается (ни shared-памяти, ни атомиков), `collect` вернёт `None` |
//!
//! Картинка с выключенной стадией неправильная — это осознанно: набор нужен для замера, а не для показа.
//! Все стадии включены = продукт ([`IspStageSet::ALL`]).

use gpu_vulkan::compute::SpecializationConstant;
use isp_model::lens_shading::LensShadingStep;

/// Отключаемая стадия слитого прохода.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IspStage {
    /// Уровни чёрного и белого.
    Levels,
    /// Коррекция виньетирования по нашей калибровке (карта усилений над полезной областью). Решение «пропуск»
    /// в параметрах кадра ([`isp_model::lens_shading::LensShadingStep::Skip`]) выключает её так же, как набор.
    LensShading,
    /// Баланс белого и обрезка пересвета.
    WhiteBalance,
    /// Демозаик 2×2 (суперпиксель).
    Demosaic,
    /// Матрицы камера → XYZ D50 → экран.
    Color,
    /// Мягкое сжатие охвата.
    GamutCompression,
    /// Тон-кривая с сохранением оттенка.
    Tone,
    /// Кривая передачи sRGB.
    Oetf,
    /// Статистика для AWB по плиткам.
    AwbStatistics,
}

impl IspStage {
    /// Все стадии в порядке прохода.
    pub const ALL: [Self; 9] = [
        Self::Levels,
        Self::LensShading,
        Self::WhiteBalance,
        Self::Demosaic,
        Self::Color,
        Self::GamutCompression,
        Self::Tone,
        Self::Oetf,
        Self::AwbStatistics,
    ];

    /// `constant_id` выключателя в шейдере (0 и 1 — размер рабочей группы). Номера не зависят от порядка
    /// прохода: стадия, добавленная позже, берёт следующий свободный (виньетирование — 10).
    pub const fn specialization_constant_id(self) -> u32 {
        match self {
            Self::Levels => 2,
            Self::WhiteBalance => 3,
            Self::Demosaic => 4,
            Self::Color => 5,
            Self::GamutCompression => 6,
            Self::Tone => 7,
            Self::Oetf => 8,
            Self::AwbStatistics => 9,
            Self::LensShading => 10,
        }
    }

    /// Бит стадии в наборе.
    const fn bit(self) -> u16 {
        1 << (self.specialization_constant_id() - FIRST_STAGE_CONSTANT_ID)
    }
}

/// `constant_id` первой стадии в шейдере.
const FIRST_STAGE_CONSTANT_ID: u32 = 2;

/// Набор включённых стадий (битовая маска внутри — наружу только intent-методы).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IspStageSet(u16);

impl IspStageSet {
    /// Все стадии — продукт.
    pub const ALL: Self = Self(0x1FF);

    /// Набор без указанной стадии (для A/B-замера её цены).
    pub const fn without(self, stage: IspStage) -> Self {
        Self(self.0 & !stage.bit())
    }

    /// Набор с указанной стадией.
    pub const fn with(self, stage: IspStage) -> Self {
        Self(self.0 | stage.bit())
    }

    /// Набор с учётом решения стадии виньетирования в параметрах кадра: «пропуск» (нет калибровки или
    /// выключено управлением) вырезает стадию из прохода, остальные стадии не меняются.
    pub fn resolved_for_lens_shading(self, step: &LensShadingStep) -> Self {
        match step {
            LensShadingStep::Skip(_) => self.without(IspStage::LensShading),
            LensShadingStep::Apply(_) => self,
        }
    }

    /// Включена ли стадия.
    pub const fn contains(self, stage: IspStage) -> bool {
        self.0 & stage.bit() != 0
    }

    /// Specialization-константы выключателей (булевы константы GLSL — 32-битные 0/1).
    pub fn specialization_constants(self) -> [SpecializationConstant; 9] {
        IspStage::ALL.map(|stage| SpecializationConstant {
            constant_id: stage.specialization_constant_id(),
            value: u32::from(self.contains(stage)),
        })
    }
}

impl Default for IspStageSet {
    /// По умолчанию — продукт: все стадии.
    fn default() -> Self {
        Self::ALL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_contains_every_stage_and_without_removes_exactly_one() {
        for stage in IspStage::ALL {
            assert!(IspStageSet::ALL.contains(stage));
            let reduced = IspStageSet::ALL.without(stage);
            assert!(!reduced.contains(stage));
            for other in IspStage::ALL.into_iter().filter(|other| *other != stage) {
                assert!(reduced.contains(other), "{stage:?} выключил ещё и {other:?}");
            }
            assert_eq!(reduced.with(stage), IspStageSet::ALL);
        }
    }

    #[test]
    fn skipped_lens_shading_removes_only_that_stage() {
        use isp_model::lens_shading::{LensShadingGainMap, LensShadingSkipReason};
        use std::sync::Arc;

        let skipped = LensShadingStep::Skip(LensShadingSkipReason::NotCalibrated);
        let resolved = IspStageSet::ALL.resolved_for_lens_shading(&skipped);
        assert_eq!(resolved, IspStageSet::ALL.without(IspStage::LensShading));
        let applied = LensShadingStep::Apply(Arc::new(LensShadingGainMap::new(2, 2, vec![[1.0; 4]; 4]).unwrap()));
        assert_eq!(IspStageSet::ALL.resolved_for_lens_shading(&applied), IspStageSet::ALL);
        // A/B-набор без стадии остаётся без неё и при готовой карте: решение набора не перекрывается параметрами.
        let ab_set = IspStageSet::ALL.without(IspStage::LensShading);
        assert_eq!(ab_set.resolved_for_lens_shading(&applied), ab_set);
    }

    #[test]
    fn all_mask_covers_exactly_the_stage_bits() {
        // Ни одна стадия не теряется в маске и в ней нет лишних бит: добавление стадии без обновления ALL
        // (например, при переходе за 8 бит) поймает этот тест.
        let union = IspStage::ALL.iter().fold(0_u16, |mask, stage| mask | stage.bit());
        assert_eq!(IspStageSet::ALL.0, union);
        assert_eq!(union.count_ones() as usize, IspStage::ALL.len());
        assert!(IspStageSet::ALL.contains(IspStage::LensShading));
        assert!(
            !IspStageSet::ALL
                .without(IspStage::LensShading)
                .contains(IspStage::LensShading)
        );
    }

    #[test]
    fn specialization_constants_follow_shader_ids_and_flags() {
        let constants = IspStageSet::ALL.without(IspStage::Tone).specialization_constants();
        let ids: Vec<u32> = constants.iter().map(|constant| constant.constant_id).collect();
        assert_eq!(
            ids,
            vec![2, 10, 3, 4, 5, 6, 7, 8, 9],
            "зеркало constant_id в viewfinder_isp.glsl, в порядке IspStage::ALL"
        );
        for constant in constants {
            let expected = u32::from(constant.constant_id != IspStage::Tone.specialization_constant_id());
            assert_eq!(constant.value, expected, "{constant:?}");
        }
    }
}
