//! Числа раскладки и вида виджетов экрана Photo. Координаты — **эталонного iPhone** (pt, ось y вниз от верха
//! экрана высотой 956 pt); привязку к низу (решение 82) делает [`crate::widget::geometry::ScreenGeometry`].
//!
//! Метки происхождения: `[measured]` — измерено по видео (`layout.md` 2.2), `[derived]` — выведено из
//! измеренного, `[doc]` — из документации платформы, `[estimate]` — подгонка, не Apple.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ui_draw::color::DisplayColor;

/// Ошибка значения раскладки виджетов.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum WidgetConfigError {
    /// Число должно быть конечным и больше нуля.
    #[error("значение раскладки виджетов {field} должно быть конечным и > 0, получено {value}")]
    NotPositive { field: &'static str, value: f64 },
    /// Число должно быть конечным (знак любой, например угол).
    #[error("значение раскладки виджетов {field} должно быть конечным, получено {value}")]
    NotFinite { field: &'static str, value: f64 },
    /// Число должно быть конечным и лежать в 0..=1.
    #[error("значение раскладки виджетов {field} должно лежать в 0..=1, получено {value}")]
    NotFraction { field: &'static str, value: f64 },
    /// В сетке меню нужна хотя бы одна колонка.
    #[error("в сетке меню нужна хотя бы одна колонка")]
    NoMenuColumns,
}

/// Ряд значков над кадром.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct StripLayout {
    /// Центр ряда по x. `[measured]` 220,0 (центральный значок, `layout.md` 2.2).
    pub center_x_pt: f64,
    /// Центр по y. `[measured]` 104,9 ± 1,3.
    pub center_y_pt: f64,
    /// Шаг центров значков. `[derived]` (272,6 − 169,5) / 2 = 51,55 из трёх измеренных центров.
    pub pitch_pt: f64,
    /// Сторона квадрата иконки. `[estimate]` 24 — подгонка, не Apple.
    pub icon_size_pt: f64,
    /// Сторона зоны касания. `[doc]` 44: минимальная цель касания HIG.
    pub touch_size_pt: f64,
    /// Сдвиг подписи значения (секунды таймера, EV) вниз от центра значка. `[estimate]` подгонка, не Apple.
    pub value_below_icon_pt: f64,
    /// Наклон зачёркивающей линии у выключенного RAW, радианы (≈ −35°). `[estimate]` подгонка, не Apple.
    pub raw_strike_angle_rad: f32,
    /// Длина зачёркивающей линии как доля ширины подписи RAW (линия чуть выступает за буквы). `[estimate]` 1,15.
    pub raw_strike_length_scale: f32,
}

impl Default for StripLayout {
    fn default() -> Self {
        Self {
            center_x_pt: 220.0,
            center_y_pt: 104.9,
            pitch_pt: 51.55,
            icon_size_pt: 24.0,
            touch_size_pt: 44.0,
            value_below_icon_pt: 22.0,
            raw_strike_angle_rad: -0.6,
            raw_strike_length_scale: 1.15,
        }
    }
}

/// Центр и диаметр круглой кнопки.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundButtonLayout {
    pub center_x_pt: f64,
    pub center_y_pt: f64,
    pub diameter_pt: f64,
}

/// Нижняя зона: затвор, миниатюра, меню, капсула режимов.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BottomLayout {
    /// Центр затвора по x. `[measured]` 220,0. Диаметр — токен `ShutterTokens.outer_diameter_pt`.
    pub shutter_center_x_pt: f64,
    /// Центр затвора по y. `[measured]` 801,9.
    pub shutter_center_y_pt: f64,
    /// Кнопка меню Controls. `[measured]` центр (381,7; 801,7), Ø 48,3.
    pub menu_button: RoundButtonLayout,
    /// Миниатюра. `[measured]` центр (57,8; 897,4), Ø 48,0.
    pub thumbnail: RoundButtonLayout,
    /// Центр капсулы режимов по x. `[measured]` 220,0.
    pub capsule_center_x_pt: f64,
    /// Центр капсулы по y. `[measured]` 897,8.
    pub capsule_center_y_pt: f64,
    /// Размер капсулы. `[measured]` 220,0 × 47,8.
    pub capsule_width_pt: f64,
    pub capsule_height_pt: f64,
    /// Сторона иконки кнопки меню. `[estimate]` 24 — подгонка, не Apple.
    pub button_icon_size_pt: f64,
    /// Диаметр диска «снимок есть» внутри миниатюры как доля диаметра круга. `[estimate]` 0,86 — подгонка, не Apple.
    pub thumbnail_dot_scale: f32,
}

