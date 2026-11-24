//! Семейство родственных вариантов пайплайна: что заготавливать вместе с созданным ключом.
//!
//! Это ЕДИНСТВЕННОЕ место решения о заготовке (раньше был один механизм только для моделей основы тона). Вариант
//! шейдера видоискателя меняется по трём «осям», которые в работе переключаются на ходу, без участия владельца
//! кэша:
//!
//! | Ось | Кто переключает | Значения |
//! |---|---|---|
//! | модель основы тона | свайп вида, ручка основы | [`ToneBaseKind::ALL`] |
//! | кадр со статистикой AWB или без | расписание: статистика каждый 4-й кадр | стадия [`IspStage::AwbStatistics`] есть/нет |
//! | раскладка групп по плиткам AWB | кадрирование зума (размер плитки в ячейках дробный или круглый) | [`AwbGroupLayout::ALL`] |
//!
//! Две последние оси склеены в [`StatisticsClass`]: без статистики раскладка ничего не значит (всегда `General`),
//! со статистикой она одна из возможных. Если при первом кадре с ключом создать только его, то первая смена
//! любой оси (первый зум, первый кадр без статистики, первая смена основы) компилировала бы пайплайн посреди кадра
//! видоискателя. Поэтому вместе с ключом создаётся всё его семейство: произведение осей.
//!
//! Что в семейство НЕ входит: набор остальных стадий (виньетирование, таблица вида). Они меняются редко и по воле
//! владельца калибровки, а не по каждому кадру; их варианты создаются по требованию (но резерв в ёмкости кэша на
//! них есть — [`crate::viewfinder::DEFAULT_PIPELINE_VARIANT_CAPACITY`]).

use super::awb_group_layout::AwbGroupLayout;
use super::awb_reduction::AwbReduction;
use super::stages::IspStage;
use super::variants::VariantKey;
use crate::tone_base::ToneBaseKind;

/// Что считает ли кадр статистику AWB и, если считает, как группы лежат на плитках: от этого зависит вариант.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatisticsClass {
    /// Кадр без статистики (раскладка не важна: ключ всегда с `General`).
    Off,
    /// Кадр со статистикой и раскладкой групп `layout`.
    On(AwbGroupLayout),
}

/// Сколько значений принимает ось «статистика»: без неё плюс каждая раскладка. Число ёмкости кэша выводится
/// отсюда, а не задаётся константой «на глаз».
pub(crate) const STATISTICS_CLASSES_PER_STAGE_SET: usize = 1 + AwbGroupLayout::ALL.len();

/// Что известно кэшу о работе ISP, когда он решает, что заготавливать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FamilyContext {
    /// Способ сведения AWB у всех вариантов: раскладка влияет только у `SharedMemoryWindow`.
    pub awb_reduction: AwbReduction,
    /// Этот ISP уже записывал кадры со статистикой AWB. Если нет (A/B-замер без стадии, статистика выключена
    /// конфигом), заготавливать варианты со статистикой незачем: они никогда не понадобятся.
    pub statistics_in_use: bool,
}

impl StatisticsClass {
    /// Класс, в который попадает ключ.
    fn of(key: &VariantKey) -> Self {
        if key.stages.contains(IspStage::AwbStatistics) {
            Self::On(key.awb_group_layout)
        } else {
            Self::Off
        }
    }

    /// Ключ `key` в этом классе: набор стадий и раскладка подгоняются, остальное не меняется.
    fn applied_to(self, key: VariantKey) -> VariantKey {
        match self {
            Self::Off => VariantKey {
                stages: key.stages.without(IspStage::AwbStatistics),
                awb_group_layout: AwbGroupLayout::General,
                ..key
            },
            Self::On(layout) => VariantKey {
                stages: key.stages.with(IspStage::AwbStatistics),
                awb_group_layout: layout,
                ..key
            },
        }
    }
}

