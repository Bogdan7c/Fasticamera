//! Подписи прибора поверх картинки (решения 326, 327): буквы целей вектороскопа R, MG, B, CY, G, YL.
//!
//! Положение буквы приходит от рендера (`ScopeLabel::at`, доли картинки, центр текста): охват экрана, масштаб из
//! настроек и зум знает только рендер, поэтому экран ничего не считает, а лишь переводит долю в точку своей картинки и
//! пишет текст. Роль текста — Caption (как числа уровней waveform), цвет — из токенов.
//!
//! # Чужие подписи не рисуются
//! Подписи берутся у `CalibrationState::scope_labels_to_draw`: только посчитанные для ТЕКУЩЕГО запроса прибора. Сразу
//! после свайпа, смены осей или зума рендер ещё не успел опубликовать новые, и старые буквы поверх нового прибора
//! стоять не должны.
//!
//! # Текст внутри картинки
//! Центр текста берётся из подписи, а рамка текста (ширина строки на высоту букв) прижимается внутрь прямоугольника
//! картинки: буква у самого края круга не вылезает на соседнюю панель.

use ui_core::geometry::{Point, Rect};
use ui_kit::tokens::TextRoleKind;

use scope_contract::{ChromaticityGamut, ScopeMark, TargetColor};

use super::paint::NodePaint;
use super::scope_panel::clamp_label_centre;
use crate::widget::Ctx;
use crate::widget::error::WidgetError;
use crate::widget::layers::{DrawLayers, Layer};
use crate::widget::text_runs::TextRuns;

/// Текст подписи. Это язык интерфейса (английский, как «R/G/B» у приборов Resolve), поэтому живёт на экране, а не в
/// словаре рендера. `match` без `_`: новая отметка не соберётся, пока для неё не задан текст.
pub(super) fn mark_text(mark: ScopeMark) -> &'static str {
    match mark {
        ScopeMark::VectorscopeTarget(TargetColor::Red) => "R",
        ScopeMark::VectorscopeTarget(TargetColor::Magenta) => "MG",
        ScopeMark::VectorscopeTarget(TargetColor::Blue) => "B",
        ScopeMark::VectorscopeTarget(TargetColor::Cyan) => "CY",
        ScopeMark::VectorscopeTarget(TargetColor::Green) => "G",
        ScopeMark::VectorscopeTarget(TargetColor::Yellow) => "YL",
        ScopeMark::Gamut(ChromaticityGamut::Srgb) => "sRGB",
        ScopeMark::Gamut(ChromaticityGamut::DisplayP3) => "P3",
        ScopeMark::Gamut(ChromaticityGamut::Rec2020) => "Rec.2020",
        ScopeMark::WhitePoint => "D65",
    }
}

/// Центр текста в сцене: точка подписи `(x, y)` в долях картинки `image`, прижатая так, чтобы рамка текста
/// `2 · half_width × 2 · half_height` (pt) целиком лежала внутри картинки.
pub(super) fn label_centre_in_picture(
    image: Rect,
    fraction: (f32, f32),
    half_width_pt: f64,
    half_height_pt: f64,
) -> Point {
    let wanted_x = image.min_x() + f64::from(fraction.0) * image.size.width;
    let wanted_y = image.min_y() + f64::from(fraction.1) * image.size.height;
    Point::new(
        clamp_label_centre(wanted_x, image.min_x(), image.max_x(), half_width_pt),
        clamp_label_centre(wanted_y, image.min_y(), image.max_y(), half_height_pt),
    )
}

/// Рисует подписи текущего прибора поверх картинки `image` (прямоугольник картинки в сцене). `zone_centre` — центр зоны
/// прибора: кисть узла считает смещения от него.
pub(super) fn draw_scope_labels(
    ctx: &Ctx<'_>,
    paint: &NodePaint,
    layers: &mut DrawLayers,
    texts: &mut TextRuns,
    (image, zone_centre): (Rect, Point),
) -> Result<(), WidgetError> {
    for label in ctx.state.calibration().scope_labels_to_draw() {
        let run = texts.run(ctx, mark_text(label.mark), TextRoleKind::Caption)?;
        let half_width_pt = f64::from(run.advance_px / ctx.px.get()) / 2.0;
        let half_height_pt = f64::from(run.cap_height_px / ctx.px.get()) / 2.0;
        let centre = label_centre_in_picture(image, (label.at.x(), label.at.y()), half_width_pt, half_height_pt);
        layers.push(
            Layer::Content,
            paint.text_centered(
                run,
                centre.x - zone_centre.x,
                centre.y - zone_centre.y,
                ctx.tokens.colors.panel_label,
            ),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ui_core::geometry::Size;

    use super::*;

    #[test]
    fn every_mark_has_the_text_the_decision_names() {
        let targets = [
            (TargetColor::Red, "R"),
            (TargetColor::Magenta, "MG"),
            (TargetColor::Blue, "B"),
            (TargetColor::Cyan, "CY"),
            (TargetColor::Green, "G"),
            (TargetColor::Yellow, "YL"),
        ];
        for (color, text) in targets {
            assert_eq!(mark_text(ScopeMark::VectorscopeTarget(color)), text);
        }
        assert_eq!(mark_text(ScopeMark::Gamut(ChromaticityGamut::DisplayP3)), "P3");
        assert_eq!(mark_text(ScopeMark::Gamut(ChromaticityGamut::Srgb)), "sRGB");
        assert_eq!(mark_text(ScopeMark::Gamut(ChromaticityGamut::Rec2020)), "Rec.2020");
        assert_eq!(mark_text(ScopeMark::WhitePoint), "D65");
    }

    #[test]
    fn the_centre_is_the_fraction_of_the_picture_and_the_text_box_is_pressed_inside_it() {
        let image = Rect::new(Point::new(100.0, 40.0), Size::new(200.0, 200.0));
        let middle = label_centre_in_picture(image, (0.5, 0.25), 6.0, 4.0);
        assert_eq!((middle.x, middle.y), (200.0, 90.0), "доля картинки без прижатия");

        let corner = label_centre_in_picture(image, (1.0, 0.0), 6.0, 4.0);
        assert_eq!(
            (corner.x, corner.y),
            (294.0, 44.0),
            "рамка текста целиком внутри: правый край 300, верх 40"
        );
        let other_corner = label_centre_in_picture(image, (0.0, 1.0), 10.0, 4.0);
        assert_eq!((other_corner.x, other_corner.y), (110.0, 236.0));
    }
}
