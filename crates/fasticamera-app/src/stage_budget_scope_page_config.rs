//! Страница прибора для `viewfinder-stage-budget` на `calibrate_screen` (сессия 14s): настройка, свайпы листания и проверка.
//!
//! Раздел калибровки открывается на странице по умолчанию, а цена приборов у страниц разная. `stage_budget.scope_page`
//! (intent-параметр `scope_page`) называет страницу, чью цену меряет таблица; без неё меряется та страница, на которой
//! раздел открылся. Страницы есть только в разделе калибровки, поэтому на другом месте замера параметр — типизированная
//! ошибка конфига ДО замера (как недопустимая строка таблицы), а не молчаливо проигнорированная настройка.
//!
//! Листает страницы драйвер по просьбе сценария (`scenario/camera/stage_budget_scope_page.rs`): жесты — свайпы по полю
//! прибора, их места лежат здесь как данные плана касаний, а не в коде.

use scope_contract::ScopeKind;
use serde::{Deserialize, Serialize};

use crate::screen_scenario_config::{ScreenPoint, SwipeGesture};
use crate::stage_budget_surface_config::StageBudgetSurface;
use crate::viewfinder_scenario_config::ViewfinderScenarioConfigError;

/// Свайпы листания страниц прибора, доли экрана.
///
/// `[estimate]` по портретной раскладке раздела (поле прибора — правая нижняя часть правой колонки; свайп в верхней
/// «чистой» зоне раздела сменил бы режим экрана, а не страницу): y = 0,80, дальше 0,3 ширины. Телефоном ещё не проверено;
/// не туда попавший жест виден отказом входа «страница не сменилась» с названием страницы, а не неверной цифрой.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScopePageSwipes {
    /// Сдвиг глазами влево: к следующей странице порядка словаря.
    pub next: SwipeGesture,
    /// Сдвиг глазами вправо: к предыдущей странице.
    pub previous: SwipeGesture,
}

impl Default for ScopePageSwipes {
    fn default() -> Self {
        let swipe = |from_x: f64, to_x: f64| SwipeGesture {
            from: ScreenPoint::new(from_x, 0.80),
            to: ScreenPoint::new(to_x, 0.80),
            duration_ms: 300,
        };
        Self {
            next: swipe(0.85, 0.55),
            previous: swipe(0.55, 0.85),
        }
    }
}

/// Страница задана только там, где страницы есть; свайпы листания — точки в долях экрана и ненулевая длительность.
pub fn validate_scope_page(
    surface: StageBudgetSurface,
    scope_page: Option<ScopeKind>,
    swipes: &ScopePageSwipes,
) -> Result<(), ViewfinderScenarioConfigError> {
    if scope_page.is_some() && !surface.runs_color_scopes() {
        return Err(ViewfinderScenarioConfigError::ScopePageNeedsCalibrateScreen {
            surface: surface.name(),
        });
    }
    for (field, swipe) in [
        ("stage_budget.scope_page_swipes.next", swipes.next),
        ("stage_budget.scope_page_swipes.previous", swipes.previous),
    ] {
        for point in [swipe.from, swipe.to] {
            if !((0.0..=1.0).contains(&point.x) && (0.0..=1.0).contains(&point.y)) {
                return Err(ViewfinderScenarioConfigError::ScreenPointOutOfRange {
                    field,
                    x: point.x,
                    y: point.y,
                });
            }
        }
        if swipe.duration_ms == 0 {
            return Err(ViewfinderScenarioConfigError::Zero { field });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_accepted_only_on_the_calibrate_screen() {
        let swipes = ScopePageSwipes::default();
        for page in ScopeKind::ALL {
            assert_eq!(
                validate_scope_page(StageBudgetSurface::CalibrateScreen, Some(page), &swipes),
                Ok(())
            );
            for surface in [StageBudgetSurface::BareViewfinder, StageBudgetSurface::PhotoScreen] {
                assert_eq!(
                    validate_scope_page(surface, Some(page), &swipes),
                    Err(ViewfinderScenarioConfigError::ScopePageNeedsCalibrateScreen {
                        surface: surface.name()
                    }),
                    "{surface:?} / {page:?}"
                );
            }
        }
    }

    #[test]
    fn no_page_means_whatever_the_section_opens_on_and_is_valid_everywhere() {
        for surface in [
            StageBudgetSurface::BareViewfinder,
            StageBudgetSurface::PhotoScreen,
            StageBudgetSurface::CalibrateScreen,
        ] {
            assert_eq!(validate_scope_page(surface, None, &ScopePageSwipes::default()), Ok(()));
        }
    }

    #[test]
    fn page_swipes_must_stay_on_the_screen_and_take_time() {
        let off_screen = ScopePageSwipes {
            next: SwipeGesture {
                to: ScreenPoint::new(1.5, 0.8),
                ..ScopePageSwipes::default().next
            },
            ..ScopePageSwipes::default()
        };
        assert!(matches!(
            validate_scope_page(StageBudgetSurface::CalibrateScreen, None, &off_screen),
            Err(ViewfinderScenarioConfigError::ScreenPointOutOfRange {
                field: "stage_budget.scope_page_swipes.next",
                ..
            })
        ));
        let instant = ScopePageSwipes {
            previous: SwipeGesture {
                duration_ms: 0,
                ..ScopePageSwipes::default().previous
            },
            ..ScopePageSwipes::default()
        };
        assert_eq!(
            validate_scope_page(StageBudgetSurface::CalibrateScreen, None, &instant),
            Err(ViewfinderScenarioConfigError::Zero {
                field: "stage_budget.scope_page_swipes.previous"
            })
        );
    }

    #[test]
    fn the_default_swipes_go_in_opposite_directions_along_one_line() {
        let swipes = ScopePageSwipes::default();
        assert!(swipes.next.to.x < swipes.next.from.x, "к следующей — сдвиг влево");
        assert!(
            swipes.previous.to.x > swipes.previous.from.x,
            "к предыдущей — сдвиг вправо"
        );
        assert_eq!(swipes.next.from.y, swipes.next.to.y);
        assert_eq!(swipes.next.from.y, swipes.previous.from.y);
    }
}