impl Default for BottomLayout {
    fn default() -> Self {
        Self {
            shutter_center_x_pt: 220.0,
            shutter_center_y_pt: 801.9,
            menu_button: RoundButtonLayout {
                center_x_pt: 381.7,
                center_y_pt: 801.7,
                diameter_pt: 48.3,
            },
            thumbnail: RoundButtonLayout {
                center_x_pt: 57.8,
                center_y_pt: 897.4,
                diameter_pt: 48.0,
            },
            capsule_center_x_pt: 220.0,
            capsule_center_y_pt: 897.8,
            capsule_width_pt: 220.0,
            capsule_height_pt: 47.8,
            button_icon_size_pt: 24.0,
            thumbnail_dot_scale: 0.86,
        }
    }
}

/// Меню Controls и его подменю.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SheetLayout {
    /// Размер шторки. `[estimate]` 382 × 416 (`layout.md` 2.2, видео B без масштаба).
    pub width_pt: f64,
    pub height_pt: f64,
    /// Отступ низа шторки от низа экрана. `[estimate]` 34,1 — как у капсулы режимов; подгонка, не Apple.
    pub bottom_inset_pt: f64,
    /// Радиус шторки. `[derived]` 39,1 — скругление экрана (`layout.md` 4.4).
    pub corner_radius_pt: f64,
    /// Диаметр пункта. `[estimate]` 68.
    pub item_diameter_pt: f64,
    /// Шаг пунктов по сетке. `[estimate]` 115.
    pub item_pitch_pt: f64,
    /// Число колонок. `[estimate]` 3.
    pub columns: u8,
    /// Отступ верха сетки от верха шторки. `[estimate]` 48 (место под «Edit» из iOS, у нас пустое).
    pub grid_top_inset_pt: f64,
    /// Сторона иконки пункта. `[estimate]` 30.
    pub item_icon_size_pt: f64,
    /// Расстояние от нижнего края круга до центра подписи. `[estimate]` 20.
    pub caption_below_circle_pt: f64,
    /// Прозрачность заливки круга пункта (поверх стекла шторки, не стекло на стекле). `[estimate]` 0,16.
    pub item_fill_alpha: f32,
    /// Высота строки подменю. `[estimate]` 56.
    pub option_row_height_pt: f64,
    /// Промежуток между строками. `[estimate]` 10.
    pub option_row_gap_pt: f64,
    /// Боковой отступ строк. `[estimate]` 24.
    pub option_side_inset_pt: f64,
    /// Отступ первой строки от верха шторки. `[estimate]` 72 (под кнопку «назад»).
    pub option_top_inset_pt: f64,
    /// Диаметр кнопки «назад» и отступ её центра от угла шторки. `[estimate]` 40 и 36.
    pub back_diameter_pt: f64,
    pub back_inset_pt: f64,
    /// Сторона стрелки «назад» как доля диаметра кнопки. `[estimate]` 0,5 — подгонка, не Apple.
    pub back_icon_scale: f64,
}

impl Default for SheetLayout {
    fn default() -> Self {
        Self {
            width_pt: 382.0,
            height_pt: 416.0,
            bottom_inset_pt: 34.1,
            corner_radius_pt: 39.1,
            item_diameter_pt: 68.0,
            item_pitch_pt: 115.0,
            columns: 3,
            grid_top_inset_pt: 48.0,
            item_icon_size_pt: 30.0,
            caption_below_circle_pt: 20.0,
            item_fill_alpha: 0.16,
            option_row_height_pt: 56.0,
            option_row_gap_pt: 10.0,
            option_side_inset_pt: 24.0,
            option_top_inset_pt: 72.0,
            back_diameter_pt: 40.0,
            back_inset_pt: 36.0,
            back_icon_scale: 0.5,
        }
    }
}

