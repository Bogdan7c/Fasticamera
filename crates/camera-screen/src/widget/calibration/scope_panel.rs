//! Зона прибора (решения 203, 255, 258, 274): плотная подложка, полоска сверху (`ScopeStrip`), под ней картинка
//! прибора с GPU и в поле слева числа уровней waveform и parade. Горизонтальный свайп (в глазных осях) листает приборы.
//!
//! Картинку просят размером её прямоугольника в пикселях (`CalibrationLayout::scope_image_size_for`), поэтому она
//! кладётся 1:1 в этот прямоугольник без фильтрации: `ImageFit::Stretch`, без обрезки, в системе повёрнутого корня.
//! Прямоугольник считает раскладка (waveform и parade — область без поля уровней, вектороскоп — квадрат во всей
//! области): панель сама рамок не выводит.
//!
//! # Свайп
//! Порог один (решение 280): распознавание пана начинается после `scope_swipe_min_pt`, и признанный свайп листает
//! ровно одну страницу в сторону движения по его концу. Между «признан» и «листает» ничего нет. Листание не
//! зацикливается: за первым и последним прибором свайп ничего не делает (точки-страницы читаются как линейный ряд).

use scope_contract::ScopeKind;
use ui_core::geometry::Rect;
use ui_core::gesture::{GestureData, GestureEvent, GestureState, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_draw::draw_list::{DrawItem, ImageClip, ImageFit, ImageItem, ImageSource, LiveImage, PressState};
use ui_draw::geometry::PxSize;
use ui_kit::tokens::TextRoleKind;
use ui_kit::view::GlassShape;

use super::paint::NodePaint;
use super::scope_strip::ScopeStrip;
use super::space::{EyeAxis, SectionSpace};
use crate::action::ScreenAction;
use crate::calibration::CalibrationLayout;
use crate::widget::error::WidgetError;
use crate::widget::intent::ScreenGesture;
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, Layer, VisualIndex};
use crate::widget::text_runs::TextRuns;
use crate::widget::{Ctx, Outputs, SECTION_BACKING_MATERIAL, single_member_material};

/// Соседний прибор в линейном ряду: `+1` — следующий, `-1` — предыдущий; за краем ряда — `None`.
pub(super) fn neighbour_scope(current: ScopeKind, step: i32) -> Option<ScopeKind> {
    let index = ScopeKind::ALL.iter().position(|kind| *kind == current)?;
    let target = i32::try_from(index).ok()?.checked_add(step)?;
    usize::try_from(target)
        .ok()
        .and_then(|i| ScopeKind::ALL.get(i))
        .copied()
}

/// Зона прибора.
pub(super) struct ScopePanel {
    node: NodeId,
    frame: Rect,
    space: SectionSpace,
    /// Раскладка, под которую построена панель (она в ключе раздела, поэтому не меняется за жизнь панели).
    layout: CalibrationLayout,
    strip: ScopeStrip,
}

