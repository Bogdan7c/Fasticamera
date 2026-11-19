//! Построение списка рисования стенда из элементов сцены: единственное место, где «что где стоит» (pt, `ui-core`)
//! превращается в `DrawList` (px, `ui-draw`).
//!
//! Перевод геометрии делает `ui_draw::scene_bridge::node_placement_px` — здесь ничего не пересчитывается
//! вручную. Порядок элементов фиксирован (число элементов в списке не зависит от времени):
//! 1) не стекло под стеклом (рамка фокуса, плашка и её подпись);
//! 2) группы стекла (капсула, две кнопки, кружки зума, кнопка меню, панель);
//! 3) подписи и иконки поверх стекла — они едут вместе со своим членом группы, включая масштаб нажатия.

use ui_core::id::NodeId;
use ui_core::tree::NodeVisual;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{
    DrawItem, DrawList, GlassGroupItem, GlassMember, IconItem, PressState, ShapeItem, StrokeStyle, TextItem,
};
use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::glass::GlassStyle;
use ui_draw::icons::IconId;
use ui_draw::light::LightDirection;
use ui_draw::scene_bridge::{NodePlacementPx, node_placement_px};
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_draw::text::tracking::sf_pro_tracking_em;
use ui_draw::text::{Figures, FontFace, TextError, TextRun, TextStyle, layout_line};
use ui_draw::transform::Affine2;

use super::config::{ShowcaseLook, TouchOffsetFraction};
use super::error::ShowcaseError;
use super::motion::PressAmounts;
use super::scene::ShowcaseNodes;

/// Пресеты стекла: кнопочный и панельный (из токенов `ui-kit`).
///
/// Капсула режимов (220 × 47,8 pt) берёт кнопочный пресет: его «доли высоты» (зона линзы 25 %, сила 50 %)
/// от меньшей стороны дают для 47,8 pt ту же толщину стекла, что у круглых кнопок рядом, а панельный
/// пресет рассчитан на крупные формы (меню, листы; `glass.md` раздел 6, `GlassRole::Button` — «≈ 44–48 pt»).
#[derive(Debug, Clone)]
pub(super) struct ShowcaseStyles {
    pub button: GlassStyle,
    pub panel: GlassStyle,
}

/// Свёрстанные подписи (меняются только при смене `px_per_pt`, поэтому строятся один раз на сцену).
#[derive(Debug, Clone)]
pub(super) struct ShowcaseTexts {
    mode: TextRun,
    zoom: [TextRun; 3],
    lock: TextRun,
}

fn semibold_style(size_pt: f32, px_per_pt: f32, figures: Figures) -> TextStyle {
    TextStyle {
        face: FontFace::Semibold,
        size_px: size_pt * px_per_pt,
        // Трекинг SF Pro по кеглю — стартовая точка подгонки (`assets.md` 3), не хардкод вёрстки.
        tracking_em: sf_pro_tracking_em(size_pt),
        figures,
    }
}

impl ShowcaseTexts {
    /// Свёрстать все подписи; символ вне запечённого набора — ошибка, а не «тофу».
    pub(super) fn layout(look: &ShowcaseLook, px_per_pt: f32) -> Result<Self, TextError> {
        let mode_style = semibold_style(look.mode_label_size_pt, px_per_pt, Figures::Proportional);
        // Цифры зума — табличные: подпись не дёргается при смене цифры (так делает iOS).
        let zoom_style = semibold_style(look.zoom_label_size_pt, px_per_pt, Figures::Tabular);
        let lock_style = semibold_style(look.lock_label_size_pt, px_per_pt, Figures::Proportional);
        Ok(Self {
            mode: layout_line(&look.labels.mode, &mode_style)?,
            zoom: [
                layout_line(&look.labels.zoom[0], &zoom_style)?,
                layout_line(&look.labels.zoom[1], &zoom_style)?,
                layout_line(&look.labels.zoom[2], &zoom_style)?,
            ],
            lock: layout_line(&look.labels.lock, &lock_style)?,
        })
    }
}

/// Что меняется от кадра к кадру.
#[derive(Debug, Clone, Copy)]
pub(super) struct FrameFacts {
    pub press: PressAmounts,
    /// Раскрытие панели 0..1.
    pub panel_progress: f32,
    pub light: LightDirection,
}

