//! Раскладка карточки белого (white-balance 02, решение WB28) и числа слежения шкал за фактами (WB21).
//!
//! Карточка белого выше обычной на `extra_height_pt`: три ряда касания по 44 pt (числа осей, кнопки света, шкала)
//! в обычные 183 pt над полосой системного жеста (решение 95) не помещаются. Все вертикали ниже отсчитываются
//! от ВЕРХА белой карточки; проверка [`WhiteCardLayout::validate`] ловит ряды, которые налезают друг на друга,
//! уходят за карточку или заходят в полосу системного жеста снизу.
//!
//! Все числа — `[estimate]`: подгонка по эскизу владельца, не Apple; правятся на телефоне.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::widgets_config::CardLayout;

/// Ошибка раскладки карточки белого.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum WhiteLayoutError {
    /// Число должно быть конечным и > 0.
    #[error("значение раскладки белого {field} должно быть конечным и > 0, получено {value}")]
    NotPositive { field: &'static str, value: f64 },
    /// Число должно быть конечным и >= 0.
    #[error("значение раскладки белого {field} должно быть конечным и >= 0, получено {value}")]
    Negative { field: &'static str, value: f64 },
    /// Число должно быть конечным.
    #[error("значение раскладки белого {field} должно быть конечным, получено {value}")]
    NotFinite { field: &'static str, value: f64 },
    /// Ряд касания ниже минимальной цели касания (HIG: 44 pt).
    #[error("зона касания {row} ниже минимума: {height_pt} pt < {minimum_pt} pt")]
    TouchTooShort {
        row: &'static str,
        height_pt: f64,
        minimum_pt: f64,
    },
    /// Два соседних ряда налезают друг на друга по вертикали.
    #[error(
        "ряды {upper} и {lower} налезают друг на друга: низ верхнего {upper_bottom_pt} pt, верх нижнего {lower_top_pt} pt"
    )]
    RowsOverlap {
        upper: &'static str,
        lower: &'static str,
        upper_bottom_pt: f64,
        lower_top_pt: f64,
    },
    /// Ряд выходит за рамку карточки сверху.
    #[error("ряд {row} выходит за верх карточки: верх {top_pt} pt")]
    RowAboveCard { row: &'static str, top_pt: f64 },
    /// Шкала заходит в полосу системного жеста снизу: касание там принадлежит системе и шкала не двигалась бы.
    #[error(
        "шкала заходит в полосу системного жеста: низ зоны {ruler_bottom_pt} pt от верха, допустимо до {limit_pt} pt"
    )]
    RulerInGestureBand { ruler_bottom_pt: f64, limit_pt: f64 },
    /// Числа осей не помещаются между боковыми кнопками карточки.
    #[error(
        "числа осей не помещаются между боковыми кнопками: нужно {needed_pt} pt от середины, есть {available_pt} pt"
    )]
    AxisButtonsTooWide { needed_pt: f64, available_pt: f64 },
    /// Кнопки света не помещаются в ширину карточки без прокрутки.
    #[error("{count} кнопок по {pitch_pt} pt шире карточки {card_width_pt} pt")]
    LightsTooWide {
        count: usize,
        pitch_pt: f64,
        card_width_pt: f64,
    },
    /// Затвор при белой карточке поднят меньше, чем карточка выросла: он наехал бы на её верх.
    #[error("затвор поднят на {shutter_extra_pt} pt, карточка выросла на {card_extra_pt} pt: наезд")]
    ShutterCollides { shutter_extra_pt: f64, card_extra_pt: f64 },
    /// Размер памяти эха шкалы должен быть не меньше 1.
    #[error("память эха шкалы должна вмещать хотя бы одно значение")]
    EmptyEchoMemory,
}