/// Панель Exposure на месте капсулы режимов.
///
/// Панель обязана целиком помещаться в полосе между низом кольца затвора и нижним отступом экрана: затвор при
/// открытой панели остаётся видимым и нажимаемым (решение 94 прячет его только при меню). В координатах
/// эталонного iPhone кольцо затвора кончается на y ≈ 841,9, низ панели — на 956 − 34,1 = 921,9: свободно ≈ 80 pt.
/// Панель — одна строка: слева вертикальная подпись EXPOSURE и сброс, по центру число над шкалой, справа «×».
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ExposurePanelLayout {
    /// Размер панели. `[estimate]` 400 × 72 (`motion.md` 4.1: растёт на месте капсулы; размер не измерен).
    /// Высота 72 из полосы ≈ 80 pt оставляет зазор ≈ 8 pt до кольца затвора; ширина — экран 440 с отступами по 20.
    pub width_pt: f64,
    pub height_pt: f64,
    /// Отступ низа панели от низа экрана. `[derived]` 34,1 — тот же, что у капсулы.
    pub bottom_inset_pt: f64,
    /// Радиус скругления. `[derived]` половина высоты — панель стоит капсулой, как капсула режимов.
    pub corner_radius_pt: f64,
    /// Центр вертикальной подписи EXPOSURE от левого края панели. `[estimate]` 18 (`behavior.md` 2.1).
    pub title_center_from_left_pt: f64,
    /// Центр числа от верха панели. `[estimate]` 20.
    pub value_center_from_top_pt: f64,
    /// Рамка ползунка: высота и отступ низа от низа панели. `[estimate]` 32 и 6 (ряд рисок ниже числа).
    pub slider_height_pt: f64,
    pub slider_bottom_inset_pt: f64,
    /// Боковые отступы ползунка от краёв панели (между кнопками). `[estimate]` 88.
    pub slider_side_inset_pt: f64,
    /// Диаметр кнопок «×» и сброса. `[estimate]` 36.
    pub button_diameter_pt: f64,
    /// Отступ центров кнопок от боковых краёв панели; по высоте кнопки стоят по центру. `[estimate]` 56.
    pub button_side_inset_pt: f64,
    /// Сторона значков «×» и сброса как доля диаметра кнопки. `[estimate]` 0,55 — подгонка, не Apple.
    pub button_icon_scale: f64,
    /// Допуск расхождения шкалы EV с состоянием в долях шага шкалы: меньше — шкала считается совпавшей и не
    /// переезжает (значения лежат на сетке). `[estimate]` 0,25 — четверть шага, подгонка, не Apple.
    pub state_match_tolerance_steps: f64,
}

impl Default for ExposurePanelLayout {
    fn default() -> Self {
        Self {
            width_pt: 400.0,
            height_pt: 72.0,
            bottom_inset_pt: 34.1,
            corner_radius_pt: 36.0,
            title_center_from_left_pt: 18.0,
            value_center_from_top_pt: 20.0,
            slider_height_pt: 32.0,
            slider_bottom_inset_pt: 6.0,
            slider_side_inset_pt: 88.0,
            button_diameter_pt: 36.0,
            button_side_inset_pt: 56.0,
            button_icon_scale: 0.55,
            state_match_tolerance_steps: 0.25,
        }
    }
}

/// Рамка фокуса, «солнце» и плашка.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FocusLayout {
    /// Зона касания «солнца»: узкая вертикальная полоса. `[estimate]` 40 × 200 (образец — тест арбитража `ui-core`).
    pub sun_touch_width_pt: f64,
    pub sun_touch_height_pt: f64,
    /// Отступ верха плашки AE/AF LOCK от верха кадра. `[estimate]` 12 («вверху кадра, по центру», `layout.md` 2.2).
    pub lock_plate_top_inset_pt: f64,
    /// Протяжённость «солнца» вверх-вниз для шкалы (область резинки). `[estimate]` 200.
    pub sun_scale_extent_pt: f64,
}

impl Default for FocusLayout {
    fn default() -> Self {
        Self {
            sun_touch_width_pt: 40.0,
            sun_touch_height_pt: 200.0,
            lock_plate_top_inset_pt: 12.0,
            sun_scale_extent_pt: 200.0,
        }
    }
}