/// Все классы статистики, которые могут понадобиться рядом с ключом `key`.
fn statistics_classes(key: &VariantKey, context: FamilyContext) -> Vec<StatisticsClass> {
    let key_has_statistics = key.stages.contains(IspStage::AwbStatistics);
    if !key_has_statistics && !context.statistics_in_use {
        return vec![StatisticsClass::Off];
    }
    // Раскладка влияет на вариант только при сведении через shared-память; иначе она всегда `General`.
    let layouts: &[AwbGroupLayout] = match context.awb_reduction {
        AwbReduction::SharedMemoryWindow => &AwbGroupLayout::ALL,
        AwbReduction::SubgroupSingleTile => &[AwbGroupLayout::General],
    };
    std::iter::once(StatisticsClass::Off)
        .chain(layouts.iter().copied().map(StatisticsClass::On))
        .collect()
}

/// Родственные варианты ключа `key` (без него самого) в порядке «нужнее раньше»: сначала те, что отличаются только
/// статистикой (она чередуется каждые несколько кадров), затем только основой, затем обоими. Порядок важен при тесном
/// кэше: заготовка берёт лишь свободные места и останавливается, когда они кончились.
pub(crate) fn related_variants(key: VariantKey, context: FamilyContext) -> Vec<VariantKey> {
    let own_class = StatisticsClass::of(&key);
    let mut candidates = Vec::new();
    for class in statistics_classes(&key, context) {
        for tone_base in ToneBaseKind::ALL {
            let differs_by_statistics = class != own_class;
            let differs_by_tone_base = tone_base != key.tone_base;
            if !differs_by_statistics && !differs_by_tone_base {
                continue;
            }
            let sibling = class.applied_to(VariantKey { tone_base, ..key });
            // Разные классы и модели могут дать один ключ (после подгонки стадий): дубликатов не держим.
            if sibling != key && !candidates.iter().any(|(_, _, known)| *known == sibling) {
                candidates.push((
                    u8::from(differs_by_statistics) + u8::from(differs_by_tone_base),
                    differs_by_tone_base,
                    sibling,
                ));
            }
        }
    }
    // Устойчивая сортировка: меньше различий раньше; при равенстве — «только статистика» раньше «только основа».
    candidates.sort_by_key(|(differences, differs_by_tone_base, _)| (*differences, *differs_by_tone_base));
    candidates.into_iter().map(|(_, _, sibling)| sibling).collect()
}

/// Сколько вариантов у ОДНОГО набора стадий (остальных стадий) в худшем случае: класс статистики × модель основы.
/// Основа ёмкости кэша ([`crate::viewfinder::DEFAULT_PIPELINE_VARIANT_CAPACITY`]).
pub(crate) const VARIANTS_PER_STAGE_SET: usize = STATISTICS_CLASSES_PER_STAGE_SET * ToneBaseKind::ALL.len();

/// Для тестов других модулей: набор стадий без побочных ограничений.
#[cfg(test)]
pub(crate) fn product_key(tone_base: ToneBaseKind, layout: AwbGroupLayout) -> VariantKey {
    use super::stages::IspStageSet;
    use super::variants::SourceKind;
    use gpu_vulkan::storage_image::StorageImageFormat;
    VariantKey {
        source: SourceKind::SampledRaw16,
        format: StorageImageFormat::Rgba16Float,
        stages: IspStageSet::ALL,
        tone_base,
        awb_group_layout: layout,
    }
}

#[cfg(test)]
mod tests {
    use super::super::stages::IspStageSet;
    use super::*;

    const SHARED: FamilyContext = FamilyContext {
        awb_reduction: AwbReduction::SharedMemoryWindow,
        statistics_in_use: true,
    };

    #[test]
    fn statistics_key_gets_every_other_tone_base_layout_and_the_no_statistics_frame() {
        let key = product_key(ToneBaseKind::Agx, AwbGroupLayout::SingleTilePerGroup);
        let siblings = related_variants(key, SHARED);
        // 3 класса (без статистики + 2 раскладки) × 4 основы − сам ключ.
        assert_eq!(siblings.len(), VARIANTS_PER_STAGE_SET - 1);
        assert_eq!(VARIANTS_PER_STAGE_SET, 12);
        let unique: std::collections::HashSet<_> = siblings.iter().copied().collect();
        assert_eq!(unique.len(), siblings.len(), "дубликатов нет");
        assert!(!siblings.contains(&key), "сам ключ не входит");
        // Кадр без статистики той же основы — раскладка у него всегда `General`.
        let no_statistics = VariantKey {
            stages: key.stages.without(IspStage::AwbStatistics),
            awb_group_layout: AwbGroupLayout::General,
            ..key
        };
        assert!(siblings.contains(&no_statistics));
        // Другая раскладка той же основы.
        assert!(siblings.contains(&VariantKey {
            awb_group_layout: AwbGroupLayout::General,
            ..key
        }));
        for tone_base in ToneBaseKind::ALL {
            for layout in AwbGroupLayout::ALL {
                let expected = VariantKey {
                    tone_base,
                    awb_group_layout: layout,
                    ..key
                };
                assert!(
                    expected == key || siblings.contains(&expected),
                    "{tone_base:?} {layout:?}"
                );
            }
        }
    }

