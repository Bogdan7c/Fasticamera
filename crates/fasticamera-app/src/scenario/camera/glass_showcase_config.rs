//! Конфиг демо-сцены стекла: раскладка в pt, вид (кегли, цвета, подписи) и общая сборка с проверкой.
//!
//! Все числа сцены живут здесь и в `glass_showcase_timeline` и нигде больше (в коде построения
//! литералов нет). У каждого числа метка происхождения, как в `docs/ios-camera/layout.md`:
//! `[measured]` — замер по видео, `[derived]` — вычислено из замеров, `[estimate]` — «подгонка, не Apple».
//! Это «стенд» материала, а не экран камеры (сессия 12): размеры и места похожи на iOS, набор — нет.

use serde::{Deserialize, Serialize};
use ui_core::time::MotionPreference;
use ui_draw::color::DisplayColor;
use ui_draw::light::TiltLightConfig;

use super::error::ShowcaseConfigError;
use super::timeline::ShowcaseTimeline;

/// Проверка «конечное и > 0».
pub(super) fn require_positive(field: &'static str, value: f64) -> Result<(), ShowcaseConfigError> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(ShowcaseConfigError::NotPositive { field, value })
    }
}

/// Проверка «конечное и >= 0».
pub(super) fn require_non_negative(field: &'static str, value: f64) -> Result<(), ShowcaseConfigError> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(ShowcaseConfigError::Negative { field, value })
    }
}

/// Проверка «конечное и в `min..=max`».
pub(super) fn require_in_range(field: &'static str, value: f64, min: f64, max: f64) -> Result<(), ShowcaseConfigError> {
    if value.is_finite() && (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(ShowcaseConfigError::OutOfRange { field, value, min, max })
    }
}

/// Размер элемента в pt.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShowcaseSizePt {
    pub width_pt: f64,
    pub height_pt: f64,
}

impl ShowcaseSizePt {
    fn validate(&self, field: &'static str) -> Result<(), ShowcaseConfigError> {
        require_positive(field, self.width_pt)?;
        require_positive(field, self.height_pt)
    }
}

/// Раскладка стенда в pt. Низ экрана — опора для всех элементов нижней половины: высота нашего
/// экрана (1080×2412 при 440 pt по ширине ≈ 982,7 pt) выше эталонных 956 pt, поэтому абсолютные
/// `y` из `layout.md` здесь не годятся, хранятся расстояния ОТ НИЗА.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ShowcaseLayout {
    /// Отступ круглых кнопок от боковых краёв, pt. `[measured]` `layout.md` 2.2: переворот камеры
    /// 34,1 справа, миниатюра 33,6 слева; берём 34,1.
    pub edge_margin_pt: f64,
    /// Отступ нижнего ряда (капсулы, круглых кнопок) от низа экрана, pt. `[measured]` 34,1
    /// (низ капсулы, равен нижней безопасной зоне).
    pub bottom_margin_pt: f64,
    /// Диаметр круглых кнопок нижнего ряда, pt. `[measured]` 48,2 / 48,0.
    pub round_button_diameter_pt: f64,
    /// Капсула режимов. `[measured]` 220,0 × 47,8.
    pub mode_capsule: ShowcaseSizePt,
    /// Диаметр кружков зума, pt. `[measured]` 43,8.
    pub zoom_diameter_pt: f64,
    /// Шаг центров кружков зума в сведённом состоянии, pt. `[measured]` ≈ 44,7 (зазоров нет).
    pub zoom_pitch_together_pt: f64,
    /// Шаг центров в разведённом состоянии, pt. `[estimate]` стенда, подгонка, не Apple: зазор ≈ 14 pt,
    /// чтобы капли гарантированно разошлись при радиусе слияния из `ShowcaseLook`.
    pub zoom_pitch_apart_pt: f64,
    /// Центр ряда зума над низом экрана, pt. `[derived]` из `[measured]`: 956 − 706,6 = 249,4.
    pub zoom_center_above_bottom_pt: f64,
    /// Диаметр кнопки меню, pt. `[measured]` 48,3.
    pub menu_button_diameter_pt: f64,
    /// Центр кнопки меню над низом экрана, pt. `[derived]`: 956 − 801,7 = 154,3.
    pub menu_button_center_above_bottom_pt: f64,
    /// Панель меню. `[estimate]` `layout.md` 2.2 (видео B): ≈ 382 × 416.
    pub panel: ShowcaseSizePt,
    /// Отступ панели от правого края, pt. `[estimate]` ≈ 10,5 (боковые отступы шторки 10–11).
    pub panel_edge_margin_pt: f64,
    /// Низ панели над низом экрана, pt. `[estimate]` стенда: над кнопкой меню (её верх на ≈ 178,5 pt
    /// над низом) с зазором ≈ 8 pt. Подгонка, не Apple.
    pub panel_bottom_above_bottom_pt: f64,
    /// Рамка фокуса. `[measured]` `layout.md` 2.2: ≈ 91 × 92.
    pub focus_frame: ShowcaseSizePt,
    /// Центр рамки фокуса ниже верха экрана, pt. `[estimate]` стенда: «точка тапа» выбрана произвольно.
    pub focus_frame_center_below_top_pt: f64,
    /// Плашка AE/AF LOCK. `[measured]` ≈ 117 × 30.
    pub lock_plate: ShowcaseSizePt,
    /// Верх плашки ниже верха экрана, pt. `[estimate]`: «вверху кадра по центру», кадр 16:9 начинается
    /// с 75,7 pt, плашка чуть ниже.
    pub lock_plate_below_top_pt: f64,
}