impl ScopePanel {
    pub(super) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        space: &SectionSpace,
        (parent, parent_rect): (NodeId, Rect),
        layout: &CalibrationLayout,
    ) -> Result<Self, WidgetError> {
        let frame = space.scene_rect(layout.zones().scope);
        let node = kit.add_node(parent, "calib_scope", frame, parent_rect)?;
        // Распознаватель — на всей зоне (и на полоске, и на картинке): свайп, начатый где угодно в приборе, листает
        // приборы, а не падает на живой видоискатель под разделом. Точки и оси — дети, их тап побеждает пан.
        kit.add_recognizer(
            node,
            RecognizerSpec::Pan(PanSpec {
                direction: space.pan_direction(EyeAxis::Horizontal),
                hysteresis_pt: ctx.config.calibration.widgets.scope_swipe_min_pt,
                ..PanSpec::default()
            }),
            ScreenGesture::CalibrationScopePan,
        )?;
        let strip = ScopeStrip::build(kit, ctx, (node, frame), space.scene_rect(layout.scope_strip()))?;
        Ok(Self {
            node,
            frame,
            space: *space,
            layout: *layout,
            strip,
        })
    }

    /// Жесты зоны: тапы полоски и свайп страниц. `false` — событие не прибора.
    pub(super) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        ctx: &Ctx<'_>,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        if self.strip.handle_gesture(kit, ctx, event, out)? {
            return Ok(true);
        }
        Ok(self.handle_swipe(ctx, event, out))
    }

    /// Свайп решается по концу жеста: сдвиг глазами влево листает к следующему прибору, вправо — к предыдущему.
    /// Отменённый жест ничего не меняет. Порог уже проверил распознаватель (гистерезис = `scope_swipe_min_pt`).
    fn handle_swipe(&mut self, ctx: &Ctx<'_>, event: &GestureEvent<ScreenGesture>, out: &mut Outputs) -> bool {
        if event.intent != ScreenGesture::CalibrationScopePan {
            return false;
        }
        if let (GestureState::Ended, GestureData::Pan { translation, .. }) = (event.state, event.data) {
            let eye_dx = self.space.eye_vector(translation.from_touch_down).dx;
            let step = if eye_dx < 0.0 {
                1
            } else if eye_dx > 0.0 {
                -1
            } else {
                0
            };
            if step != 0
                && let Some(next) = neighbour_scope(ctx.state.calibration().scope_kind(), step)
            {
                out.actions.push(ScreenAction::ScopeKindSelected(next));
            }
        }
        true
    }

    /// Переключатель осей жив только на странице вектороскопа.
    pub(super) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        self.strip.sync(kit, ctx)
    }

    pub(super) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let Some(placement) = visuals.placement(self.node) else {
            return Ok(());
        };
        let config = &ctx.config.calibration.widgets;
        // Подложка под полоской и следом — та же поверхность, что у панелей раздела, но плотнее (решения 257, 263):
        // цвет сцены не подмешивается в цвет следа. Слой — прежний слой панелей: след и подписи поверх.
        let (backing, _host) = single_member_material(
            ctx,
            SECTION_BACKING_MATERIAL,
            GlassShape::RoundedRect {
                corner_radius_pt: config.panel_corner_radius_pt,
            },
            &placement,
            PressState::RELEASED,
            "calib_scope",
        )?;
        layers.push(Layer::Glass, backing);
        self.strip.draw(ctx, visuals, layers, texts)?;
        let Some(paint) = NodePaint::new(ctx, placement, 1.0)? else {
            return Ok(());
        };
        let kind = ctx.state.calibration().scope_kind();
        self.draw_picture(&paint, layers, kind);
        self.draw_level_numbers(ctx, &paint, layers, texts, kind)
    }

    /// Картинка прибора в своём прямоугольнике: размер тот же, что в запросе прибора, поэтому она кладётся 1:1.
    fn draw_picture(&self, paint: &NodePaint, layers: &mut DrawLayers, kind: ScopeKind) {
        let size = self.layout.scope_image_size_for(kind);
        let image_centre = self.space.scene_rect(self.layout.scope_image_rect_for(kind)).center();
        let zone_centre = self.frame.center();
        layers.push(
            Layer::Content,
            DrawItem::Image(ImageItem {
                source: ImageSource::Live(LiveImage::ColorScope),
                transform: paint.at(image_centre.x - zone_centre.x, image_centre.y - zone_centre.y),
                size_px: PxSize::new(size.width() as f32, size.height() as f32),
                fit: ImageFit::Stretch,
                clip: ImageClip::None,
                opacity: paint.opacity(),
            }),
        );
    }

    /// Числа уровней в поле слева, каждое на высоте своей линии сетки (решение 274). Список уровней — из словаря
    /// (`ScopeKind::level_lines`): `scope-model` кладёт линии по нему же, поэтому число стоит у своей линии. У
    /// вектороскопа уровней нет — цикл пуст.
    ///
    /// Подписи крайних уровней (0 и 100) лежат на краях картинки: чтобы не вылезти за неё по вертикали, центр подписи
    /// прижимается внутрь на половину высоты букв. Только по вертикали — горизонтальное место не меняется.
    fn draw_level_numbers(
        &self,
        ctx: &Ctx<'_>,
        paint: &NodePaint,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        kind: ScopeKind,
    ) -> Result<(), WidgetError> {
        let lines = kind.level_lines();
        if lines.is_empty() {
            return Ok(());
        }
        let config = &ctx.config.calibration.widgets;
        let image = self.space.scene_rect(self.layout.scope_image_rect_for(kind));
        let gutter = self.space.scene_rect(self.layout.scope_levels_gutter());
        let zone_centre = self.frame.center();
        let right_edge = gutter.max_x() - config.scope_level_label_inset_pt - zone_centre.x;
        for line in lines {
            let run = texts.run(ctx, line.label, TextRoleKind::Caption)?;
            let half_height_pt = f64::from(run.cap_height_px / ctx.px.get()) / 2.0;
            let line_y = image.min_y() + f64::from(line.nominal_fraction_from_top()) * image.size.height;
            let y = line_y.clamp(image.min_y() + half_height_pt, image.max_y() - half_height_pt) - zone_centre.y;
            layers.push(
                Layer::Content,
                paint.text_right(run, right_edge, y, ctx.tokens.colors.panel_label),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbours_walk_the_linear_row_and_stop_at_the_ends() {
        assert_eq!(neighbour_scope(ScopeKind::Waveform, 1), Some(ScopeKind::RgbParade));
        assert_eq!(neighbour_scope(ScopeKind::RgbParade, 1), Some(ScopeKind::Vectorscope));
        assert_eq!(neighbour_scope(ScopeKind::Vectorscope, 1), None);
        assert_eq!(neighbour_scope(ScopeKind::Waveform, -1), None);
        assert_eq!(neighbour_scope(ScopeKind::Vectorscope, -1), Some(ScopeKind::RgbParade));
    }
}