/// Раскладка карточки белого и числа слежения шкал.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WhiteCardLayout {
    /// На сколько pt белая карточка выше обычной. `[doc]` 24 (WB28): 183,1 → 207,1.
    pub extra_height_pt: f64,
    /// На сколько pt выше обычного подъёма затвор стоит над белой карточкой. `[doc]` 24 (WB28): 100 → 124.
    pub raised_shutter_extra_pt: f64,
    /// Минимальная зона касания, pt. `[doc]` 44: цель касания HIG.
    pub min_touch_pt: f64,
    /// Низ зоны системного жеста снизу, pt от низа экрана. `[estimate]` 53,8 = 132 px телефона владельца при
    /// 2,4545 px/pt (`SystemGestureZones`); шкала обязана лежать ПОЛНОСТЬЮ выше.
    pub system_gesture_band_pt: f64,

    /// Центр строки чисел осей от верха карточки. `[estimate]` 43: на одной линии с «×» (T+43).
    pub axis_row_center_from_top_pt: f64,
    /// Высота зоны касания числа оси. `[estimate]` 44.
    pub axis_touch_height_pt: f64,
    /// Ширина зоны касания числа оси. `[estimate]` 130.
    pub axis_touch_width_pt: f64,
    /// Сдвиг центра числа оси от середины карточки (влево — Кельвины, вправо — оттенок). `[estimate]` 72.
    pub axis_center_offset_pt: f64,
    /// Ширина черты-капсулы под выбранной осью. `[estimate]` 28.
    pub axis_underline_width_pt: f64,
    /// Толщина черты под выбранной осью. `[estimate]` 3.
    pub axis_underline_height_pt: f64,
    /// Сдвиг черты вниз от центра числа оси. `[estimate]` 17.
    pub axis_underline_offset_pt: f64,
    /// Прозрачность невыбранной оси (приглушена). `[estimate]` 0,55.
    pub axis_dim_opacity: f64,

    /// Центр ряда «Auto», «Phone» и кнопок света от верха карточки. `[estimate]` 91.
    pub lights_row_center_from_top_pt: f64,
    /// Высота зоны касания кнопки света. `[estimate]` 50.
    pub lights_touch_height_pt: f64,
    /// Шаг центров кнопок ряда (он же ширина зоны). `[owner]` 60 (WB48): семь кнопок = 420 из 421 pt.
    pub light_pitch_pt: f64,
    /// Сторона значка кнопки света. `[estimate]` 20.
    pub light_icon_size_pt: f64,
    /// Сдвиг значка от центра ряда (вверх — минус). `[estimate]` −13.
    pub light_icon_offset_pt: f64,
    /// Сдвиг подписи от центра ряда. `[estimate]` 6.
    pub light_label_offset_pt: f64,
    /// Сдвиг строки оценки «≈ 3100» от центра ряда: место постоянное, раскладка не прыгает. `[estimate]` 19.
    pub light_estimate_offset_pt: f64,

    /// Центр зоны шкалы от верха карточки. `[estimate]` 138,5: зона 116,5…160,5 при пределе 161,5.
    pub ruler_center_from_top_pt: f64,
    /// Высота зоны шкалы. `[estimate]` 44.
    pub ruler_touch_height_pt: f64,

    /// Допуск «шкала уже стоит на значении», в единицах положения шкалы (майреды / единицы оттенка).
    /// `[estimate]` 0,05: ниже шага риски (5), выше шума пересчёта K ↔ майреды.
    pub follow_tolerance_units: f64,
    /// Допуск «факт — это значение, отправленное нашим пальцем», в единицах положения. `[estimate]` 0,01.
    pub echo_match_tolerance_units: f64,
    /// Сколько миллисекунд отправленное пальцем значение ещё считается ожидающим подтверждения. `[estimate]` 600:
    /// факты отстают на кадр-два; после срока факт, равный старому значению, считается чужой правкой.
    pub echo_memory_ms: f64,
    /// Сколько отправленных значений помнится (защита от разрастания при долгом ведении пальца). `[estimate]` 64.
    pub echo_memory_len: usize,
}

