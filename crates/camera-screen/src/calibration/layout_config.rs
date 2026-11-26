//! Числа раскладки раздела калибровки: форма зон каждой ориентации, сторона колонки ручек, нижний ряд и полоска
//! прибора. Все числа — `[estimate]` (решение 202: «доли — конфиг»), а не измерение Apple: в эталоне нет кадра
//! раздела калибровки (правило 96). Размеры самих виджетов — `CalibrationWidgetsConfig`.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Как раздел режет глазной прямоугольник на зоны (решения 255, 260). Формулы раскладки выбирают ветку по этому виду,
/// а не по тому, «портрет это или альбом»: вид — данные конфига каждой ориентации.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoneShape {
    /// Прибор — полоса на ВСЮ ширину под ручками и чистым превью (портрет, решение 255): ручки и превью делят верх
    /// пополам, прибор лежит под обоими.
    ScopeStripBelow,
    /// Прибор — под чистым превью, а колонка ручек стоит на всю высоту рядом (альбом, решение 260).
    ScopeUnderPreview,
}

/// С какой стороны стоит колонка ручек (решение 275: слева по умолчанию, сторона — конфиг).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnobsSide {
    /// Колонка у левого края безопасной зоны, чистое превью — справа.
    Left,
    /// Колонка у правого края безопасной зоны, чистое превью — слева.
    Right,
}

/// Где в этой ориентации стоит линия капсулы режимов, а с ней кнопки меню и CLEAN (решения 256, 259).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapsuleLine {
    /// Как у PHOTO: опорная высота центра на эталонном экране плюс сдвиг привязки к низу (`ScreenLayout::
    /// bottom_anchor_shift_pt`, решение 82). Своё число раздела, а не ссылка на числа PHOTO: PHOTO меняют отдельно.
    ReferenceAnchoredToBottom { reference_center_y_pt: f64 },
    /// Середина по высоте нижнего ряда.
    BottomRowCentre,
}

/// Числа одной ориентации.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrientationLayoutConfig {
    /// Форма зон.
    pub shape: ZoneShape,
    /// Доля ширины экрана под колонку ручек (0..1, не включая края).
    pub knobs_width_fraction: f64,
    /// Доля высоты над нижним рядом, где кончается чистое превью (в `ScopeStripBelow` и колонка ручек); ниже
    /// прибор. Это линия владельца (решение 255) — её место в конфиге, а не в формуле.
    pub preview_height_fraction: f64,
    /// Высота нижнего ряда, pt.
    pub bottom_row_height_pt: f64,
    /// Где стоит линия капсулы режимов.
    pub capsule_line: CapsuleLine,
}

/// Числа раскладки раздела.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CalibrationLayoutConfig {
    /// Портрет: прибор полосой на всю ширину внизу (решение 255). Ручки и превью делят верх поровну; линия «верх /
    /// прибор» стоит там, где её было видно до 14n (≈ 493,8 pt на 440 × 982,67) `[estimate]`.
    pub portrait: OrientationLayoutConfig,
    /// Альбом (решения 202, 260): колонка ручек ≈ треть ширины, правая колонка поровну между превью и прибором;
    /// нижний ряд компактный, потому что экран по высоте всего 440 pt `[estimate]`.
    pub landscape: OrientationLayoutConfig,
    /// Сторона колонки ручек для обеих ориентаций.
    pub knobs_side: KnobsSide,
    /// Высота полоски прибора (имя, точки страниц, оси, белое), pt (решение 291: 44, в ней живые цели).
    pub scope_strip_height_pt: f64,
    /// Ширина поля уровней слева от картинки waveform и parade, pt (решение 274: ≈ 28).
    pub scope_levels_gutter_pt: f64,
    /// Отступ центров кнопки меню и CLEAN от боковых краёв безопасной зоны, pt: как у кнопки меню PHOTO от края
    /// экрана (440 − 383,6 = 56,4), чтобы кнопка не оказалась в полосе системного жеста.
    pub side_button_inset_pt: f64,
}

impl Default for CalibrationLayoutConfig {
    fn default() -> Self {
        Self {
            portrait: OrientationLayoutConfig {
                shape: ZoneShape::ScopeStripBelow,
                knobs_width_fraction: 0.5,
                preview_height_fraction: 0.5646,
                bottom_row_height_pt: 108.0,
                capsule_line: CapsuleLine::ReferenceAnchoredToBottom {
                    reference_center_y_pt: 898.0,
                },
            },
            landscape: OrientationLayoutConfig {
                shape: ZoneShape::ScopeUnderPreview,
                knobs_width_fraction: 1.0 / 3.0,
                preview_height_fraction: 0.5,
                bottom_row_height_pt: 64.0,
                capsule_line: CapsuleLine::BottomRowCentre,
            },
            knobs_side: KnobsSide::Left,
            scope_strip_height_pt: 44.0,
            scope_levels_gutter_pt: 28.0,
            side_button_inset_pt: 56.4,
        }
    }
}

/// Ошибка чисел раскладки.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum CalibrationLayoutConfigError {
    #[error("доля {field} должна быть числом строго между 0 и 1: {value}")]
    FractionOutOfRange { field: &'static str, value: f64 },
    #[error("размер раскладки {field} должен быть конечным и больше нуля: {value}")]
    NonPositive { field: &'static str, value: f64 },
    #[error("опорная высота линии капсулы {field} должна быть конечной: {value}")]
    NonFinite { field: &'static str, value: f64 },
}

impl CalibrationLayoutConfig {
    /// Проверка значений (не зависит от размера surface).
    pub fn validate(&self) -> Result<(), CalibrationLayoutConfigError> {
        // Имена полей — для ошибки: по ним видно, какое именно число конфига плохое.
        let orientations = [
            (
                &self.portrait,
                ["portrait.knobs_width_fraction", "portrait.preview_height_fraction"],
                "portrait.bottom_row_height_pt",
            ),
            (
                &self.landscape,
                ["landscape.knobs_width_fraction", "landscape.preview_height_fraction"],
                "landscape.bottom_row_height_pt",
            ),
        ];
        for (orientation, [width_field, height_field], row_field) in orientations {
            for (field, value) in [
                (width_field, orientation.knobs_width_fraction),
                (height_field, orientation.preview_height_fraction),
            ] {
                if !(value.is_finite() && value > 0.0 && value < 1.0) {
                    return Err(CalibrationLayoutConfigError::FractionOutOfRange { field, value });
                }
            }
            if !(orientation.bottom_row_height_pt.is_finite() && orientation.bottom_row_height_pt > 0.0) {
                return Err(CalibrationLayoutConfigError::NonPositive {
                    field: row_field,
                    value: orientation.bottom_row_height_pt,
                });
            }
            if let CapsuleLine::ReferenceAnchoredToBottom { reference_center_y_pt } = orientation.capsule_line
                && !reference_center_y_pt.is_finite()
            {
                return Err(CalibrationLayoutConfigError::NonFinite {
                    field: "capsule_line.reference_center_y_pt",
                    value: reference_center_y_pt,
                });
            }
        }
        for (field, value) in [
            ("scope_strip_height_pt", self.scope_strip_height_pt),
            ("scope_levels_gutter_pt", self.scope_levels_gutter_pt),
            ("side_button_inset_pt", self.side_button_inset_pt),
        ] {
            if !(value.is_finite() && value > 0.0) {
                return Err(CalibrationLayoutConfigError::NonPositive { field, value });
            }
        }
        Ok(())
    }
}
