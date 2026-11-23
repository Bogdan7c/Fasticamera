//! Числа ВИДА раздела калибровки: размеры колонки ручек, строк, кнопок, точек полос и прибора.
//!
//! Все числа здесь — `[invented]`/`[estimate]`: в эталоне Apple нет раздела калибровки (правило 96), поэтому
//! это подгонка под телефон, а не измерение. Единственное заимствованное из измеренного — вид кнопки «↻»
//! (диск и затемнение до 44 %, `[measured: C t=54.0]`): он берётся из `widgets.card`, а не дублируется здесь.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Сколько таблеток ряда можно завести: номер таблетки идёт в `ButtonId(u8)`, поэтому больше 255 не бывает.
pub const MAX_CHIP_POOL: usize = 255;

/// Числа вида раздела калибровки (pt, если не сказано иное).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CalibrationWidgetsConfig {
    /// Радиус скругления стеклянных панелей (колонка ручек, прибор). `[invented]`.
    pub panel_corner_radius_pt: f32,
    /// Отступ содержимого колонки от краёв панели. `[invented]`.
    pub panel_padding_pt: f64,
    /// Высота шапки колонки: переключатель основы и кнопка «чистяк». `[invented]`.
    pub header_height_pt: f64,
    /// Высота кнопки «чистяк» (зона касания расширяется до 44 pt отдельно). `[invented]`.
    pub clean_button_height_pt: f64,
    /// Ширина кнопки «чистяк»: капсула с подписью CLEAN (значка «глаз» в наборе нет). `[invented]`.
    pub clean_button_width_pt: f64,
    /// Прозрачность заливки кнопки «чистяк» в покое. `[invented]`.
    pub clean_button_fill_alpha: f32,
    /// Прозрачность заливки кнопки «чистяк», пока её держат. `[invented]`.
    pub clean_button_held_fill_alpha: f32,
    /// Промежуток между элементами шапки (переключатель основы, «↻», «чистяк») и между значением и «↻». `[invented]`.
    pub header_gap_pt: f64,
    /// Промежуток между значением и «↻» в строке ручки. `[invented]`.
    pub value_gap_pt: f64,
    /// Высота заголовка группы (TONE / COLOR / BANDS). `[invented]`.
    pub group_header_height_pt: f64,
    /// Высота строки ручки целиком: заголовок со значением и «↻», под ними линейка. `[invented]`.
    pub row_height_pt: f64,
    /// Высота верхней линии строки (заголовок, значение, «↻»). `[invented]`.
    pub row_title_height_pt: f64,
    /// Высота строки переключателя вида SCALES | WHEELS под шапкой (она же зона касания сегментов). `[invented]`.
    pub view_switcher_height_pt: f64,
    /// Высота капсулы сегмента переключателя вида и вкладки колеса (строка выше на зазор до зоны касания). `[invented]`.
    pub segment_height_pt: f64,
    /// Промежуток между сегментами переключателя вида и между вкладками колёс. `[invented]`.
    pub segment_gap_pt: f64,
    /// Высота строки вкладок LIFT · GAMMA · GAIN · OFFSET. `[invented]`.
    pub wheel_tabs_height_pt: f64,
    /// Высота строки «название колеса и ↻ колеса целиком». `[invented]`.
    pub wheel_title_height_pt: f64,
    /// Высота линейки кольца яркости (под строкой с названием и значением, как у остальных ручек). `[invented]`.
    pub wheel_ring_ruler_height_pt: f64,
    /// Вертикальный промежуток между блоками вида колёс (вкладки, название, круг, шкала кольца). `[invented]`.
    pub wheel_block_gap_pt: f64,
    /// Наибольший диаметр круга колеса (решение 205: ≈ 170 pt портрет); меньше — по месту. `[estimate]`.
    pub wheel_circle_max_diameter_pt: f64,
    /// Наименьший диаметр круга: меньше круг не строится (раздел отказывает сборкой, а не рисует крошку). `[estimate]`.
    pub wheel_circle_min_diameter_pt: f64,
    /// Толщина цветного кольца оттенков вокруг круга толчка. `[estimate]`.
    pub wheel_ring_thickness_pt: f64,
    /// Прозрачность тёмной подложки круга толчка (чёрная заливка). `[estimate]`.
    pub wheel_pad_fill_alpha: f32,
    /// Диаметр метки-кольца «колесо правлено» на вкладке. `[invented]`.
    pub wheel_tab_marker_diameter_pt: f64,
    /// Толщина метки-кольца вкладки. `[invented]`.
    pub wheel_tab_marker_width_pt: f64,
    /// Диаметр кнопки «↻» в строке. `[invented]`.
    pub reset_button_diameter_pt: f64,
    /// Высота ряда цветных точек полос. `[invented]`.
    pub band_dots_height_pt: f64,
    /// Диаметр цветной точки полосы. `[invented]`.
    pub band_dot_diameter_pt: f64,
    /// Промежуток между точками полос. `[invented]`.
    pub band_dot_gap_pt: f64,
    /// Толщина кольца вокруг выбранной точки. `[invented]`.
    pub band_ring_width_pt: f64,
    /// Зазор между точкой и её кольцом. `[invented]`.
    pub band_ring_gap_pt: f64,
    /// Диаметр метки «полоса отличается от пресета» под точкой. `[invented]`.
    pub band_marker_diameter_pt: f64,
    /// Шаг ручки (доля диапазона [-1, 1]). `[estimate]` 0,1: 21 риска на весь ход.
    pub knob_step: f64,
    /// Каждая какая риска крупная. `[estimate]` 5 (то есть 0,5).
    pub knob_major_every_steps: u32,
    /// На сколько шагов линейка может разойтись с фактами, не двигаясь к ним. `[estimate]`.
    pub follow_tolerance_steps: f64,
    /// Прозрачность недоступной ручки (основа её не умеет). `[invented]`.
    pub disabled_row_opacity: f32,
    /// Диаметр точки-страницы под прибором. `[invented]`.
    pub scope_page_dot_diameter_pt: f64,
    /// Промежуток между точками-страницами. `[invented]`.
    pub scope_page_dot_gap_pt: f64,
    /// Отступ подписи прибора и точек от краёв зоны прибора. `[invented]`.
    pub scope_label_inset_pt: f64,
    /// Минимальный сдвиг пальца по горизонтали, чтобы свайп сменил прибор. `[estimate]`.
    pub scope_swipe_min_pt: f64,
    /// Запас пилюли режима по бокам подписи в разделе. `[estimate]`.
    pub mode_pill_padding_pt: f64,
    /// Высота «таблетки» в ряду имён пресетов и в ряду действий (NEW / RENAME / SAVE / DELETE); общая для карточки
    /// Styles и карточек меню раздела. `[estimate]` (зона касания расширяется до 44 pt отдельно).
    pub chip_height_pt: f64,
    /// Поля таблетки слева и справа от подписи. `[estimate]`.
    pub chip_padding_pt: f64,
    /// Промежуток между соседними таблетками ряда. `[estimate]`.
    pub chip_gap_pt: f64,
    /// Прозрачность заливки таблетки (белая поверх стекла карточки). `[estimate]`.
    pub chip_fill_alpha: f32,
    /// Сколько таблеток ряд умеет держать (узлы сцены заводятся заранее). Больше имён — лишние не показываются, их
    /// число идёт в счётчик переполнения диагностики, а не теряется молча. `[estimate]`.
    pub chip_pool_size: usize,
    /// Запас под подписями пунктов корня меню раздела: низ шторки отстоит от низа подписей на столько. `[estimate]`.
    pub menu_sheet_bottom_padding_pt: f64,
    /// Сколько секунд висит плашка проблемы, пока её не закроют. `[estimate]`.
    pub problem_banner_seconds: f64,
    /// Зазор между плашкой проблемы и тем, к чему она прижата (плашка AE/AF LOCK, нижний ряд раздела). `[estimate]`.
    pub problem_banner_gap_pt: f64,
}