/// Сетка, уровень, отсчёт, затемнение при снимке, подложка вне кадра.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct OverlayLayout {
    /// Цвет подложки вне кадра. `[doc]` `scope-v1.md`: в v1 чёрный.
    pub outside_frame_color: DisplayColor,
    /// Толщина и непрозрачность линий сетки. `[estimate]` 0,5 pt и 0,45.
    pub grid_line_width_pt: f64,
    pub grid_line_alpha: f32,
    /// Длина и толщина линии уровня. `[estimate]` 120 и 1,5 pt.
    pub level_length_pt: f64,
    pub level_thickness_pt: f64,
    /// Допуск «ровно» для жёлтого цвета уровня, градусы. `[estimate]` 0,5.
    pub level_aligned_tolerance_deg: f32,
    /// Затемнение кадра при снимке (`motion.md` 4.1, `[measured]`): глубина (1 − 19,2/128,7 ≈ 0,85),
    /// время нарастания (1 кадр, 16 мс), удержание (≈ 66 мс), скорость возврата λ ≈ 18 1/с.
    pub blink_peak_darkness: f64,
    pub blink_attack_ms: f64,
    pub blink_hold_ms: f64,
    pub blink_release_rate_per_s: f64,
    /// Остаток затемнения, с которого вспышка считается законченной. `[estimate]` 0,005.
    pub blink_end_threshold: f64,
    /// Текст по центру кадра, когда камеры нет (`is_camera_unavailable()`).
    pub unavailable_text: String,
}

impl Default for OverlayLayout {
    fn default() -> Self {
        Self {
            outside_frame_color: DisplayColor::BLACK,
            grid_line_width_pt: 0.5,
            grid_line_alpha: 0.45,
            level_length_pt: 120.0,
            level_thickness_pt: 1.5,
            level_aligned_tolerance_deg: 0.5,
            blink_peak_darkness: 0.85,
            blink_attack_ms: 16.0,
            blink_hold_ms: 66.0,
            blink_release_rate_per_s: 18.0,
            blink_end_threshold: 0.005,
            unavailable_text: "Camera unavailable".to_owned(),
        }
    }
}

/// Раскладка и вид виджетов экрана.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WidgetLayoutConfig {
    pub strip: StripLayout,
    pub bottom: BottomLayout,
    pub sheet: SheetLayout,
    pub exposure_panel: ExposurePanelLayout,
    pub focus: FocusLayout,
    pub overlay: OverlayLayout,
    /// Ёмкость очереди эффектов до опустошения приложением. `[estimate]` 64: за кадр экран выдаёт единицы
    /// эффектов; переполнение считается в диагностике, а не теряется молча.
    pub effect_queue_capacity: usize,
    /// Пауза между попытками пересобрать сцену после ошибки, мс: устойчивая ошибка (например, неподходящий
    /// размер) не должна пересобирать сцену на каждом кадре. `[estimate]` 250 — несколько десятков кадров,
    /// подгонка, не Apple. Пока пауза идёт, кадры отдаются ошибкой источника и считаются в диагностике.
    pub stage_rebuild_backoff_ms: f64,
    /// Предел кэша раскладки текста (число разных подписей): динамические подписи (EV, секунды) конечны, но
    /// кэш всё равно ограничен и при переполнении очищается. `[estimate]` 128.
    pub text_run_cache_capacity: usize,
}

impl Default for WidgetLayoutConfig {
    fn default() -> Self {
        Self {
            strip: StripLayout::default(),
            bottom: BottomLayout::default(),
            sheet: SheetLayout::default(),
            exposure_panel: ExposurePanelLayout::default(),
            focus: FocusLayout::default(),
            overlay: OverlayLayout::default(),
            effect_queue_capacity: 64,
            stage_rebuild_backoff_ms: 250.0,
            text_run_cache_capacity: 128,
        }
    }
}

fn positive(field: &'static str, value: f64) -> Result<(), WidgetConfigError> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(WidgetConfigError::NotPositive { field, value })
    }
}

fn fraction(field: &'static str, value: f64) -> Result<(), WidgetConfigError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(WidgetConfigError::NotFraction { field, value })
    }
}

