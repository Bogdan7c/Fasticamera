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
    /// Число должно быть конечным и не меньше нуля (отступ, расстояние, которое может быть нулевым).
    #[error("значение раскладки виджетов {field} должно быть конечным и >= 0, получено {value}")]
    Negative { field: &'static str, value: f64 },
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
    /// Центр по y. `[measured: C t=18.0]` 102,7.
    pub center_y_pt: f64,
    /// Шаг центров значков. `[measured: C t=18.0]` 43,6.
    pub pitch_pt: f64,
    /// Сторона квадрата иконки. `[measured: C t=18.0]` ≈ 20.
    pub icon_size_pt: f64,
    /// Сторона зоны касания. `[doc]` 44: минимальная цель касания HIG.
    pub touch_size_pt: f64,
    /// Сдвиг подписи значения (секунды таймера, EV) вниз от центра значка. `[estimate]` подгонка, не Apple.
    pub value_below_icon_pt: f64,
    /// Центр числа EV («0.0») относительно центра значка по y: 98,5 − 102,7. `[measured: C t=18.0]` −4,2.
    pub ev_value_offset_pt: f64,
    /// Центр единицы («EV») относительно центра значка по y: 111,6 − 102,7. `[measured: C t=18.0]` 8,9.
    pub ev_unit_offset_pt: f64,
    /// Наклон зачёркивающей линии у выключенного RAW, радианы (≈ −35°). `[estimate]` подгонка, не Apple.
    pub raw_strike_angle_rad: f32,
    /// Длина зачёркивающей линии как доля ширины подписи RAW (линия чуть выступает за буквы). `[estimate]` 1,15.
    pub raw_strike_length_scale: f32,
}

impl Default for StripLayout {
    fn default() -> Self {
        Self {
            center_x_pt: 220.0,
            center_y_pt: 102.7,
            pitch_pt: 43.6,
            icon_size_pt: 20.0,
            touch_size_pt: 44.0,
            value_below_icon_pt: 22.0,
            ev_value_offset_pt: -4.2,
            ev_unit_offset_pt: 8.9,
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
    /// Кнопка меню Controls. `[measured: C t=18.0]` центр (383,6; 801,9), Ø 44.
    pub menu_button: RoundButtonLayout,
    /// Миниатюра. `[measured: R00 t=10.027]` центр (57,7; 898,0), Ø 48.
    pub thumbnail: RoundButtonLayout,
    /// Центр капсулы режимов по x. `[measured]` 220,0.
    pub capsule_center_x_pt: f64,
    /// Центр капсулы по y. `[measured: C t=18.0]` 898,0.
    pub capsule_center_y_pt: f64,
    /// Размер капсулы. `[measured: C t=18.0]` 221,4 × 48,9.
    pub capsule_width_pt: f64,
    pub capsule_height_pt: f64,
    /// Пилюля выбранного режима внутри капсулы: 89 × 43 (отступ 3 pt). `[measured: C t=18.0, R00 t=10.027]`.
    pub mode_pill_width_pt: f64,
    pub mode_pill_height_pt: f64,
    /// Заливка пилюли: белый α ≈ 0,1. `[derived]` по яркости пилюли над капсулой (C t=18.0).
    pub mode_pill_fill_alpha: f32,
    /// Ободок пилюли: толщина 1 pt `[measured: R00 t=10.027]`, белый α ≈ 0,3 `[derived]`.
    pub mode_pill_border_width_pt: f64,
    pub mode_pill_border_alpha: f32,
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
                center_x_pt: 383.6,
                center_y_pt: 801.9,
                diameter_pt: 44.0,
            },
            thumbnail: RoundButtonLayout {
                center_x_pt: 57.7,
                center_y_pt: 898.0,
                diameter_pt: 48.0,
            },
            capsule_center_x_pt: 220.0,
            capsule_center_y_pt: 898.0,
            capsule_width_pt: 221.4,
            capsule_height_pt: 48.9,
            mode_pill_width_pt: 89.0,
            mode_pill_height_pt: 43.0,
            mode_pill_fill_alpha: 0.1,
            mode_pill_border_width_pt: 1.0,
            mode_pill_border_alpha: 0.3,
            button_icon_size_pt: 24.0,
            thumbnail_dot_scale: 0.86,
        }
    }
}

