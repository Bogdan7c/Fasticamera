//! Конфиг зума экрана камеры (сессия 13): лента кнопок, колесо, щипок, переходы между камерами.
//!
//! Отдельный модуль, а не часть `widgets_config.rs` (тот уже ≈ 600 строк): все числа зума живут здесь, с
//! метками происхождения (`[measured]` / `[derived]` / `[estimate]`; оценки — «подгонка, не Apple»,
//! решения 111–115). Пределы зума (10× сзади, 3× спереди) сюда НЕ входят: их задаёт каталог камер
//! (`ZoomCatalog::range`), экран их не выдумывает.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::widgets_config::RoundButtonLayout;

/// Ошибка конфига зума.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ZoomConfigError {
    #[error("зум: {field} должно быть конечным и > 0, получено {value}")]
    NotPositive { field: &'static str, value: f64 },
    #[error("зум: {field} должно быть конечным, получено {value}")]
    NotFinite { field: &'static str, value: f64 },
    #[error("зум: {reason}")]
    Inconsistent { reason: &'static str },
}

/// Лента кнопок зума над кнопками режимов. Координаты — эталонного iPhone (привязка к низу — `ScreenGeometry`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ZoomStripLayout {
    /// Шаг между подписями. `[measured: R00 t=88.5–100]` 39,0.
    pub pitch_pt: f64,
    /// Позиция выбранной подписи по x (всегда одна и та же, лента едет под ней). `[measured: R00 t=88.5–100]` 220.
    pub selected_x_pt: f64,
    /// Центр ленты по y. `[measured: R00 t=88.5–100]` 709,9.
    pub center_y_pt: f64,
    /// Диаметр тёмного круга выбранной кнопки. `[measured: R00 t=88.5–100]` 37 ± 1,5.
    pub selected_circle_diameter_pt: f64,
    /// Прозрачность тёмного круга выбранной. `[estimate]` 0,45 — подгонка, не Apple (в замере «тёмный полупрозрачный»).
    pub selected_circle_alpha: f32,
    /// Сторона зоны касания кнопки. `[doc]` 44: минимальная цель касания HIG.
    pub touch_size_pt: f64,
    /// Сколько кнопок вмещает лента (ячеек сцены). Больше упоров в каталоге — явная ошибка, а не тихая обрезка.
    /// Технический предел, не число Apple.
    pub max_buttons: u8,
}

impl Default for ZoomStripLayout {
    fn default() -> Self {
        Self {
            pitch_pt: 39.0,
            selected_x_pt: 220.0,
            center_y_pt: 709.9,
            selected_circle_diameter_pt: 37.0,
            selected_circle_alpha: 0.45,
            touch_size_pt: 44.0,
            max_buttons: 5,
        }
    }
}

/// Колесо зума.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ZoomWheelLayout {
    /// Центр диска по x. `[measured: R00 t=88.5–100]` 219,7.
    pub center_x_pt: f64,
    /// Центр диска по y (эталонный экран). `[measured: R00 t=88.5–100]` 848,9.
    pub center_y_pt: f64,
    /// Радиус подписей-чисел. `[measured: R00 t=88.5–100]` 206,8.
    pub number_label_radius_pt: f64,
    /// Радиус подписи фокусного («24MM»). `[measured: R00 t=88.5–100]` ≈ 191.
    pub focal_label_radius_pt: f64,
    /// Центр строки «1.7×» (жёлтая) по y в эталонных координатах. `[estimate]` 703 — подгонка, не Apple: на месте
    /// выбранной кнопки ленты (центр ленты 709,9), которую колесо заменяет; точное место в замере не снято.
    pub readout_value_center_y_pt: f64,
    /// Центр строки «24MM» под значением по y. `[estimate]` 720 — подгонка, не Apple.
    pub readout_focal_center_y_pt: f64,
    /// Верх зоны касания колеса (эталонный экран): диск виден от верха колец рисок вниз. `[derived]`
    /// 848,9 − 238,5 ≈ 610.
    pub touch_top_pt: f64,
    /// Явный шаг рисок до порога. `[measured: R00 t=88.5–100]` 0,1×.
    pub fine_tick_step: f64,
    /// До какого значения риски идут с мелким шагом. `[measured: R00 t=88.5–100]` ≈ 4,4×.
    pub fine_tick_until: f64,
    /// Шаг рисок за порогом. `[measured: R00 t=88.5–100]` 1×.
    pub coarse_tick_step: f64,
    /// Значения ярких рисок до порога целых. `[measured: R00 t=88.5–100]` 0,5 / 1 / 1,2 / 1,5 / 2.
    pub bright_tick_values: Vec<f64>,
    /// С какого значения яркие все целые. `[measured: R00 t=88.5–100]` 3.
    pub bright_whole_numbers_from: f64,
    /// Риска ближе этого (в октавах) к упору сливается с упором (0,6 рядом с 0,593 — одна риска). `[estimate]`
    /// подгонка, не Apple.
    pub tick_merge_octaves: f64,
    /// Опорные значения с подписями-числами на кольце. `[measured: R00 t=88.5–100]` .5 / 1 / 2 / 3… подписи ставятся
    /// у упоров каталога и у целых из этого списка.
    pub number_label_values: Vec<f64>,
    /// Появление: длительность. `[measured: R00 t=88.5–100]` 0,2 с.
    pub appear_ms: f64,
    /// Появление: подъём. `[measured: R00 t=88.5–100]` 10,7 pt.
    pub appear_rise_pt: f64,
    /// Появление: перелёт. `[measured: R00 t=88.5–100]` 1,7 pt.
    pub appear_overshoot_pt: f64,
    /// Уход: через сколько после остановки пальца и колеса. `[measured: R00 t=88.5–100]` ≈ 2,15 с.
    pub idle_dismiss_ms: f64,
    /// Уход: длительность. `[measured: R00 t=88.5–100]` 0,11 с.
    pub dismiss_ms: f64,
    /// Уход: на сколько колесо уезжает вниз, растворяясь. `[measured: R00 t=88.5–100]` 33 pt.
    pub dismiss_drop_pt: f64,
}