impl Default for ShowcaseLayout {
    fn default() -> Self {
        Self {
            edge_margin_pt: 34.1,
            bottom_margin_pt: 34.1,
            round_button_diameter_pt: 48.2,
            mode_capsule: ShowcaseSizePt {
                width_pt: 220.0,
                height_pt: 47.8,
            },
            zoom_diameter_pt: 43.8,
            zoom_pitch_together_pt: 44.7,
            zoom_pitch_apart_pt: 58.0,
            zoom_center_above_bottom_pt: 249.4,
            menu_button_diameter_pt: 48.3,
            menu_button_center_above_bottom_pt: 154.3,
            panel: ShowcaseSizePt {
                width_pt: 382.0,
                height_pt: 416.0,
            },
            panel_edge_margin_pt: 10.5,
            panel_bottom_above_bottom_pt: 186.0,
            focus_frame: ShowcaseSizePt {
                width_pt: 91.0,
                height_pt: 92.0,
            },
            focus_frame_center_below_top_pt: 330.0,
            lock_plate: ShowcaseSizePt {
                width_pt: 117.0,
                height_pt: 30.0,
            },
            lock_plate_below_top_pt: 90.0,
        }
    }
}

impl ShowcaseLayout {
    /// Проверить числа: размеры положительны, отступы неотрицательны, разведённый шаг больше сведённого.
    pub fn validate(&self) -> Result<(), ShowcaseConfigError> {
        require_non_negative("layout.edge_margin_pt", self.edge_margin_pt)?;
        require_non_negative("layout.bottom_margin_pt", self.bottom_margin_pt)?;
        require_positive("layout.round_button_diameter_pt", self.round_button_diameter_pt)?;
        self.mode_capsule.validate("layout.mode_capsule")?;
        require_positive("layout.zoom_diameter_pt", self.zoom_diameter_pt)?;
        require_positive("layout.zoom_pitch_together_pt", self.zoom_pitch_together_pt)?;
        require_positive("layout.zoom_pitch_apart_pt", self.zoom_pitch_apart_pt)?;
        require_non_negative("layout.zoom_center_above_bottom_pt", self.zoom_center_above_bottom_pt)?;
        require_positive("layout.menu_button_diameter_pt", self.menu_button_diameter_pt)?;
        require_non_negative(
            "layout.menu_button_center_above_bottom_pt",
            self.menu_button_center_above_bottom_pt,
        )?;
        self.panel.validate("layout.panel")?;
        require_non_negative("layout.panel_edge_margin_pt", self.panel_edge_margin_pt)?;
        require_non_negative("layout.panel_bottom_above_bottom_pt", self.panel_bottom_above_bottom_pt)?;
        self.focus_frame.validate("layout.focus_frame")?;
        require_non_negative(
            "layout.focus_frame_center_below_top_pt",
            self.focus_frame_center_below_top_pt,
        )?;
        self.lock_plate.validate("layout.lock_plate")?;
        require_non_negative("layout.lock_plate_below_top_pt", self.lock_plate_below_top_pt)?;
        if self.zoom_pitch_apart_pt <= self.zoom_pitch_together_pt {
            return Err(ShowcaseConfigError::Inconsistent {
                reason: format!(
                    "разведённый шаг зума {} pt должен быть больше сведённого {} pt",
                    self.zoom_pitch_apart_pt, self.zoom_pitch_together_pt
                ),
            });
        }
        Ok(())
    }