impl WidgetLayoutConfig {
    /// Проверка значений: размеры конечны и положительны, доли лежат в 0..=1.
    pub fn validate(&self) -> Result<(), WidgetConfigError> {
        let strip = &self.strip;
        let bottom = &self.bottom;
        let sheet = &self.sheet;
        let exposure = &self.exposure_panel;
        let focus = &self.focus;
        let overlay = &self.overlay;
        let positives: [(&'static str, f64); 38] = [
            ("strip.pitch_pt", strip.pitch_pt),
            ("strip.icon_size_pt", strip.icon_size_pt),
            ("strip.touch_size_pt", strip.touch_size_pt),
            ("bottom.menu_button.diameter_pt", bottom.menu_button.diameter_pt),
            ("bottom.thumbnail.diameter_pt", bottom.thumbnail.diameter_pt),
            ("bottom.capsule_width_pt", bottom.capsule_width_pt),
            ("bottom.capsule_height_pt", bottom.capsule_height_pt),
            ("bottom.button_icon_size_pt", bottom.button_icon_size_pt),
            ("sheet.width_pt", sheet.width_pt),
            ("sheet.height_pt", sheet.height_pt),
            ("sheet.corner_radius_pt", sheet.corner_radius_pt),
            ("sheet.item_diameter_pt", sheet.item_diameter_pt),
            ("sheet.item_pitch_pt", sheet.item_pitch_pt),
            ("sheet.item_icon_size_pt", sheet.item_icon_size_pt),
            ("sheet.option_row_height_pt", sheet.option_row_height_pt),
            ("sheet.back_diameter_pt", sheet.back_diameter_pt),
            ("exposure_panel.width_pt", exposure.width_pt),
            ("exposure_panel.height_pt", exposure.height_pt),
            ("exposure_panel.corner_radius_pt", exposure.corner_radius_pt),
            ("exposure_panel.slider_height_pt", exposure.slider_height_pt),
            ("exposure_panel.button_diameter_pt", exposure.button_diameter_pt),
            ("focus.sun_touch_width_pt", focus.sun_touch_width_pt),
            ("focus.sun_touch_height_pt", focus.sun_touch_height_pt),
            ("focus.sun_scale_extent_pt", focus.sun_scale_extent_pt),
            ("overlay.grid_line_width_pt", overlay.grid_line_width_pt),
            ("overlay.level_length_pt", overlay.level_length_pt),
            ("overlay.level_thickness_pt", overlay.level_thickness_pt),
            ("overlay.blink_attack_ms", overlay.blink_attack_ms),
            ("overlay.blink_hold_ms", overlay.blink_hold_ms),
            ("overlay.blink_release_rate_per_s", overlay.blink_release_rate_per_s),
            ("overlay.blink_end_threshold", overlay.blink_end_threshold),
            ("sheet.caption_below_circle_pt", sheet.caption_below_circle_pt),
            ("sheet.option_side_inset_pt", sheet.option_side_inset_pt),
            ("strip.value_below_icon_pt", strip.value_below_icon_pt),
            (
                "strip.raw_strike_length_scale",
                f64::from(strip.raw_strike_length_scale),
            ),
            ("sheet.back_icon_scale", sheet.back_icon_scale),
            ("exposure_panel.button_icon_scale", exposure.button_icon_scale),
            (
                "exposure_panel.state_match_tolerance_steps",
                exposure.state_match_tolerance_steps,
            ),
        ];
        for (field, value) in positives {
            positive(field, value)?;
        }
        fraction("overlay.blink_peak_darkness", overlay.blink_peak_darkness)?;
        fraction("overlay.grid_line_alpha", f64::from(overlay.grid_line_alpha))?;
        fraction("sheet.item_fill_alpha", f64::from(sheet.item_fill_alpha))?;
        fraction("bottom.thumbnail_dot_scale", f64::from(bottom.thumbnail_dot_scale))?;
        positive("stage_rebuild_backoff_ms", self.stage_rebuild_backoff_ms)?;
        if !strip.raw_strike_angle_rad.is_finite() {
            return Err(WidgetConfigError::NotFinite {
                field: "strip.raw_strike_angle_rad",
                value: f64::from(strip.raw_strike_angle_rad),
            });
        }
        if self.text_run_cache_capacity == 0 {
            return Err(WidgetConfigError::NotPositive {
                field: "text_run_cache_capacity",
                value: 0.0,
            });
        }
        if self.effect_queue_capacity == 0 {
            return Err(WidgetConfigError::NotPositive {
                field: "effect_queue_capacity",
                value: 0.0,
            });
        }
        if sheet.columns == 0 {
            return Err(WidgetConfigError::NoMenuColumns);
        }
        Ok(())
    }
}