impl Default for ZoomWheelLayout {
    fn default() -> Self {
        Self {
            center_x_pt: 219.7,
            center_y_pt: 848.9,
            number_label_radius_pt: 206.8,
            focal_label_radius_pt: 191.0,
            readout_value_center_y_pt: 703.0,
            readout_focal_center_y_pt: 720.0,
            touch_top_pt: 610.0,
            fine_tick_step: 0.1,
            fine_tick_until: 4.4,
            coarse_tick_step: 1.0,
            bright_tick_values: vec![0.5, 1.0, 1.2, 1.5, 2.0],
            bright_whole_numbers_from: 3.0,
            tick_merge_octaves: 0.03,
            number_label_values: vec![0.5, 1.0, 2.0, 3.0, 5.0, 10.0],
            appear_ms: 200.0,
            appear_rise_pt: 10.7,
            appear_overshoot_pt: 1.7,
            idle_dismiss_ms: 2150.0,
            dismiss_ms: 110.0,
            dismiss_drop_pt: 33.0,
        }
    }
}

/// Движение значения зума.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ZoomMotionConfig {
    /// Длительность перехода к упору по тапу на кнопке. `[measured: R00 t=88.5–100]` 0,27–0,30 с, кривая
    /// smoothstep (таблица прогресса `motion.md` M:124 совпала с ней в пределах ±0,02).
    pub glide_ms: f64,
}

impl Default for ZoomMotionConfig {
    fn default() -> Self {
        Self { glide_ms: 270.0 }
    }
}

/// Щипок по видоискателю.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ZoomPinchConfig {
    /// Играть ли щелчок щипка на упорах каталога (решение 112: «щипок — только щелчки на точках объективов»).
    /// Сам щипок — правило, а не число: коэффициент = значение в начале жеста × масштаб жеста (`reduce/zoom.rs`).
    /// Частоту щелчков ограничивает `ui_kit::haptic_throttle` (`[estimate]` 30 мс).
    pub haptics_at_stops: bool,
}

impl Default for ZoomPinchConfig {
    fn default() -> Self {
        Self { haptics_at_stops: true }
    }
}

/// Переходы между камерами (размытие удержанного кадра и переворот фронт/тыл).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ZoomTransitionConfig {
    /// Проявление живого кадра после удержания: размытие уходит за это время. `[estimate]` 0,2 с — по образцу
    /// смены режима iOS («резкость возвращается за ≈ 0,2 с», `motion.md` M:108).
    pub blur_fade_out_ms: f64,
    /// Поворот-карточка на переворот: длительность. `[estimate]` 0,27 с (iOS: 0,17 с чёрного + 0,1 с въезда).
    pub flip_rotate_ms: f64,
    /// Растворение после переворота. `[estimate]` 0,15 с (iOS: 0,1–0,15 с).
    pub flip_fade_out_ms: f64,
    /// Карточка уже этого (доля ширины) не рисуется: при ширине 0 преобразование вырождено, а список рисования
    /// с вырожденным преобразованием рендер отвергает целиком. Технический порог, не число Apple.
    pub min_card_width_fraction: f64,
}

impl Default for ZoomTransitionConfig {
    fn default() -> Self {
        Self {
            blur_fade_out_ms: 200.0,
            flip_rotate_ms: 270.0,
            flip_fade_out_ms: 150.0,
            min_card_width_fraction: 0.01,
        }
    }
}

/// Весь конфиг зума.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ZoomConfig {
    pub strip: ZoomStripLayout,
    pub wheel: ZoomWheelLayout,
    pub motion: ZoomMotionConfig,
    pub pinch: ZoomPinchConfig,
    pub transition: ZoomTransitionConfig,
    /// Кнопка переворота. `[measured: R00 t=7.5–10.8]` круг Ø 48 в (382; 898).
    pub flip_button: RoundButtonLayout,
    /// Заливка кнопки переворота. `[measured: R00 t=7.5–10.8]` #1C1C1C, без кольца.
    pub flip_button_fill_rgb: [f32; 3],
    /// Размер значка на кнопке переворота. `[estimate]` 22 — подгонка, не Apple; значок свой (решение 81).
    pub flip_icon_size_pt: f64,
}

