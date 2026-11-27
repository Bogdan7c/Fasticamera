//! Место замера `viewfinder-stage-budget` (сессия 16a): где стоит приложение, пока меряется ток.
//!
//! Раньше сценарий мерил только «голый» видоискатель: экран камеры на время сценария снят, рисуется один
//! видоискатель. Телефон же греется от всего продукта вместе (видоискатель + стекло UI + цветовые приборы), поэтому
//! цену каждой работы нужно мерить там, где она работает по-настоящему, — на продуктовом экране.
//!
//! Модуль владеет двумя вещами: списком мест ([`StageBudgetSurface`]) и таблицей «какая строка на каком месте
//! допустима» ([`row_allowed_on`]). Недопустимое сочетание — типизированная ошибка конфига ДО начала замера
//! ([`validate_rows_for_surface`]), а не тихий пропуск строки: шесть минут замера «цены приборов» на экране, где
//! приборов нет, дали бы ноль и ввели бы в заблуждение.

use serde::{Deserialize, Serialize};

use crate::viewfinder_scenario_config::{StageBudgetRowSetting, ViewfinderScenarioConfigError};

/// Место замера.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageBudgetSurface {
    /// Экран камеры снят, рисуется один видоискатель (поведение сценария до 16a).
    #[default]
    BareViewfinder,
    /// Продуктовый экран камеры остаётся на месте, режим PHOTO.
    PhotoScreen,
    /// Продуктовый экран остаётся на месте и открыт раздел CALIBRATE: живой видоискатель, цветовой прибор на каждом
    /// кадре, стекло UI.
    CalibrateScreen,
}

impl StageBudgetSurface {
    /// Имя для отчёта и для intent-параметра `surface`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::BareViewfinder => "bare_viewfinder",
            Self::PhotoScreen => "photo_screen",
            Self::CalibrateScreen => "calibrate_screen",
        }
    }

    /// Остаётся ли продуктовый экран камеры на месте на время замера.
    pub const fn keeps_product_screen(self) -> bool {
        !matches!(self, Self::BareViewfinder)
    }

    /// Должен ли на этом месте считаться цветовой прибор (раздел калибровки открыт).
    pub const fn runs_color_scopes(self) -> bool {
        matches!(self, Self::CalibrateScreen)
    }

    /// Рисуется ли на этом месте UI экрана поверх видоискателя.
    pub const fn draws_ui_overlay(self) -> bool {
        self.keeps_product_screen()
    }
}

/// Допустима ли строка на месте.
pub fn row_allowed_on(row: StageBudgetRowSetting, surface: StageBudgetSurface) -> bool {
    match row {
        // Старые строки меряют работу видоискателя и годятся где угодно.
        StageBudgetRowSetting::Stage(_) | StageBudgetRowSetting::WholeViewfinder => true,
        // Частота экрана от места не зависит.
        StageBudgetRowSetting::Display120Hz => true,
        // Вид (14o) считается шейдером видоискателя и годится на всех трёх местах.
        StageBudgetRowSetting::Look(_) => true,
        // Приборы есть только в разделе калибровки.
        StageBudgetRowSetting::ColorScopes => surface == StageBudgetSurface::CalibrateScreen,
        // Рисование UI есть только там, где стоит экран камеры.
        StageBudgetRowSetting::UiOverlay | StageBudgetRowSetting::WholeProduct => surface.keeps_product_screen(),
    }
}

/// Где строка допустима, словами (для текста ошибки).
fn allowed_places(row: StageBudgetRowSetting) -> &'static str {
    match row {
        StageBudgetRowSetting::ColorScopes => "calibrate_screen",
        StageBudgetRowSetting::UiOverlay | StageBudgetRowSetting::WholeProduct => "photo_screen, calibrate_screen",
        _ => "bare_viewfinder, photo_screen, calibrate_screen",
    }
}

/// Все строки допустимы на выбранном месте; первая недопустимая — типизированная ошибка.
pub fn validate_rows_for_surface(
    rows: &[StageBudgetRowSetting],
    surface: StageBudgetSurface,
) -> Result<(), ViewfinderScenarioConfigError> {
    match rows.iter().find(|row| !row_allowed_on(**row, surface)) {
        Some(row) => Err(ViewfinderScenarioConfigError::RowNotAllowedOnSurface {
            row: row.name(),
            surface: surface.name(),
            allowed: allowed_places(*row),
        }),
        None => Ok(()),
    }
}

#[cfg(test)]
#[path = "stage_budget_surface_config_tests.rs"]
mod tests;