impl Default for CalibrationWidgetsConfig {
    fn default() -> Self {
        Self {
            panel_corner_radius_pt: 26.0,
            panel_padding_pt: 12.0,
            header_height_pt: 56.0,
            clean_button_height_pt: 32.0,
            clean_button_width_pt: 64.0,
            clean_button_fill_alpha: 0.16,
            clean_button_held_fill_alpha: 0.4,
            header_gap_pt: 8.0,
            value_gap_pt: 8.0,
            group_header_height_pt: 30.0,
            row_height_pt: 68.0,
            row_title_height_pt: 28.0,
            view_switcher_height_pt: 36.0,
            segment_height_pt: 28.0,
            segment_gap_pt: 4.0,
            wheel_tabs_height_pt: 36.0,
            wheel_title_height_pt: 26.0,
            wheel_ring_ruler_height_pt: 32.0,
            wheel_block_gap_pt: 4.0,
            wheel_circle_max_diameter_pt: 170.0,
            wheel_circle_min_diameter_pt: 48.0,
            wheel_ring_thickness_pt: 10.0,
            wheel_pad_fill_alpha: 0.35,
            wheel_tab_marker_diameter_pt: 7.0,
            wheel_tab_marker_width_pt: 1.5,
            reset_button_diameter_pt: 26.0,
            band_dots_height_pt: 44.0,
            band_dot_diameter_pt: 24.0,
            band_dot_gap_pt: 10.0,
            band_ring_width_pt: 2.0,
            band_ring_gap_pt: 3.0,
            band_marker_diameter_pt: 5.0,
            knob_step: 0.1,
            knob_major_every_steps: 5,
            follow_tolerance_steps: 0.01,
            disabled_row_opacity: 0.35,
            scope_page_dot_diameter_pt: 6.0,
            scope_page_dot_gap_pt: 8.0,
            scope_label_inset_pt: 10.0,
            scope_swipe_min_pt: 40.0,
            mode_pill_padding_pt: 20.0,
            chip_height_pt: 32.0,
            chip_padding_pt: 16.0,
            chip_gap_pt: 8.0,
            chip_fill_alpha: 0.16,
            chip_pool_size: 16,
            menu_sheet_bottom_padding_pt: 18.0,
            problem_banner_seconds: 3.0,
            problem_banner_gap_pt: 8.0,
        }
    }
}