impl Default for ZoomConfig {
    fn default() -> Self {
        Self {
            strip: ZoomStripLayout::default(),
            wheel: ZoomWheelLayout::default(),
            motion: ZoomMotionConfig::default(),
            pinch: ZoomPinchConfig::default(),
            transition: ZoomTransitionConfig::default(),
            flip_button: RoundButtonLayout {
                center_x_pt: 382.0,
                center_y_pt: 898.0,
                diameter_pt: 48.0,
            },
            flip_button_fill_rgb: [28.0 / 255.0, 28.0 / 255.0, 28.0 / 255.0],
            flip_icon_size_pt: 22.0,
        }
    }
}

fn positive(field: &'static str, value: f64) -> Result<(), ZoomConfigError> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(ZoomConfigError::NotPositive { field, value })
    }
}

fn finite(field: &'static str, value: f64) -> Result<(), ZoomConfigError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(ZoomConfigError::NotFinite { field, value })
    }
}

impl ZoomConfig {
    /// Проверка чисел: размеры и времена конечны и положительны, пороги согласованы.
    pub fn validate(&self) -> Result<(), ZoomConfigError> {
        let strip = &self.strip;
        for (field, value) in [
            ("strip.pitch_pt", strip.pitch_pt),
            ("strip.selected_circle_diameter_pt", strip.selected_circle_diameter_pt),
            ("strip.touch_size_pt", strip.touch_size_pt),
            ("wheel.number_label_radius_pt", self.wheel.number_label_radius_pt),
            ("wheel.focal_label_radius_pt", self.wheel.focal_label_radius_pt),
            ("wheel.fine_tick_step", self.wheel.fine_tick_step),
            ("wheel.fine_tick_until", self.wheel.fine_tick_until),
            ("wheel.coarse_tick_step", self.wheel.coarse_tick_step),
            ("wheel.bright_whole_numbers_from", self.wheel.bright_whole_numbers_from),
            ("wheel.appear_ms", self.wheel.appear_ms),
            ("wheel.idle_dismiss_ms", self.wheel.idle_dismiss_ms),
            ("wheel.dismiss_ms", self.wheel.dismiss_ms),
            ("motion.glide_ms", self.motion.glide_ms),
            ("transition.blur_fade_out_ms", self.transition.blur_fade_out_ms),
            ("transition.flip_rotate_ms", self.transition.flip_rotate_ms),
            ("transition.flip_fade_out_ms", self.transition.flip_fade_out_ms),
            ("flip_button.diameter_pt", self.flip_button.diameter_pt),
            ("flip_icon_size_pt", self.flip_icon_size_pt),
        ] {
            positive(field, value)?;
        }
        for (field, value) in [
            ("strip.selected_x_pt", strip.selected_x_pt),
            ("strip.center_y_pt", strip.center_y_pt),
            ("wheel.center_x_pt", self.wheel.center_x_pt),
            ("wheel.center_y_pt", self.wheel.center_y_pt),
            ("wheel.touch_top_pt", self.wheel.touch_top_pt),
            ("wheel.readout_value_center_y_pt", self.wheel.readout_value_center_y_pt),
            ("wheel.readout_focal_center_y_pt", self.wheel.readout_focal_center_y_pt),
            ("wheel.appear_rise_pt", self.wheel.appear_rise_pt),
            ("wheel.appear_overshoot_pt", self.wheel.appear_overshoot_pt),
            ("wheel.dismiss_drop_pt", self.wheel.dismiss_drop_pt),
            ("wheel.tick_merge_octaves", self.wheel.tick_merge_octaves),
            (
                "transition.min_card_width_fraction",
                self.transition.min_card_width_fraction,
            ),
            ("flip_button.center_x_pt", self.flip_button.center_x_pt),
            ("flip_button.center_y_pt", self.flip_button.center_y_pt),
        ] {
            finite(field, value)?;
        }
        for value in self
            .wheel
            .bright_tick_values
            .iter()
            .chain(&self.wheel.number_label_values)
        {
            positive("wheel.bright_tick_values / number_label_values", *value)?;
        }
        if strip.max_buttons == 0 {
            return Err(ZoomConfigError::Inconsistent {
                reason: "в ленте нужна хотя бы одна кнопка",
            });
        }
        if !(0.0..1.0).contains(&self.transition.min_card_width_fraction) {
            return Err(ZoomConfigError::Inconsistent {
                reason: "min_card_width_fraction должен лежать в 0..1",
            });
        }
        if self.flip_button_fill_rgb.iter().any(|c| !(0.0..=1.0).contains(c)) {
            return Err(ZoomConfigError::Inconsistent {
                reason: "заливка кнопки переворота: компоненты в 0..1",
            });
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "zoom_config_tests.rs"]
mod tests;