    /// Зазор между соседними кружками зума в сведённом состоянии, pt (может быть < 0 — перекрытие).
    pub fn zoom_gap_together_pt(&self) -> f64 {
        self.zoom_pitch_together_pt - self.zoom_diameter_pt
    }

    /// Зазор между соседними кружками зума в разведённом состоянии, pt.
    pub fn zoom_gap_apart_pt(&self) -> f64 {
        self.zoom_pitch_apart_pt - self.zoom_diameter_pt
    }
}

/// Смещение точки касания в долях диаметра круглой кнопки (центр = 0, ось y вниз).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TouchOffsetFraction {
    pub x: f32,
    pub y: f32,
}

/// Подписи стенда. Английские, как и весь UI; символы обязаны входить в запечённый набор шрифта.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ShowcaseLabels {
    /// Подпись капсулы режимов.
    pub mode: String,
    /// Подписи трёх кружков зума слева направо.
    pub zoom: [String; 3],
    /// Подпись плашки блокировки.
    pub lock: String,
}

impl Default for ShowcaseLabels {
    fn default() -> Self {
        Self {
            mode: "PHOTO".to_owned(),
            zoom: [".5".to_owned(), "1×".to_owned(), "2".to_owned()],
            lock: "AE/AF LOCK".to_owned(),
        }
    }
}

/// Вид стенда: кегли, цвета, скругления, радиус слияния.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ShowcaseLook {
    pub labels: ShowcaseLabels,
    /// Кегль подписи режима, pt. `[estimate]` `layout.md` 2.2: заглавные 14,2 pt → кегль ≈ 20.
    pub mode_label_size_pt: f32,
    /// Кегль подписей зума, pt. `[estimate]` стенда, подгонка, не Apple.
    pub zoom_label_size_pt: f32,
    /// Кегль подписи плашки блокировки, pt. `[estimate]` стенда, подгонка, не Apple.
    pub lock_label_size_pt: f32,
    /// Сторона квадрата иконки, pt. `[estimate]`: около половины диаметра кнопки.
    pub icon_size_pt: f32,
    /// Скругление панели меню, pt (непрерывное). `[estimate]` стенда, подгонка, не Apple.
    pub panel_corner_radius_pt: f32,
    /// Скругление плашки блокировки, pt (непрерывное). `[estimate]`, подгонка, не Apple.
    pub lock_plate_corner_radius_pt: f32,
    /// Радиус слияния кружков зума, pt. `[estimate]`, подгонка, не Apple. Условие: слитыми остаются
    /// сведённые (`2·зазор_сведённый < радиус`) и расходятся разведённые (`радиус < 2·зазор_разведённый`) —
    /// следует из формулы smooth-min: в середине зазора поле равно `зазор/2 − радиус/4`.
    pub zoom_merge_radius_pt: f64,
    /// Точка касания круглых кнопок при нажатии, доли диаметра. `[estimate]`: чуть правее-выше центра,
    /// чтобы свечение было видно смещённым.
    pub press_touch_offset: TouchOffsetFraction,
    /// Толщина обводки рамки фокуса, pt. `[estimate]`: «тонкая линия».
    pub focus_stroke_width_pt: f32,
    /// Жёлтый фокуса и плашки. `[estimate]`: системный жёлтый iOS ≈ #FFCC00.
    pub focus_yellow: DisplayColor,
    /// Цвет подписей и иконок поверх стекла. `[estimate]`: белый.
    pub glass_foreground: DisplayColor,
    /// Цвет подписи на жёлтой плашке. `[estimate]`: «тёмный текст» (`layout.md` 2.2).
    pub lock_label_color: DisplayColor,
}