/// Всё неизменное для построения кадра.
pub(super) struct DrawContext<'a> {
    pub look: &'a ShowcaseLook,
    pub styles: &'a ShowcaseStyles,
    pub texts: &'a ShowcaseTexts,
    pub px_per_pt: f32,
}

/// Размещения узлов в px, найденные по идентификатору.
struct Placements {
    entries: Vec<(NodeId, NodePlacementPx)>,
}

impl Placements {
    fn collect(visuals: &[NodeVisual], px_per_pt: f32) -> Self {
        Self {
            entries: visuals
                .iter()
                .map(|visual| (visual.node, node_placement_px(visual, f64::from(px_per_pt))))
                .collect(),
        }
    }

    fn require(&self, node: NodeId, label: &'static str) -> Result<NodePlacementPx, ShowcaseError> {
        self.entries
            .iter()
            .find(|(candidate, _)| *candidate == node)
            .map(|(_, placement)| *placement)
            .ok_or(ShowcaseError::MissingVisual { label })
    }
}

fn opacity_of(label: &'static str, value: f32) -> Result<Opacity, ShowcaseError> {
    Opacity::new(value).map_err(|_| ShowcaseError::BadOpacity { label, value })
}

/// Нажатие члена группы: сила и смещение точки касания.
#[derive(Clone, Copy)]
struct PressRequest {
    amount: f32,
    touch_offset: TouchOffsetFraction,
}

impl PressRequest {
    /// Член, который не нажимается.
    const NONE: PressRequest = PressRequest {
        amount: 0.0,
        touch_offset: TouchOffsetFraction { x: 0.0, y: 0.0 },
    };
}

/// Член группы до сборки.
struct MemberInput {
    geometry: ShapeGeometry,
    placement: NodePlacementPx,
    press: PressRequest,
}

/// Общие числа группы.
struct GroupSpec<'a> {
    style: &'a GlassStyle,
    px_per_pt: f32,
    merge_radius_px: f32,
    effect: f32,
    opacity: Opacity,
}

/// Размер элемента на экране в px с учётом масштаба его преобразования (раскрытие панели).
/// Параметры стекла считаются от него, чтобы свёрнутая панель не получила линзу в 24 pt на форме в 48 pt.
fn effective_size_px(placement: &NodePlacementPx) -> PxSize {
    let transform = &placement.transform;
    // Образы осей x и y в строковой записи `Affine2`: (a, c) и (b, d).
    let scale_x = transform.a.hypot(transform.c);
    let scale_y = transform.b.hypot(transform.d);
    PxSize::new(placement.size.width * scale_x, placement.size.height * scale_y)
}

/// Собрать группу стекла. Масштаб нажатия запекается в преобразование члена здесь (рендер его не добавляет).
fn glass_group(spec: &GroupSpec<'_>, inputs: &[MemberInput]) -> GlassGroupItem {
    let reference_size = inputs
        .first()
        .map_or(PxSize::new(0.0, 0.0), |input| effective_size_px(&input.placement));
    let params = spec.style.resolve(reference_size, spec.px_per_pt);
    let members = inputs
        .iter()
        .map(|input| {
            let press_scale = params.press.scale_for(input.press.amount);
            let side = input.geometry.extent().min_side();
            GlassMember {
                geometry: input.geometry,
                transform: Affine2::uniform_scale(press_scale).then(input.placement.transform),
                press: PressState {
                    amount: input.press.amount,
                    touch_point_local: PxPoint::new(
                        input.press.touch_offset.x * side,
                        input.press.touch_offset.y * side,
                    ),
                },
            }
        })
        .collect();
    GlassGroupItem {
        params,
        members,
        merge_radius_px: spec.merge_radius_px,
        effect: spec.effect,
        opacity: spec.opacity,
    }
}