/// Шторка меню Controls (варианты инструментов живут в нижней карточке, [`CardLayout`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SheetLayout {
    /// Размер шторки (x 5–435). `[measured: C t=30.0]` 430 × 464.
    pub width_pt: f64,
    pub height_pt: f64,
    /// Отступ низа шторки от низа экрана. `[measured: C t=30.0]` 8,6.
    pub bottom_inset_pt: f64,
    /// Радиус шторки. `[measured: C t=30.0]` 54.
    pub corner_radius_pt: f64,
    /// Диаметр пункта. `[measured: C t=30.0]` 68.
    pub item_diameter_pt: f64,
    /// Шаг центров пунктов по x (91 / 220 / 349). `[measured: C t=30.0]` 129.
    pub column_pitch_pt: f64,
    /// Шаг рядов по y. `[measured: C t=30.0]` 124.
    pub row_pitch_pt: f64,
    /// Число колонок. `[measured: C t=30.0]` 3.
    pub columns: u8,
    /// Центр первого ряда ниже верха шторки. `[measured: C t=30.0]` 83.
    pub first_row_center_from_top_pt: f64,
    /// Сторона иконки пункта. `[measured: C t=30.0]` ≈ 28.
    pub item_icon_size_pt: f64,
    /// Расстояние от нижнего края круга до центра заглавных подписи: базовая линия на 22,7 ниже круга
    /// `[measured: R00 t=22.512]`, высота заглавных 10,3 → центр на 22,7 − 5,15 = 17,55 `[derived]`.
    pub caption_center_below_circle_pt: f64,
    /// На сколько pt пункты шторки «въезжают» снизу при открытии. `[measured: C t=22.88–23.15]` 4,9.
    pub open_content_slide_pt: f64,
    /// Прозрачность подписей ВСЕХ пунктов (белые): пик 228–231 на фоне 60 `[measured: R00 t=22.512]`, 0,88.
    pub caption_alpha: f32,
    /// Прозрачность чёрной заливки круга пункта поверх стекла шторки. `[measured: C t=30.0]` 0,5.
    pub item_fill_alpha: f32,
    /// «Ручка» шторки: размер 37,6 × 3,7 `[measured: C t=30.0]`, белая α 0,75, верх на 8,9 ниже верха шторки.
    pub handle_width_pt: f64,
    pub handle_height_pt: f64,
    pub handle_top_inset_pt: f64,
    pub handle_alpha: f32,
}

impl Default for SheetLayout {
    fn default() -> Self {
        Self {
            width_pt: 430.0,
            height_pt: 464.0,
            bottom_inset_pt: 8.6,
            corner_radius_pt: 54.0,
            item_diameter_pt: 68.0,
            column_pitch_pt: 129.0,
            row_pitch_pt: 124.0,
            columns: 3,
            first_row_center_from_top_pt: 83.0,
            item_icon_size_pt: 28.0,
            caption_center_below_circle_pt: 17.55,
            open_content_slide_pt: 4.9,
            caption_alpha: 0.88,
            item_fill_alpha: 0.5,
            handle_width_pt: 37.6,
            handle_height_pt: 3.7,
            handle_top_inset_pt: 8.9,
            handle_alpha: 0.75,
        }
    }
}

/// Точка внутри карточки: смещение центра от середины карточки по x и отступ от верха карточки.
///
/// Всё отсчитывается от самой карточки, а не от абсолютных координат эталона 440 pt: карточка стоит по центру
/// экрана, поэтому при другой ширине экрана кнопки и ряд остаются симметричны и под заголовком.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardPoint {
    /// Смещение центра по x от середины карточки, pt (вправо положительно).
    pub offset_from_center_x_pt: f64,
    /// Расстояние от верха карточки до центра, pt.
    pub center_from_top_pt: f64,
}