impl Default for ShowcaseLook {
    fn default() -> Self {
        Self {
            labels: ShowcaseLabels::default(),
            mode_label_size_pt: 20.0,
            zoom_label_size_pt: 15.0,
            lock_label_size_pt: 13.0,
            icon_size_pt: 24.0,
            panel_corner_radius_pt: 44.0,
            lock_plate_corner_radius_pt: 8.0,
            zoom_merge_radius_pt: 12.0,
            press_touch_offset: TouchOffsetFraction { x: 0.2, y: -0.2 },
            focus_stroke_width_pt: 1.5,
            focus_yellow: DisplayColor::rgb(1.0, 0.8, 0.0),
            glass_foreground: DisplayColor::WHITE,
            lock_label_color: DisplayColor::rgb(0.1, 0.1, 0.1),
        }
    }
}

impl ShowcaseLook {
    /// Проверить числа, цвета и подписи.
    pub fn validate(&self) -> Result<(), ShowcaseConfigError> {
        for (field, label) in [
            ("look.labels.mode", &self.labels.mode),
            ("look.labels.zoom[0]", &self.labels.zoom[0]),
            ("look.labels.zoom[1]", &self.labels.zoom[1]),
            ("look.labels.zoom[2]", &self.labels.zoom[2]),
            ("look.labels.lock", &self.labels.lock),
        ] {
            if label.is_empty() {
                return Err(ShowcaseConfigError::EmptyLabel { field });
            }
        }
        for (field, value) in [
            ("look.mode_label_size_pt", self.mode_label_size_pt),
            ("look.zoom_label_size_pt", self.zoom_label_size_pt),
            ("look.lock_label_size_pt", self.lock_label_size_pt),
            ("look.icon_size_pt", self.icon_size_pt),
            ("look.focus_stroke_width_pt", self.focus_stroke_width_pt),
        ] {
            require_positive(field, f64::from(value))?;
        }
        require_non_negative("look.panel_corner_radius_pt", f64::from(self.panel_corner_radius_pt))?;
        require_non_negative(
            "look.lock_plate_corner_radius_pt",
            f64::from(self.lock_plate_corner_radius_pt),
        )?;
        require_positive("look.zoom_merge_radius_pt", self.zoom_merge_radius_pt)?;
        require_in_range(
            "look.press_touch_offset.x",
            f64::from(self.press_touch_offset.x),
            -0.5,
            0.5,
        )?;
        require_in_range(
            "look.press_touch_offset.y",
            f64::from(self.press_touch_offset.y),
            -0.5,
            0.5,
        )?;
        for (field, color) in [
            ("look.focus_yellow", self.focus_yellow),
            ("look.glass_foreground", self.glass_foreground),
            ("look.lock_label_color", self.lock_label_color),
        ] {
            if !color.is_valid() {
                return Err(ShowcaseConfigError::InvalidColor { field });
            }
        }
        Ok(())
    }
}

/// Конфиг демо-сцены стекла; станет секцией конфига сценария `glass`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct GlassShowcaseConfig {
    /// Раскладка в pt.
    pub layout: ShowcaseLayout,
    /// Сценарий времени: период цикла и окна «нажато / разведено / раскрыто».
    pub timeline: ShowcaseTimeline,
    /// Вид: кегли, цвета, скругления.
    pub look: ShowcaseLook,
    /// Свет от наклона (используется, только если передан ящик гравитации).
    pub tilt_light: TiltLightConfig,
    /// Reduce Motion: пружины без перелёта.
    pub motion: MotionPreference,
}

impl GlassShowcaseConfig {
    /// Проверить конфиг целиком, включая согласование раскладки и радиуса слияния.
    pub fn validate(&self) -> Result<(), ShowcaseConfigError> {
        self.layout.validate()?;
        self.timeline.validate()?;
        self.look.validate()?;
        self.tilt_light.validate()?;
        // Слияние капель: в середине зазора поле smooth-min равно `зазор/2 − радиус/4` (см. `ShowcaseLook`).
        let merge = self.look.zoom_merge_radius_pt;
        if 2.0 * self.layout.zoom_gap_together_pt() >= merge || merge >= 2.0 * self.layout.zoom_gap_apart_pt() {
            return Err(ShowcaseConfigError::Inconsistent {
                reason: format!(
                    "радиус слияния {merge} pt должен лежать строго между 2·зазором сведённых ({}) и 2·зазором разведённых ({}) кружков зума",
                    2.0 * self.layout.zoom_gap_together_pt(),
                    2.0 * self.layout.zoom_gap_apart_pt()
                ),
            });
        }
        Ok(())
    }
}