/// Подпись по центру формы: начало строки — левый конец базовой линии, поэтому сдвиг на половину ширины
/// влево и половину высоты заглавных вниз, затем преобразование члена.
fn centered_text(run: &TextRun, member_transform: Affine2, color: DisplayColor, opacity: Opacity) -> DrawItem {
    let to_baseline_origin = Affine2::translate(-run.advance_px * 0.5, run.cap_height_px * 0.5);
    DrawItem::Text(TextItem {
        run: run.clone(),
        transform: to_baseline_origin.then(member_transform),
        color,
        opacity,
    })
}

fn centered_icon(
    icon: IconId,
    size_pt: f32,
    px_per_pt: f32,
    member_transform: Affine2,
    color: DisplayColor,
    opacity: Opacity,
) -> DrawItem {
    let side_px = size_pt * px_per_pt;
    DrawItem::Icon(IconItem {
        icon,
        transform: member_transform,
        size_px: PxSize::new(side_px, side_px),
        color,
        opacity,
    })
}

/// Круглый член группы по размещению узла.
fn circle_member(placement: NodePlacementPx, press: PressRequest) -> MemberInput {
    MemberInput {
        geometry: ShapeGeometry::Circle {
            diameter: placement.size.width,
        },
        placement,
        press,
    }
}

/// Преобразование первого члена группы (на него садятся подпись и иконка).
fn first_member_transform(group: &GlassGroupItem) -> Affine2 {
    group
        .members
        .first()
        .map_or(Affine2::IDENTITY, |member| member.transform)
}

