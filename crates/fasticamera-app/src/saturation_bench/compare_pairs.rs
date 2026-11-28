//! Пары «эталон и вариант» стенда насыщения: какая работа сравнивается по картинке с какой (сессия 16a, пакет IL).
//!
//! Отпечаток выхода отвечает только «совпало или нет». Для неточных оптимизаций нужно «насколько»: у работы-варианта
//! в списке есть работа-эталон; после прогона обеих исполнитель читает байты их картинок и пишет в итог варианта отчёт
//! `ImageDiffReport` (max |Δ| в кодах 10 бит, число пикселей с Δ ≥ 1 и ≥ 2, гистограмма). Эталон обязан идти в списке
//! РАНЬШЕ варианта и на той же сцене: иначе отчёт честно говорит «эталон не прогонялся».
//!
//! Таблица — данные: новая пара — новая строка, GPU-кода здесь нет.

/// Пара: `variant` сравнивается с `reference`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComparePair {
    pub variant: &'static str,
    pub reference: &'static str,
}

/// Пары каталога (порядок не важен).
pub const PAIRS: &[ComparePair] = &[
    // Самопроверка механизма: двойник точной работы обязан совпасть с ней бит в бит (отчёт «Δ = 0»).
    ComparePair {
        variant: "viewfinder_awb_never_twin",
        reference: "viewfinder_awb_never",
    },
    // Группа `look_cost` (14o): вариант «вкл» против «выкл»-эталона, который идёт в списке раньше. Заметка `diff_vs=`
    // в итоге варианта показывает, что рычаг действительно меняет картинку (иначе цена рычага — цена пустоты).
    ComparePair {
        variant: "look_product",
        reference: "look_neutral",
    },
    ComparePair {
        variant: "look_tone_base_agx",
        reference: "look_tone_base_legacy",
    },
    ComparePair {
        variant: "look_tone_base_aces",
        reference: "look_tone_base_legacy",
    },
    ComparePair {
        variant: "look_tone_base_filmic",
        reference: "look_tone_base_legacy",
    },
    ComparePair {
        variant: "look_wheels_sample",
        reference: "look_without_wheels",
    },
    ComparePair {
        variant: "look_tone_sample",
        reference: "look_without_tone",
    },
    ComparePair {
        variant: "look_primaries_sample",
        reference: "look_without_primaries",
    },
];

/// Эталон, с которым сравнивается работа `name` (`None` — работа ни с чем не сравнивается).
pub fn reference_of(name: &str) -> Option<&'static str> {
    PAIRS
        .iter()
        .find(|pair| pair.variant == name)
        .map(|pair| pair.reference)
}

/// Нужны ли байты картинки работы `name` для чьих-то сравнений (работа — эталон или вариант).
pub fn needs_pixels(name: &str) -> bool {
    PAIRS.iter().any(|pair| pair.variant == name || pair.reference == name)
}

/// Работа — эталон хотя бы одной пары (её байты надо запомнить до конца прогона).
pub fn is_reference(name: &str) -> bool {
    PAIRS.iter().any(|pair| pair.reference == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_resolve_in_both_directions() {
        assert_eq!(reference_of("viewfinder_awb_never_twin"), Some("viewfinder_awb_never"));
        assert_eq!(
            reference_of("viewfinder_awb_never"),
            None,
            "эталон сам ни с чем не сравнивается"
        );
        assert!(is_reference("viewfinder_awb_never") && needs_pixels("viewfinder_awb_never_twin"));
        assert!(!needs_pixels("viewfinder") && !is_reference("viewfinder_awb_never_twin"));
    }

    #[test]
    fn every_pair_names_entries_that_exist_in_the_catalog_and_the_reference_is_not_a_variant() {
        use super::super::catalog::find_entry;
        for pair in PAIRS {
            assert!(find_entry(pair.variant).is_ok(), "{}", pair.variant);
            assert!(find_entry(pair.reference).is_ok(), "{}", pair.reference);
            assert_eq!(
                reference_of(pair.reference),
                None,
                "эталон {} не может быть вариантом",
                pair.reference
            );
        }
    }
}