/// Нижняя карточка инструмента (решение 103): варианты Timer/Flash/Aspect/RAW/Grid/Level и шкала Exposure.
///
/// Карточка стоит у низа экрана (привязка ScreenGeometry), поверх нижнего ряда; ряд затвора при ней поднят
/// на `raised_shutter_offset_pt`. Метки: `[measured: C t=…]` — iOS 27, `[measured: R00 t=…]` — iOS 26
/// (3 px = 1 pt), `[estimate]` — подгонка, не Apple. Вертикальные числа — от верха карточки `T`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CardLayout {
    /// Ширина рамки (x 9,5–430,5). `[measured: C t=40.0]` 421.
    pub width_pt: f64,
    /// Высота рамки (верх 764,7, низ 947,8). `[measured: C t=40.0]` 183,1.
    pub height_pt: f64,
    /// Отступ низа рамки от низа экрана (956 − 947,8). `[measured: C t=40.0]` 8,2.
    pub bottom_inset_pt: f64,
    /// Радиус скругления. `[measured: C t=40.0]` 52.
    pub corner_radius_pt: f64,
    /// Диаметр тёмного диска боковых кнопок. `[measured: C t=40.0]` 36.
    pub button_diameter_pt: f64,
    /// Левая кнопка «‹» / «×». `[measured: C t=40.0]` центр (49; T+43): x 49 − 220 = −171 от середины.
    pub left_button: CardPoint,
    /// Правая кнопка «↻» (только Exposure). `[measured: C t=54.0]` центр (391; T+44): x 391 − 220 = +171.
    pub right_button: CardPoint,
    /// Сторона квадрата значка боковых кнопок. `[estimate]` 20 (чернила шеврона 7,6 × 15,1, «×» 13,2,
    /// «↻» 18,8 × 14 — `[measured: C t=40.0, t=54.0]` — зависят от рисунка значка).
    pub button_icon_size_pt: f64,
    /// Прозрачность чёрного диска боковых кнопок. `[estimate]` 0,35.
    pub button_disc_alpha: f32,
    /// Минимальная сторона зоны касания боковых кнопок. `[doc]` 44: цель касания HIG.
    pub button_touch_side_pt: f64,
    /// Прозрачность кнопки сброса при EV = 0. `[estimate]` 0,44 (в 26 снято: `[measured: R00 t=36.77–36.95]`;
    /// в 27 не снималось).
    pub reset_dim_opacity: f32,
    /// Центр заголовка (заглавные 10,3 pt) от верха карточки. `[measured: C t=40.0]` 30.
    pub title_center_from_top_pt: f64,
    /// Центр крупного значения (цифры 20,7 pt) от верха карточки. `[measured: C t=40.0, t=54.0]` 77.
    pub value_center_from_top_pt: f64,
    /// Ряд вариантов: центр по x (от середины карточки) и от верха карточки. `[measured: C t=40.0]` x 220 (то
    /// есть 0 от середины), T+118.
    pub options_center: CardPoint,
    /// Шаг центров вариантов. `[measured: C t=40.0]` 75,7.
    pub option_pitch_pt: f64,
    /// Высота зоны касания варианта. `[estimate]` 56 (≥ 44 pt по HIG); ширина зоны — шаг.
    pub option_touch_height_pt: f64,
    /// Сторона значка варианта (flash, timer). `[measured: C t=40.0]` ≈ 21,7 × 20,7 → квадрат 21,7.
    pub option_icon_size_pt: f64,
    /// Низ рисок шкалы Exposure от верха карточки. `[measured: C t=40.0]` T+129.
    pub ruler_ticks_bottom_from_top_pt: f64,
    /// Высота зоны пана шкалы (ширина — вся карточка). `[estimate]` 60 (≥ 44 pt).
    pub ruler_touch_height_pt: f64,
    /// На сколько pt поднят ряд затвора, пока открыта карточка (центр 801,9 → 701,9).
    /// `[measured: C t=54.0]` 100, ±3.
    pub raised_shutter_offset_pt: f64,
    /// Допуск расхождения шкалы EV с состоянием в долях шага: меньше — шкала считается совпавшей.
    /// `[estimate]` 0,25 — четверть шага, подгонка, не Apple.
    pub state_match_tolerance_steps: f64,
}