/// Построить список рисования кадра.
pub(super) fn build_draw_list(
    ctx: &DrawContext<'_>,
    nodes: &ShowcaseNodes,
    visuals: &[NodeVisual],
    facts: &FrameFacts,
) -> Result<DrawList, ShowcaseError> {
    let look = ctx.look;
    let px_per_pt = ctx.px_per_pt;
    let placements = Placements::collect(visuals, px_per_pt);
    let mut items: Vec<DrawItem> = Vec::with_capacity(16);

    // 1. Не стекло: рамка фокуса и плашка блокировки (проверка рисования обычных фигур и текста).
    let focus = placements.require(nodes.focus_frame, "focus_frame")?;
    items.push(DrawItem::Shape(ShapeItem {
        geometry: ShapeGeometry::RoundedRect {
            size: focus.size,
            corner: CornerSpec::circular(0.0),
        },
        transform: focus.transform,
        fill: DisplayColor::TRANSPARENT,
        stroke: Some(StrokeStyle {
            width_px: look.focus_stroke_width_pt * px_per_pt,
            color: look.focus_yellow,
        }),
        shadow: None,
        opacity: opacity_of("focus_frame", focus.opacity)?,
    }));
    let plate = placements.require(nodes.lock_plate, "lock_plate")?;
    let plate_opacity = opacity_of("lock_plate", plate.opacity)?;
    items.push(DrawItem::Shape(ShapeItem {
        geometry: ShapeGeometry::RoundedRect {
            size: plate.size,
            corner: CornerSpec::continuous(look.lock_plate_corner_radius_pt * px_per_pt),
        },
        transform: plate.transform,
        fill: look.focus_yellow,
        stroke: None,
        shadow: None,
        opacity: plate_opacity,
    }));
    items.push(centered_text(
        &ctx.texts.lock,
        plate.transform,
        look.lock_label_color,
        plate_opacity,
    ));

    // 2. Стекло. Капсула режимов.
    let button_group = |merge_radius_px: f32, effect: f32, opacity: Opacity| GroupSpec {
        style: &ctx.styles.button,
        px_per_pt,
        merge_radius_px,
        effect,
        opacity,
    };
    let capsule = placements.require(nodes.mode_capsule, "mode_capsule")?;
    let capsule_opacity = opacity_of("mode_capsule", capsule.opacity)?;
    let capsule_group = glass_group(
        &button_group(0.0, 1.0, capsule_opacity),
        &[MemberInput {
            geometry: ShapeGeometry::Capsule { size: capsule.size },
            placement: capsule,
            press: PressRequest::NONE,
        }],
    );

    // Две круглые кнопки: у каждой своя группа (они на разных краях экрана, общей группе нечего сливать).
    let flash = placements.require(nodes.flash_button, "flash_button")?;
    let flash_opacity = opacity_of("flash_button", flash.opacity)?;
    let flash_group = glass_group(
        &button_group(0.0, 1.0, flash_opacity),
        &[circle_member(
            flash,
            PressRequest {
                amount: facts.press.flash,
                touch_offset: look.press_touch_offset,
            },
        )],
    );
    let flip = placements.require(nodes.flip_button, "flip_button")?;
    let flip_opacity = opacity_of("flip_button", flip.opacity)?;
    let flip_group = glass_group(
        &button_group(0.0, 1.0, flip_opacity),
        &[circle_member(
            flip,
            PressRequest {
                amount: facts.press.flip,
                touch_offset: look.press_touch_offset,
            },
        )],
    );

    // Три кружка зума в ОДНОЙ группе: сходятся и расходятся — сливаются «как капли».
    let zoom_placements = [
        placements.require(nodes.zoom[0], "zoom_left")?,
        placements.require(nodes.zoom[1], "zoom_middle")?,
        placements.require(nodes.zoom[2], "zoom_right")?,
    ];
    let zoom_opacity = opacity_of("zoom_middle", zoom_placements[1].opacity)?;
    let zoom_inputs: Vec<MemberInput> = zoom_placements
        .iter()
        .map(|placement| circle_member(*placement, PressRequest::NONE))
        .collect();
    let zoom_group = glass_group(
        &button_group(look.zoom_merge_radius_pt as f32 * px_per_pt, 1.0, zoom_opacity),
        &zoom_inputs,
    );

    // Кнопка меню: пока панель раскрывается, плавно гаснет (панель вырастает из неё). Это перекрёстное
    // растворение источника, а не «появление панели прозрачностью»: у панели растёт именно `effect`.
    let menu = placements.require(nodes.menu_button, "menu_button")?;
    let menu_opacity = opacity_of("menu_button", menu.opacity * (1.0 - facts.panel_progress))?;
    let menu_group = glass_group(
        &button_group(0.0, 1.0, menu_opacity),
        &[circle_member(menu, PressRequest::NONE)],
    );

    // Панель-морф: непрерывное скругление, материализация `effect` 0 → 1 (Apple 219, 2:55).
    let panel = placements.require(nodes.panel, "panel")?;
    let panel_group = glass_group(
        &GroupSpec {
            style: &ctx.styles.panel,
            px_per_pt,
            merge_radius_px: 0.0,
            effect: facts.panel_progress,
            opacity: opacity_of("panel", panel.opacity)?,
        },
        &[MemberInput {
            geometry: ShapeGeometry::RoundedRect {
                size: panel.size,
                corner: CornerSpec::continuous(look.panel_corner_radius_pt * px_per_pt),
            },
            placement: panel,
            press: PressRequest::NONE,
        }],
    );

    // 3. Подписи и иконки поверх стекла — до того, как группы уйдут в список (нужны их преобразования).
    let foreground = look.glass_foreground;
    let mut overlays: Vec<DrawItem> = Vec::with_capacity(7);
    overlays.push(centered_text(
        &ctx.texts.mode,
        first_member_transform(&capsule_group),
        foreground,
        capsule_opacity,
    ));
    overlays.push(centered_icon(
        IconId::FlashOn,
        look.icon_size_pt,
        px_per_pt,
        first_member_transform(&flash_group),
        foreground,
        flash_opacity,
    ));
    overlays.push(centered_icon(
        IconId::FlipCamera,
        look.icon_size_pt,
        px_per_pt,
        first_member_transform(&flip_group),
        foreground,
        flip_opacity,
    ));
    for (run, member) in ctx.texts.zoom.iter().zip(&zoom_group.members) {
        overlays.push(centered_text(run, member.transform, foreground, zoom_opacity));
    }
    overlays.push(centered_icon(
        IconId::MenuChevron,
        look.icon_size_pt,
        px_per_pt,
        first_member_transform(&menu_group),
        foreground,
        menu_opacity,
    ));

    for group in [
        capsule_group,
        flash_group,
        flip_group,
        zoom_group,
        menu_group,
        panel_group,
    ] {
        items.push(DrawItem::Glass(group));
    }
    items.extend(overlays);

    let mut list = DrawList::new(facts.light);
    list.items = items;
    Ok(list)
}