impl Default for WhiteCardLayout {
    fn default() -> Self {
        Self {
            extra_height_pt: 24.0,
            raised_shutter_extra_pt: 24.0,
            min_touch_pt: 44.0,
            system_gesture_band_pt: 53.8,
            axis_row_center_from_top_pt: 43.0,
            axis_touch_height_pt: 44.0,
            axis_touch_width_pt: 130.0,
            axis_center_offset_pt: 72.0,
            axis_underline_width_pt: 28.0,
            axis_underline_height_pt: 3.0,
            axis_underline_offset_pt: 17.0,
            axis_dim_opacity: 0.55,
            lights_row_center_from_top_pt: 91.0,
            lights_touch_height_pt: 50.0,
            light_pitch_pt: 60.0,
            light_icon_size_pt: 20.0,
            light_icon_offset_pt: -13.0,
            light_label_offset_pt: 6.0,
            light_estimate_offset_pt: 19.0,
            ruler_center_from_top_pt: 138.5,
            ruler_touch_height_pt: 44.0,
            follow_tolerance_units: 0.05,
            echo_match_tolerance_units: 0.01,
            echo_memory_ms: 600.0,
            echo_memory_len: 64,
        }
    }
}

fn positive(field: &'static str, value: f64) -> Result<(), WhiteLayoutError> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(WhiteLayoutError::NotPositive { field, value })
    }
}

fn non_negative(field: &'static str, value: f64) -> Result<(), WhiteLayoutError> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(WhiteLayoutError::Negative { field, value })
    }
}

fn finite(field: &'static str, value: f64) -> Result<(), WhiteLayoutError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(WhiteLayoutError::NotFinite { field, value })
    }
}

/// Вертикальный ряд касания: имя, верх и низ (pt от верха карточки).
struct Row {
    name: &'static str,
    top_pt: f64,
    bottom_pt: f64,
}

impl WhiteCardLayout {
    /// Высота белой карточки: обычная плюс `extra_height_pt`.
    pub fn card_height_pt(&self, card: &CardLayout) -> f64 {
        card.height_pt + self.extra_height_pt
    }

    fn rows(&self) -> [Row; 3] {
        let row = |name, center: f64, height: f64| Row {
            name,
            top_pt: center - height * 0.5,
            bottom_pt: center + height * 0.5,
        };
        [
            row(
                "числа осей",
                self.axis_row_center_from_top_pt,
                self.axis_touch_height_pt,
            ),
            row(
                "кнопки света",
                self.lights_row_center_from_top_pt,
                self.lights_touch_height_pt,
            ),
            row("шкала", self.ruler_center_from_top_pt, self.ruler_touch_height_pt),
        ]
    }

    /// Проверка чисел раскладки против обычной карточки (ширина, высота, отступ снизу, боковые кнопки).
    pub fn validate(&self, card: &CardLayout) -> Result<(), WhiteLayoutError> {
        for (field, value) in [
            ("white_card.min_touch_pt", self.min_touch_pt),
            ("white_card.axis_touch_height_pt", self.axis_touch_height_pt),
            ("white_card.axis_touch_width_pt", self.axis_touch_width_pt),
            ("white_card.axis_underline_width_pt", self.axis_underline_width_pt),
            ("white_card.axis_underline_height_pt", self.axis_underline_height_pt),
            ("white_card.lights_touch_height_pt", self.lights_touch_height_pt),
            ("white_card.light_pitch_pt", self.light_pitch_pt),
            ("white_card.light_icon_size_pt", self.light_icon_size_pt),
            ("white_card.ruler_touch_height_pt", self.ruler_touch_height_pt),
            ("white_card.follow_tolerance_units", self.follow_tolerance_units),
            ("white_card.echo_match_tolerance_units", self.echo_match_tolerance_units),
            ("white_card.echo_memory_ms", self.echo_memory_ms),
        ] {
            positive(field, value)?;
        }
        for (field, value) in [
            ("white_card.extra_height_pt", self.extra_height_pt),
            ("white_card.raised_shutter_extra_pt", self.raised_shutter_extra_pt),
            ("white_card.system_gesture_band_pt", self.system_gesture_band_pt),
            ("white_card.axis_center_offset_pt", self.axis_center_offset_pt),
        ] {
            non_negative(field, value)?;
        }
        for (field, value) in [
            (
                "white_card.axis_row_center_from_top_pt",
                self.axis_row_center_from_top_pt,
            ),
            ("white_card.axis_underline_offset_pt", self.axis_underline_offset_pt),
            ("white_card.axis_dim_opacity", self.axis_dim_opacity),
            (
                "white_card.lights_row_center_from_top_pt",
                self.lights_row_center_from_top_pt,
            ),
            ("white_card.light_icon_offset_pt", self.light_icon_offset_pt),
            ("white_card.light_label_offset_pt", self.light_label_offset_pt),
            ("white_card.light_estimate_offset_pt", self.light_estimate_offset_pt),
            ("white_card.ruler_center_from_top_pt", self.ruler_center_from_top_pt),
        ] {
            finite(field, value)?;
        }
        if self.echo_memory_len == 0 {
            return Err(WhiteLayoutError::EmptyEchoMemory);
        }
        if self.raised_shutter_extra_pt < self.extra_height_pt {
            return Err(WhiteLayoutError::ShutterCollides {
                shutter_extra_pt: self.raised_shutter_extra_pt,
                card_extra_pt: self.extra_height_pt,
            });
        }
        self.validate_rows(card)?;
        self.validate_horizontal(card)
    }