impl Default for CardLayout {
    fn default() -> Self {
        Self {
            width_pt: 421.0,
            height_pt: 183.1,
            bottom_inset_pt: 8.2,
            corner_radius_pt: 52.0,
            button_diameter_pt: 36.0,
            left_button: CardPoint {
                offset_from_center_x_pt: -171.0,
                center_from_top_pt: 43.0,
            },
            right_button: CardPoint {
                offset_from_center_x_pt: 171.0,
                center_from_top_pt: 44.0,
            },
            button_icon_size_pt: 20.0,
            button_disc_alpha: 0.35,
            button_touch_side_pt: 44.0,
            reset_dim_opacity: 0.44,
            title_center_from_top_pt: 30.0,
            value_center_from_top_pt: 77.0,
            options_center: CardPoint {
                offset_from_center_x_pt: 0.0,
                center_from_top_pt: 118.0,
            },
            option_pitch_pt: 75.7,
            option_touch_height_pt: 56.0,
            option_icon_size_pt: 21.7,
            ruler_ticks_bottom_from_top_pt: 129.0,
            ruler_touch_height_pt: 60.0,
            raised_shutter_offset_pt: 100.0,
            state_match_tolerance_steps: 0.25,
        }
    }
}

/// Рамка фокуса, «солнце» и плашка.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FocusLayout {
    /// Зона касания «солнца»: узкая вертикальная полоса. Ширина `[estimate]` 40; высота = длина направляющей
    /// `[measured: R00 t=99.05–109.8]` 143.
    pub sun_touch_width_pt: f64,
    pub sun_touch_height_pt: f64,
    /// Отступ верха плашки AE/AF LOCK от верха кадра. `[estimate]` 12 («вверху кадра, по центру», `layout.md` 2.2).
    pub lock_plate_top_inset_pt: f64,
    /// Протяжённость «солнца» вверх-вниз для шкалы (область резинки) = длина направляющей.
    /// `[measured: R00 t=99.05–109.8]` 143.
    pub sun_scale_extent_pt: f64,
}

impl Default for FocusLayout {
    fn default() -> Self {
        Self {
            sun_touch_width_pt: 40.0,
            sun_touch_height_pt: 143.0,
            lock_plate_top_inset_pt: 12.0,
            sun_scale_extent_pt: 143.0,
        }
    }
}

/// Сетка, уровень, отсчёт, затемнение при снимке, подложка вне кадра.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct OverlayLayout {
    /// Цвет подложки вне кадра. `[doc]` `scope-v1.md`: в v1 чёрный.
    pub outside_frame_color: DisplayColor,
    /// Толщина и непрозрачность линий сетки. `[measured: R00 t=10.027]` 1,0 pt и белый α 0,36.
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
            grid_line_width_pt: 1.0,
            grid_line_alpha: 0.36,
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
    pub card: CardLayout,
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
            card: CardLayout::default(),
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

fn non_negative(field: &'static str, value: f64) -> Result<(), WidgetConfigError> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(WidgetConfigError::Negative { field, value })
    }
}

