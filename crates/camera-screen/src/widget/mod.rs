//! Виджеты экрана Photo: каждый — сборка элементов `ui-kit` (решение 84) со своими узлами сцены.
//!
//! Виджет делает пять вещей и больше ничего:
//! 1. `build` — создаёт узлы и распознаватели жестов в [`SceneKit`];
//! 2. `handle_gesture` — переводит событие жеста в [`ScreenAction`] (или в вызов элемента `ui-kit`);
//! 3. `sync` — выравнивает элементы по [`ScreenState`] (программно, без эха действий: значение принадлежит
//!    состоянию экрана, элемент отвечает только за физику);
//! 4. `advance` — продвигает элементы по кадру и забирает их события (хаптика, смена значения);
//! 5. `draw` — строит `DrawItem` через `ui_kit::view` в нужный слой.
//!
//! Своих жестов, физики и рисования примитивов здесь нет там, где их даёт `ui-kit`; чего не хватило —
//! помечено `TODO(ui-kit)` и перечислено в отчёте пакета.

pub(crate) mod bottom_row;
pub(crate) mod button;
pub(crate) mod calibration;
pub(crate) mod capture_blink;
pub(crate) mod card_geometry;
pub(crate) mod chip_row;
pub(crate) mod controls_drawer;
pub(crate) mod countdown;
pub(crate) mod error;
pub(crate) mod exposure_link;
pub(crate) mod exposure_ruler;
pub(crate) mod flip_button;
pub(crate) mod focus_indicator;
pub(crate) mod frame_mask;
pub(crate) mod geometry;
pub(crate) mod grid_overlay;
pub(crate) mod intent;
pub(crate) mod kit;
pub(crate) mod layers;
pub(crate) mod level_indicator;
pub(crate) mod lock_banner;
pub(crate) mod menu_button;
pub(crate) mod mode_carousel;
pub(crate) mod problem_banner;
pub(crate) mod sheet_view;
pub(crate) mod shutter_button;
pub(crate) mod shutter_row;
pub(crate) mod side_button;
pub(crate) mod text_runs;
pub(crate) mod thumbnail;
pub(crate) mod tool_card;
pub(crate) mod tool_card_content;
pub(crate) mod tool_card_options;
pub(crate) mod tool_card_styles;
pub(crate) mod top_bar;
pub(crate) mod transition_veil;
pub(crate) mod unavailable_notice;
pub(crate) mod white_attention;
pub(crate) mod white_button;
pub(crate) mod white_card;
pub(crate) mod white_card_row;
pub(crate) mod white_follow;
pub(crate) mod white_geometry;
pub(crate) mod white_ruler;
pub(crate) mod zoom_strip;
pub(crate) mod zoom_wheel;
pub(crate) mod zoom_wheel_model;

use shell_contract::clock::MonotonicNanos;
use shell_contract::haptics::HapticPattern;
use ui_core::spring::Spring;
use ui_core::time::MotionPreference;
use ui_kit::environment::ElementEnvironment;
use ui_kit::tokens::{MotionToken, UiKitTokens};
use ui_kit::view::PxPerPt;

use crate::action::ScreenAction;
use crate::config::ScreenConfig;
use crate::state::ScreenState;
use error::{WidgetError, element};
use geometry::ScreenGeometry;

/// Всё, что виджету нужно знать о кадре. Копируемая связка ссылок: фасад собирает её на каждый вызов.
#[derive(Clone, Copy)]
pub(crate) struct Ctx<'a> {
    pub state: &'a ScreenState,
    pub config: &'a ScreenConfig,
    pub tokens: &'a UiKitTokens,
    pub motion: MotionPreference,
    pub geometry: &'a ScreenGeometry,
    pub px: PxPerPt,
    /// Момент, на который строится вызов: время кадра или (для события жеста) время события.
    pub now: MonotonicNanos,
}

impl<'a> Ctx<'a> {
    /// Окружение элемента `ui-kit`; `viewport_extent_points` — протяжённость области вдоль оси элемента.
    pub(crate) fn environment(&self, viewport_extent_points: f64) -> ElementEnvironment<'a> {
        ElementEnvironment {
            tokens: self.tokens,
            motion: self.motion,
            viewport_extent_points,
        }
    }

    /// Пружина по имени токена с учётом Reduce Motion.
    pub(crate) fn spring(&self, token: MotionToken) -> Result<Spring, WidgetError> {
        self.tokens
            .motion
            .spring(token, self.motion)
            .map_err(element("токены движения"))
    }
}

/// Что виджеты просят у фасада: действия над состоянием и хаптика (идёт эффектом наружу).
#[derive(Debug, Default)]
pub(crate) struct Outputs {
    pub actions: Vec<ScreenAction>,
    pub haptics: Vec<HapticPattern>,
}

use ui_draw::draw_list::{GlassGroupItem, PressState};
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::transform::Affine2;
use ui_kit::tokens::GlassRole;
use ui_kit::view::{GlassGroupBuilder, GlassShape, member_transform, opacity_of_placement};

/// Группа стекла из одного члена и преобразование, в котором ставится его содержимое (с масштабом нажатия).
/// Прозрачность группы — прозрачность узла (перемноженная с предками), поэтому растворение узла растворяет стекло.
pub(crate) fn single_member_group(
    ctx: &Ctx<'_>,
    role: GlassRole,
    shape: GlassShape,
    placement: &NodePlacementPx,
    press: PressState,
    what: &'static str,
) -> Result<(GlassGroupItem, Affine2), WidgetError> {
    let mut builder = GlassGroupBuilder::new(ctx.tokens.glass.style_for(role), ctx.px);
    let member = builder.add_member(shape, *placement, press)?;
    let group = builder.build(1.0, opacity_of_placement(placement, what)?)?;
    let host = member_transform(&group, member)?;
    Ok((group, host))
}

/// Хост значка внутри кнопки: поворот узла значка относительно кнопки, затем преобразование члена группы.
///
/// Узел значка — ребёнок узла кнопки на всю её рамку, поэтому его размещение отличается от размещения кнопки
/// только поворотом; чтобы не потерять масштаб нажатия члена стекла, поворот берётся как отношение
/// преобразований (`значок ∘ кнопка⁻¹`) и ставится поверх `member_host`.
pub(crate) fn rotation_only(
    icon: &NodePlacementPx,
    button: &NodePlacementPx,
    member_host: ui_draw::transform::Affine2,
) -> ui_draw::transform::Affine2 {
    match button.transform.inverse() {
        Ok(inverse) => icon.transform.then(inverse).then(member_host),
        Err(_) => member_host,
    }
}

use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, ShapeItem};
use ui_draw::shape::ShapeGeometry;

use ui_core::geometry::Rect;
use ui_draw::shape::CornerSpec;

/// Закрашенный прямоугольник по рамке в pt (абсолютные координаты экрана). Для подложек, линий сетки и затемнения.
pub(crate) fn rect_item(rect: Rect, color: DisplayColor, opacity: Opacity, px: PxPerPt) -> DrawItem {
    let center = rect.center();
    DrawItem::Shape(ShapeItem {
        geometry: ShapeGeometry::RoundedRect {
            size: ui_draw::geometry::PxSize::new(px.px(rect.size.width), px.px(rect.size.height)),
            corner: CornerSpec::circular(0.0),
        },
        transform: Affine2::translate(px.px(center.x), px.px(center.y)),
        fill: color,
        stroke: None,
        shadow: None,
        opacity,
    })
}

#[cfg(test)]
#[path = "widget_tests.rs"]
mod widget_tests;
