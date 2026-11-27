//! Выбор страницы цветового прибора для `viewfinder-stage-budget` на `calibrate_screen` (сессия 14s).
//!
//! Раздел калибровки открывается на странице по умолчанию, а цена приборов у страниц разная (стенд: гистограмма втрое
//! дешевле RGB WAVEFORM). Чтобы мерить ток именно нужной страницы, сценарий после входа в раздел просит у драйвера свайпы
//! листания (как `modes_swipe_*`: сам ничего не нажимает) и ждёт, пока рендер подтвердит запрос нужной страницы.
//!
//! Модуль чистый: решает только «что делать теперь» по порядку страниц словаря ([`ScopeKind::ALL`]) и по тому, какую
//! страницу экран показал последней; время и жесты — у [`super::stage_budget_surface::SurfaceEntry`].

use scope_contract::ScopeKind;

/// Куда листать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageDirection {
    /// К следующей странице порядка словаря (свайп глазами влево).
    Next,
    /// К предыдущей (свайп глазами вправо).
    Previous,
}

/// Что делать, чтобы страница `wanted` стала показанной.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageStep {
    /// Нужная страница уже показана.
    Reached,
    /// Листнуть в эту сторону на одну страницу.
    Swipe(PageDirection),
    /// Рендер ещё не сообщил, какая страница показана: листать вслепую нельзя.
    WaitForReport,
}

/// Шаг к странице `wanted` от показанной `shown` (`None` — приборы ещё не запрошены).
pub fn step_towards(wanted: ScopeKind, shown: Option<ScopeKind>) -> PageStep {
    let Some(shown) = shown else {
        return PageStep::WaitForReport;
    };
    if shown == wanted {
        return PageStep::Reached;
    }
    // Порядок страниц — порядок словаря; обе страницы в нём есть (`match` без `_` в словаре гарантирует список).
    let position = |kind: ScopeKind| ScopeKind::ALL.iter().position(|candidate| *candidate == kind);
    match (position(shown), position(wanted)) {
        (Some(from), Some(to)) if to > from => PageStep::Swipe(PageDirection::Next),
        (Some(_), Some(_)) => PageStep::Swipe(PageDirection::Previous),
        // Страницы вне порядка словаря не бывает; на всякий случай — ждать отчёта, а не листать наугад.
        _ => PageStep::WaitForReport,
    }
}

impl PageDirection {
    /// Имя шага для отчёта и поля `step` стадии жеста.
    pub const fn step_name(self) -> &'static str {
        match self {
            Self::Next => "scope_page_next",
            Self::Previous => "scope_page_previous",
        }
    }

    /// Зачем жест (по-русски, в отчёт).
    pub const fn purpose(self) -> &'static str {
        match self {
            Self::Next => "свайп сдвигом влево листает к следующей странице прибора: меряется нужная страница",
            Self::Previous => "свайп сдвигом вправо листает к предыдущей странице прибора: меряется нужная страница",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wanted_page_is_reached_without_a_swipe() {
        for kind in ScopeKind::ALL {
            assert_eq!(step_towards(kind, Some(kind)), PageStep::Reached, "{kind:?}");
        }
    }

    #[test]
    fn the_swipe_goes_toward_the_wanted_page_along_the_dictionary_order() {
        // Порядок: HISTOGRAM, WAVEFORM, LUMA, PARADE, VECTORSCOPE, CIE.
        assert_eq!(
            step_towards(ScopeKind::Histogram, Some(ScopeKind::Waveform)),
            PageStep::Swipe(PageDirection::Previous)
        );
        assert_eq!(
            step_towards(ScopeKind::RgbParade, Some(ScopeKind::Waveform)),
            PageStep::Swipe(PageDirection::Next)
        );
        assert_eq!(
            step_towards(ScopeKind::Waveform, Some(ScopeKind::Vectorscope)),
            PageStep::Swipe(PageDirection::Previous)
        );
        assert_eq!(
            step_towards(ScopeKind::Vectorscope, Some(ScopeKind::Histogram)),
            PageStep::Swipe(PageDirection::Next)
        );
    }

    #[test]
    fn the_cie_page_is_reached_from_the_opening_histogram_and_the_default_opening_page_is_the_histogram() {
        // Раздел открывается на HISTOGRAM (решение 335); до CIE — пять свайпов вперёд по порядку словаря, обратно — назад.
        let opening = camera_screen::calibration::CalibrationConfig::default().opening_scope_page;
        assert_eq!(opening, ScopeKind::Histogram);
        assert_eq!(
            step_towards(ScopeKind::Cie, Some(opening)),
            PageStep::Swipe(PageDirection::Next)
        );
        assert_eq!(
            step_towards(ScopeKind::Waveform, Some(ScopeKind::Cie)),
            PageStep::Swipe(PageDirection::Previous)
        );
        let mut shown = opening;
        let mut swipes = 0;
        while let PageStep::Swipe(PageDirection::Next) = step_towards(ScopeKind::Cie, Some(shown)) {
            let index = ScopeKind::ALL.iter().position(|kind| *kind == shown).unwrap();
            shown = ScopeKind::ALL[index + 1];
            swipes += 1;
        }
        assert_eq!((shown, swipes), (ScopeKind::Cie, 5));
    }

    #[test]
    fn following_the_swipes_visits_every_page_in_at_most_the_distance_between_them() {
        for from in ScopeKind::ALL {
            for to in ScopeKind::ALL {
                let mut shown = from;
                let mut swipes = 0;
                while let PageStep::Swipe(direction) = step_towards(to, Some(shown)) {
                    let index = ScopeKind::ALL.iter().position(|kind| *kind == shown).unwrap();
                    shown = match direction {
                        PageDirection::Next => ScopeKind::ALL[index + 1],
                        PageDirection::Previous => ScopeKind::ALL[index - 1],
                    };
                    swipes += 1;
                    assert!(swipes <= ScopeKind::ALL.len(), "{from:?} → {to:?} не сходится");
                }
                assert_eq!(shown, to, "{from:?} → {to:?}");
            }
        }
    }

    #[test]
    fn without_a_report_the_pages_are_not_swiped_blindly() {
        assert_eq!(step_towards(ScopeKind::Histogram, None), PageStep::WaitForReport);
    }

    #[test]
    fn step_names_are_distinct_driver_contract_names() {
        assert_eq!(PageDirection::Next.step_name(), "scope_page_next");
        assert_eq!(PageDirection::Previous.step_name(), "scope_page_previous");
        assert_ne!(PageDirection::Next.purpose(), PageDirection::Previous.purpose());
    }
}