/// Конечное число любого знака (смещение, угол).
fn finite(field: &'static str, value: f64) -> Result<(), WidgetConfigError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(WidgetConfigError::NotFinite { field, value })
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
    /// Расстановка карточки: отступ и вертикали — числа от низа/верха карточки, смещения — любого знака, но
    /// конечные (NaN/inf сломали бы все рамки карточки молча).
    fn validate_card_placement(card: &CardLayout) -> Result<(), WidgetConfigError> {
        non_negative("card.bottom_inset_pt", card.bottom_inset_pt)?;
        let points = [
            (
                (
                    "card.left_button.offset_from_center_x_pt",
                    "card.left_button.center_from_top_pt",
                ),
                card.left_button,
            ),
            (
                (
                    "card.right_button.offset_from_center_x_pt",
                    "card.right_button.center_from_top_pt",
                ),
                card.right_button,
            ),
            (
                (
                    "card.options_center.offset_from_center_x_pt",
                    "card.options_center.center_from_top_pt",
                ),
                card.options_center,
            ),
        ];
        for ((x_field, y_field), point) in points {
            finite(x_field, point.offset_from_center_x_pt)?;
            finite(y_field, point.center_from_top_pt)?;
        }
        finite("card.title_center_from_top_pt", card.title_center_from_top_pt)?;
        finite("card.value_center_from_top_pt", card.value_center_from_top_pt)?;
        finite(
            "card.ruler_ticks_bottom_from_top_pt",
            card.ruler_ticks_bottom_from_top_pt,
        )?;
        // Сдвиг ряда затвора может быть любого знака, но конечным.
        finite("card.raised_shutter_offset_pt", card.raised_shutter_offset_pt)
    }

    /// Проверка значений: размеры конечны и положительны, доли лежат в 0..=1.
    pub fn validate(&self) -> Result<(), WidgetConfigError> {
        let strip = &self.strip;
        let bottom = &self.bottom;
        let sheet = &self.sheet;
        let card = &self.card;
        let focus = &self.focus;
        let overlay = &self.overlay;
        let positives = [
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
            ("sheet.column_pitch_pt", sheet.column_pitch_pt),
            ("sheet.row_pitch_pt", sheet.row_pitch_pt),
            ("sheet.first_row_center_from_top_pt", sheet.first_row_center_from_top_pt),
            (
                "sheet.caption_center_below_circle_pt",
                sheet.caption_center_below_circle_pt,
            ),
            ("sheet.handle_width_pt", sheet.handle_width_pt),
            ("sheet.handle_height_pt", sheet.handle_height_pt),
            ("sheet.handle_top_inset_pt", sheet.handle_top_inset_pt),
            ("bottom.mode_pill_width_pt", bottom.mode_pill_width_pt),
            ("bottom.mode_pill_height_pt", bottom.mode_pill_height_pt),
            ("bottom.mode_pill_border_width_pt", bottom.mode_pill_border_width_pt),
            ("sheet.item_icon_size_pt", sheet.item_icon_size_pt),
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
            ("strip.value_below_icon_pt", strip.value_below_icon_pt),
            (
                "strip.raw_strike_length_scale",
                f64::from(strip.raw_strike_length_scale),
            ),
            ("card.width_pt", card.width_pt),
            ("card.height_pt", card.height_pt),
            ("card.corner_radius_pt", card.corner_radius_pt),
            ("card.button_diameter_pt", card.button_diameter_pt),
            ("card.button_touch_side_pt", card.button_touch_side_pt),
            ("card.option_pitch_pt", card.option_pitch_pt),
            ("card.option_touch_height_pt", card.option_touch_height_pt),
            ("card.option_icon_size_pt", card.option_icon_size_pt),
            ("card.button_icon_size_pt", card.button_icon_size_pt),
            ("card.ruler_touch_height_pt", card.ruler_touch_height_pt),
            ("card.state_match_tolerance_steps", card.state_match_tolerance_steps),
        ];
        for (field, value) in positives {
            positive(field, value)?;
        }
        fraction("overlay.blink_peak_darkness", overlay.blink_peak_darkness)?;
        fraction("overlay.grid_line_alpha", f64::from(overlay.grid_line_alpha))?;
        fraction("sheet.item_fill_alpha", f64::from(sheet.item_fill_alpha))?;
        fraction("sheet.caption_alpha", f64::from(sheet.caption_alpha))?;
        fraction("sheet.handle_alpha", f64::from(sheet.handle_alpha))?;
        fraction("bottom.mode_pill_fill_alpha", f64::from(bottom.mode_pill_fill_alpha))?;
        fraction(
            "bottom.mode_pill_border_alpha",
            f64::from(bottom.mode_pill_border_alpha),
        )?;
        // Сдвиги подписей EV могут быть любого знака, но конечными.
        finite("strip.ev_unit_offset_pt", strip.ev_unit_offset_pt)?;
        finite("strip.ev_value_offset_pt", strip.ev_value_offset_pt)?;
        // Въезд пунктов шторки может быть нулевым («без въезда»), но не отрицательным.
        non_negative("sheet.open_content_slide_pt", sheet.open_content_slide_pt)?;
        fraction("bottom.thumbnail_dot_scale", f64::from(bottom.thumbnail_dot_scale))?;
        fraction("card.button_disc_alpha", f64::from(card.button_disc_alpha))?;
        fraction("card.reset_dim_opacity", f64::from(card.reset_dim_opacity))?;
        Self::validate_card_placement(card)?;
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