    #[test]
    fn nearest_siblings_come_first_so_a_tight_cache_keeps_the_most_needed() {
        let key = product_key(ToneBaseKind::Legacy, AwbGroupLayout::General);
        let siblings = related_variants(key, SHARED);
        let differences = |sibling: &VariantKey| {
            let by_tone = sibling.tone_base != key.tone_base;
            let by_statistics = StatisticsClass::of(sibling) != StatisticsClass::of(&key);
            usize::from(by_tone) + usize::from(by_statistics)
        };
        let order: Vec<usize> = siblings.iter().map(differences).collect();
        assert!(
            order.windows(2).all(|pair| pair[0] <= pair[1]),
            "по числу отличий: {order:?}"
        );
        // Первыми — те, что отличаются только статистикой (чередуется каждые несколько кадров).
        assert_eq!(siblings[0].tone_base, key.tone_base);
        assert_ne!(StatisticsClass::of(&siblings[0]), StatisticsClass::of(&key));
    }

    #[test]
    fn subgroup_reduction_has_no_layout_axis() {
        let key = product_key(ToneBaseKind::Agx, AwbGroupLayout::General);
        let context = FamilyContext {
            awb_reduction: AwbReduction::SubgroupSingleTile,
            ..SHARED
        };
        let siblings = related_variants(key, context);
        // 2 класса (без статистики, со статистикой) × 4 основы − сам ключ.
        assert_eq!(siblings.len(), 2 * ToneBaseKind::ALL.len() - 1);
        assert!(
            siblings
                .iter()
                .all(|sibling| sibling.awb_group_layout == AwbGroupLayout::General)
        );
    }

    #[test]
    fn statistics_siblings_appear_only_after_this_isp_has_used_statistics() {
        let no_statistics_key = VariantKey {
            stages: IspStageSet::ALL.without(IspStage::AwbStatistics),
            ..product_key(ToneBaseKind::Legacy, AwbGroupLayout::General)
        };
        let never_used = FamilyContext {
            statistics_in_use: false,
            ..SHARED
        };
        let siblings = related_variants(no_statistics_key, never_used);
        assert_eq!(siblings.len(), ToneBaseKind::ALL.len() - 1, "только другие основы");
        assert!(
            siblings
                .iter()
                .all(|sibling| !sibling.stages.contains(IspStage::AwbStatistics)),
            "стадию статистики не добавляем: она не нужна"
        );
        // Статистика уже была — кадр без неё заготавливает и кадры со статистикой.
        let used = related_variants(no_statistics_key, SHARED);
        assert_eq!(used.len(), VARIANTS_PER_STAGE_SET - 1);
        assert!(
            used.iter()
                .any(|sibling| sibling.stages.contains(IspStage::AwbStatistics))
        );
    }

    #[test]
    fn other_stages_and_source_stay_exactly_as_in_the_key() {
        let key = VariantKey {
            stages: IspStageSet::ALL
                .without(IspStage::LensShading)
                .without(IspStage::LookLut),
            ..product_key(ToneBaseKind::Aces, AwbGroupLayout::General)
        };
        for sibling in related_variants(key, SHARED) {
            assert_eq!((sibling.source, sibling.format), (key.source, key.format));
            assert!(!sibling.stages.contains(IspStage::LensShading));
            assert!(!sibling.stages.contains(IspStage::LookLut));
            assert!(sibling.stages.contains(IspStage::Tone));
        }
    }
}