    fn validate_rows(&self, card: &CardLayout) -> Result<(), WhiteLayoutError> {
        let rows = self.rows();
        for row in &rows {
            let height_pt = row.bottom_pt - row.top_pt;
            if height_pt < self.min_touch_pt {
                return Err(WhiteLayoutError::TouchTooShort {
                    row: row.name,
                    height_pt,
                    minimum_pt: self.min_touch_pt,
                });
            }
            if row.top_pt < 0.0 {
                return Err(WhiteLayoutError::RowAboveCard {
                    row: row.name,
                    top_pt: row.top_pt,
                });
            }
        }
        for pair in rows.windows(2) {
            if pair[0].bottom_pt > pair[1].top_pt {
                return Err(WhiteLayoutError::RowsOverlap {
                    upper: pair[0].name,
                    lower: pair[1].name,
                    upper_bottom_pt: pair[0].bottom_pt,
                    lower_top_pt: pair[1].top_pt,
                });
            }
        }
        // От верха карточки до границы полосы системного жеста: высота карточки + её отступ от низа экрана − полоса.
        let limit_pt = self.card_height_pt(card) + card.bottom_inset_pt - self.system_gesture_band_pt;
        let ruler = &rows[2];
        if ruler.bottom_pt > limit_pt {
            return Err(WhiteLayoutError::RulerInGestureBand {
                ruler_bottom_pt: ruler.bottom_pt,
                limit_pt,
            });
        }
        Ok(())
    }

    fn validate_horizontal(&self, card: &CardLayout) -> Result<(), WhiteLayoutError> {
        // Между «×» (левая боковая кнопка) и серединой: край её зоны касания до середины карточки.
        let available_pt = card.left_button.offset_from_center_x_pt.abs() - card.button_touch_side_pt * 0.5;
        let needed_pt = self.axis_center_offset_pt + self.axis_touch_width_pt * 0.5;
        if needed_pt > available_pt {
            return Err(WhiteLayoutError::AxisButtonsTooWide {
                needed_pt,
                available_pt,
            });
        }
        Ok(())
    }

    /// Сверка с числом кнопок ряда («Auto» + «Phone» + кнопки света): все видны без прокрутки в ширину карточки.
    pub fn check_buttons_fit(&self, button_count: usize, card: &CardLayout) -> Result<(), WhiteLayoutError> {
        if button_count as f64 * self.light_pitch_pt > card.width_pt {
            return Err(WhiteLayoutError::LightsTooWide {
                count: button_count,
                pitch_pt: self.light_pitch_pt,
                card_width_pt: card.width_pt,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "white_layout_tests.rs"]
mod white_layout_tests;