/// Ошибка чисел вида раздела.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum CalibrationWidgetsConfigError {
    #[error("размер {field} должен быть конечным и больше нуля: {value}")]
    NonPositive { field: &'static str, value: f64 },
    #[error("доля {field} должна быть конечной и в 0..=1: {value}")]
    FractionOutOfRange { field: &'static str, value: f64 },
    #[error("шаг ручки {value} должен быть в (0, 1]")]
    KnobStepOutOfRange { value: f64 },
    #[error("период крупной риски должен быть не меньше 1")]
    ZeroMajorPeriod,
    #[error("в ряду таблеток от 1 до {MAX_CHIP_POOL}: {value}")]
    ChipPoolOutOfRange { value: usize },
    #[error(
        "круг колеса: наименьший диаметр {min_pt} больше наибольшего {max_pt}, либо кольцо {thickness_pt} не уже радиуса"
    )]
    WheelCircleRange {
        min_pt: f64,
        max_pt: f64,
        thickness_pt: f64,
    },
    #[error("строка ({row_height_pt} pt) не вмещает верхнюю линию ({title_height_pt} pt) и линейку")]
    RowTooShort { row_height_pt: f64, title_height_pt: f64 },
}

impl CalibrationWidgetsConfig {
    /// Проверка значений.
    pub fn validate(&self) -> Result<(), CalibrationWidgetsConfigError> {
        let sizes = [
            ("panel_corner_radius_pt", f64::from(self.panel_corner_radius_pt)),
            ("panel_padding_pt", self.panel_padding_pt),
            ("header_height_pt", self.header_height_pt),
            ("clean_button_height_pt", self.clean_button_height_pt),
            ("clean_button_width_pt", self.clean_button_width_pt),
            ("header_gap_pt", self.header_gap_pt),
            ("value_gap_pt", self.value_gap_pt),
            ("group_header_height_pt", self.group_header_height_pt),
            ("row_height_pt", self.row_height_pt),
            ("row_title_height_pt", self.row_title_height_pt),
            ("reset_button_diameter_pt", self.reset_button_diameter_pt),
            ("view_switcher_height_pt", self.view_switcher_height_pt),
            ("segment_height_pt", self.segment_height_pt),
            ("segment_gap_pt", self.segment_gap_pt),
            ("wheel_tabs_height_pt", self.wheel_tabs_height_pt),
            ("wheel_title_height_pt", self.wheel_title_height_pt),
            ("wheel_ring_ruler_height_pt", self.wheel_ring_ruler_height_pt),
            ("wheel_block_gap_pt", self.wheel_block_gap_pt),
            ("wheel_circle_max_diameter_pt", self.wheel_circle_max_diameter_pt),
            ("wheel_circle_min_diameter_pt", self.wheel_circle_min_diameter_pt),
            ("wheel_ring_thickness_pt", self.wheel_ring_thickness_pt),
            ("wheel_tab_marker_diameter_pt", self.wheel_tab_marker_diameter_pt),
            ("wheel_tab_marker_width_pt", self.wheel_tab_marker_width_pt),
            ("band_dots_height_pt", self.band_dots_height_pt),
            ("band_dot_diameter_pt", self.band_dot_diameter_pt),
            ("band_dot_gap_pt", self.band_dot_gap_pt),
            ("band_ring_width_pt", self.band_ring_width_pt),
            ("band_ring_gap_pt", self.band_ring_gap_pt),
            ("band_marker_diameter_pt", self.band_marker_diameter_pt),
            ("follow_tolerance_steps", self.follow_tolerance_steps),
            ("scope_page_dot_diameter_pt", self.scope_page_dot_diameter_pt),
            ("scope_page_dot_gap_pt", self.scope_page_dot_gap_pt),
            ("scope_label_inset_pt", self.scope_label_inset_pt),
            ("scope_swipe_min_pt", self.scope_swipe_min_pt),
            ("mode_pill_padding_pt", self.mode_pill_padding_pt),
            ("chip_height_pt", self.chip_height_pt),
            ("chip_padding_pt", self.chip_padding_pt),
            ("chip_gap_pt", self.chip_gap_pt),
            ("menu_sheet_bottom_padding_pt", self.menu_sheet_bottom_padding_pt),
            ("problem_banner_seconds", self.problem_banner_seconds),
            ("problem_banner_gap_pt", self.problem_banner_gap_pt),
        ];
        for (field, value) in sizes {
            if !(value.is_finite() && value > 0.0) {
                return Err(CalibrationWidgetsConfigError::NonPositive { field, value });
            }
        }
        let fractions = [
            ("disabled_row_opacity", f64::from(self.disabled_row_opacity)),
            ("clean_button_fill_alpha", f64::from(self.clean_button_fill_alpha)),
            ("chip_fill_alpha", f64::from(self.chip_fill_alpha)),
            ("wheel_pad_fill_alpha", f64::from(self.wheel_pad_fill_alpha)),
            (
                "clean_button_held_fill_alpha",
                f64::from(self.clean_button_held_fill_alpha),
            ),
        ];
        for (field, value) in fractions {
            if !(value.is_finite() && (0.0..=1.0).contains(&value)) {
                return Err(CalibrationWidgetsConfigError::FractionOutOfRange { field, value });
            }
        }
        if !(self.knob_step.is_finite() && self.knob_step > 0.0 && self.knob_step <= 1.0) {
            return Err(CalibrationWidgetsConfigError::KnobStepOutOfRange { value: self.knob_step });
        }
        if self.knob_major_every_steps == 0 {
            return Err(CalibrationWidgetsConfigError::ZeroMajorPeriod);
        }
        if self.chip_pool_size == 0 || self.chip_pool_size > MAX_CHIP_POOL {
            return Err(CalibrationWidgetsConfigError::ChipPoolOutOfRange {
                value: self.chip_pool_size,
            });
        }
        if self.row_height_pt <= self.row_title_height_pt {
            return Err(CalibrationWidgetsConfigError::RowTooShort {
                row_height_pt: self.row_height_pt,
                title_height_pt: self.row_title_height_pt,
            });
        }
        if self.wheel_circle_min_diameter_pt > self.wheel_circle_max_diameter_pt
            || self.wheel_ring_thickness_pt >= self.wheel_circle_min_diameter_pt / 2.0
        {
            return Err(CalibrationWidgetsConfigError::WheelCircleRange {
                min_pt: self.wheel_circle_min_diameter_pt,
                max_pt: self.wheel_circle_max_diameter_pt,
                thickness_pt: self.wheel_ring_thickness_pt,
            });
        }
        Ok(())
    }
}
