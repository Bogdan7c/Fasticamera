//! Сцена демо-стекла на `ui_core::scene::UiScene`: узлы, вёрстка в pt, привязка к низу, движение пружинами.
//!
//! Модуль владеет деревом и знает, КАКИЕ свойства каких узлов ездят (разведение кружков зума,
//! раскрытие панели). Что рисовать и какими числами — не его дело (`glass_showcase_build`), КОГДА ехать —
//! тоже (`glass_showcase_motion`). Нажатие кнопок в сцене не живёт: это не геометрия узла, а число
//! `PressState.amount`, которое строитель переводит в масштаб члена группы.

use ui_core::animator::{AnimatedProperty, AnimationRequest, PropertyKey, StartVelocity};
use ui_core::geometry::{Insets, Rect};
use ui_core::id::NodeId;
use ui_core::layout::{AnchorRule, AxisPlacement, Placement, ReferenceArea};
use ui_core::scene::{SceneSettings, SurfaceGeometry, UiScene};
use ui_core::spring::Spring;
use ui_core::time::{MonotonicNanos, MotionPreference};
use ui_core::tree::NodeVisual;

use super::config::{ShowcaseLayout, ShowcaseSizePt};
use super::error::ShowcaseError;

/// Узлы сцены по ролям.
#[derive(Debug, Clone, Copy)]
pub(super) struct ShowcaseNodes {
    pub focus_frame: NodeId,
    pub lock_plate: NodeId,
    pub mode_capsule: NodeId,
    pub flash_button: NodeId,
    pub flip_button: NodeId,
    /// Кружки зума слева направо.
    pub zoom: [NodeId; 3],
    pub menu_button: NodeId,
    pub panel: NodeId,
}

/// Поза свёрнутой панели: где она «сидит» внутри кнопки меню.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PanelPose {
    /// Равномерный масштаб свёрнутой панели (панель целиком помещается в кнопку меню).
    pub scale: f64,
    /// Сдвиг центра панели в центр кнопки меню, pt.
    pub shift_x: f64,
    pub shift_y: f64,
}

/// Сцена стенда.
pub(super) struct ShowcaseScene {
    scene: UiScene<()>,
    nodes: ShowcaseNodes,
    layout: ShowcaseLayout,
    panel_collapsed: PanelPose,
}

/// Привязка по горизонтали: к левому краю, к правому или по центру.
enum Horizontal {
    Start(f64),
    End(f64),
    CenterOffset(f64),
}

/// Привязка по вертикали: от верха или от низа.
enum Vertical {
    FromTop(f64),
    FromBottom(f64),
}

/// Якорное размещение элемента заданного размера в рамке родителя (без безопасной зоны).
fn anchored(horizontal: Horizontal, vertical: Vertical, size: ShowcaseSizePt) -> Placement {
    let horizontal = match horizontal {
        Horizontal::Start(inset) => AxisPlacement::Start {
            inset,
            length: size.width_pt,
        },
        Horizontal::End(inset) => AxisPlacement::End {
            inset,
            length: size.width_pt,
        },
        Horizontal::CenterOffset(offset) => AxisPlacement::Center {
            offset,
            length: size.width_pt,
        },
    };
    let vertical = match vertical {
        Vertical::FromTop(inset) => AxisPlacement::Start {
            inset,
            length: size.height_pt,
        },
        Vertical::FromBottom(inset) => AxisPlacement::End {
            inset,
            length: size.height_pt,
        },
    };
    Placement::Anchored(AnchorRule {
        area: ReferenceArea::Frame,
        horizontal,
        vertical,
    })
}

fn square(diameter_pt: f64) -> ShowcaseSizePt {
    ShowcaseSizePt {
        width_pt: diameter_pt,
        height_pt: diameter_pt,
    }
}

impl ShowcaseScene {
    /// Построить сцену под поверхность: узлы, привязки, вёрстка, позы покоя.
    pub(super) fn new(
        layout: &ShowcaseLayout,
        motion: MotionPreference,
        surface: SurfaceGeometry,
    ) -> Result<Self, ShowcaseError> {
        let settings = SceneSettings {
            motion,
            ..SceneSettings::default()
        };
        let mut scene: UiScene<()> = UiScene::new(surface, settings)?;
        let root = scene.tree().root();
        let nodes = ShowcaseNodes {
            focus_frame: scene.insert_child(root, "focus_frame")?,
            lock_plate: scene.insert_child(root, "lock_plate")?,
            mode_capsule: scene.insert_child(root, "mode_capsule")?,
            flash_button: scene.insert_child(root, "flash_button")?,
            flip_button: scene.insert_child(root, "flip_button")?,
            zoom: [
                scene.insert_child(root, "zoom_left")?,
                scene.insert_child(root, "zoom_middle")?,
                scene.insert_child(root, "zoom_right")?,
            ],
            menu_button: scene.insert_child(root, "menu_button")?,
            panel: scene.insert_child(root, "panel")?,
        };
        Self::place_nodes(&mut scene, &nodes, layout)?;
        scene.layout()?;
        let panel_collapsed = Self::collapsed_pose(&scene, &nodes, layout)?;
        let mut showcase = Self {
            scene,
            nodes,
            layout: layout.clone(),
            panel_collapsed,
        };
        showcase.snap_to_rest_pose()?;
        Ok(showcase)
    }

    /// Привязки всех узлов (числа — из конфига раскладки).
    fn place_nodes(
        scene: &mut UiScene<()>,
        nodes: &ShowcaseNodes,
        layout: &ShowcaseLayout,
    ) -> Result<(), ShowcaseError> {
        let round = square(layout.round_button_diameter_pt);
        let zoom = square(layout.zoom_diameter_pt);
        let menu = square(layout.menu_button_diameter_pt);
        // Центры кружков зума и кнопки меню заданы над низом, а привязка — отступ от низа до НИЖНЕГО края.
        let zoom_bottom_inset = layout.zoom_center_above_bottom_pt - layout.zoom_diameter_pt * 0.5;
        let menu_bottom_inset = layout.menu_button_center_above_bottom_pt - layout.menu_button_diameter_pt * 0.5;

        scene.set_placement(
            nodes.focus_frame,
            anchored(
                Horizontal::CenterOffset(0.0),
                Vertical::FromTop(layout.focus_frame_center_below_top_pt - layout.focus_frame.height_pt * 0.5),
                layout.focus_frame,
            ),
        )?;
        scene.set_placement(
            nodes.lock_plate,
            anchored(
                Horizontal::CenterOffset(0.0),
                Vertical::FromTop(layout.lock_plate_below_top_pt),
                layout.lock_plate,
            ),
        )?;
        scene.set_placement(
            nodes.mode_capsule,
            anchored(
                Horizontal::CenterOffset(0.0),
                Vertical::FromBottom(layout.bottom_margin_pt),
                layout.mode_capsule,
            ),
        )?;
        scene.set_placement(
            nodes.flash_button,
            anchored(
                Horizontal::Start(layout.edge_margin_pt),
                Vertical::FromBottom(layout.bottom_margin_pt),
                round,
            ),
        )?;
        scene.set_placement(
            nodes.flip_button,
            anchored(
                Horizontal::End(layout.edge_margin_pt),
                Vertical::FromBottom(layout.bottom_margin_pt),
                round,
            ),
        )?;
        for (slot, node) in nodes.zoom.iter().enumerate() {
            // Слот 0, 1, 2 → сдвиг центра −шаг, 0, +шаг от центра экрана.
            let offset = (slot as f64 - 1.0) * layout.zoom_pitch_together_pt;
            scene.set_placement(
                *node,
                anchored(
                    Horizontal::CenterOffset(offset),
                    Vertical::FromBottom(zoom_bottom_inset),
                    zoom,
                ),
            )?;
        }
        scene.set_placement(
            nodes.menu_button,
            anchored(
                Horizontal::End(layout.edge_margin_pt),
                Vertical::FromBottom(menu_bottom_inset),
                menu,
            ),
        )?;
        scene.set_placement(
            nodes.panel,
            anchored(
                Horizontal::End(layout.panel_edge_margin_pt),
                Vertical::FromBottom(layout.panel_bottom_above_bottom_pt),
                layout.panel,
            ),
        )?;
        Ok(())
    }

    /// Поза свёрнутой панели по готовым рамкам вёрстки.
    fn collapsed_pose(
        scene: &UiScene<()>,
        nodes: &ShowcaseNodes,
        layout: &ShowcaseLayout,
    ) -> Result<PanelPose, ShowcaseError> {
        let panel_frame: Rect = scene
            .tree()
            .frame(nodes.panel)
            .map_err(ui_core::scene::SceneError::from)?;
        let menu_frame: Rect = scene
            .tree()
            .frame(nodes.menu_button)
            .map_err(ui_core::scene::SceneError::from)?;
        let panel_center = panel_frame.center();
        let menu_center = menu_frame.center();
        // Один равномерный масштаб на обе оси: непрерывное скругление и линза не искажаются. Панель в
        // свёрнутом виде целиком внутри кнопки меню (по большей стороне).
        let larger_side = layout.panel.width_pt.max(layout.panel.height_pt);
        Ok(PanelPose {
            scale: layout.menu_button_diameter_pt / larger_side,
            shift_x: menu_center.x - panel_center.x,
            shift_y: menu_center.y - panel_center.y,
        })
    }

    /// Поза покоя: кружки сведены, панель свёрнута. Без анимации, снимает идущие.
    pub(super) fn snap_to_rest_pose(&mut self) -> Result<(), ShowcaseError> {
        for node in [self.nodes.zoom[0], self.nodes.zoom[2]] {
            self.scene.set_property(node, AnimatedProperty::TranslationX, 0.0)?;
        }
        let pose = self.panel_collapsed;
        let panel = self.nodes.panel;
        self.scene.set_property(panel, AnimatedProperty::ScaleX, pose.scale)?;
        self.scene.set_property(panel, AnimatedProperty::ScaleY, pose.scale)?;
        self.scene
            .set_property(panel, AnimatedProperty::TranslationX, pose.shift_x)?;
        self.scene
            .set_property(panel, AnimatedProperty::TranslationY, pose.shift_y)?;
        Ok(())
    }

    fn animate_property(
        &mut self,
        node: NodeId,
        property: AnimatedProperty,
        target: f64,
        spring: Spring,
        now: MonotonicNanos,
    ) -> Result<(), ShowcaseError> {
        self.scene.animate(AnimationRequest {
            key: PropertyKey { node, property },
            target,
            spring,
            // Прерывание «как в iOS»: новая цель наследует скорость едущей анимации.
            velocity: StartVelocity::Inherit,
            now,
        })?;
        Ok(())
    }

    /// Развести (`ZoomSpread::Apart`) или свести (`Together`) кружки зума: крайние уезжают в стороны от среднего.
    pub(super) fn animate_zoom_spread(
        &mut self,
        spread: ZoomSpread,
        spring: Spring,
        now: MonotonicNanos,
    ) -> Result<(), ShowcaseError> {
        let extra_pt = match spread {
            ZoomSpread::Together => 0.0,
            ZoomSpread::Apart => (self.layout.zoom_pitch_apart_pt - self.layout.zoom_pitch_together_pt).max(0.0),
        };
        let [left, _, right] = self.nodes.zoom;
        self.animate_property(left, AnimatedProperty::TranslationX, -extra_pt, spring, now)?;
        self.animate_property(right, AnimatedProperty::TranslationX, extra_pt, spring, now)
    }

    /// Раскрыть (`PanelState::Open`) или свернуть панель: масштаб и сдвиг едут одной пружиной,
    /// поэтому «рамка растёт из кнопки» без перекоса пропорций.
    pub(super) fn animate_panel(
        &mut self,
        state: PanelState,
        spring: Spring,
        now: MonotonicNanos,
    ) -> Result<(), ShowcaseError> {
        let pose = self.panel_collapsed;
        let (scale, shift_x, shift_y) = match state {
            PanelState::Open => (1.0, 0.0, 0.0),
            PanelState::Collapsed => (pose.scale, pose.shift_x, pose.shift_y),
        };
        let panel = self.nodes.panel;
        self.animate_property(panel, AnimatedProperty::ScaleX, scale, spring, now)?;
        self.animate_property(panel, AnimatedProperty::ScaleY, scale, spring, now)?;
        self.animate_property(panel, AnimatedProperty::TranslationX, shift_x, spring, now)?;
        self.animate_property(panel, AnimatedProperty::TranslationY, shift_y, spring, now)
    }

    /// Продвинуть сцену ко времени `time` (часы → таймеры → аниматор). Время назад сцена игнорирует.
    pub(super) fn advance_to(&mut self, time: MonotonicNanos) {
        // Отчёт кадра (повтор/назад, счётчики) стенду не нужен: время монотонно по построению
        // (`GlassShowcase` пересоздаёт сцену при скачке назад).
        let _frame_report = self.scene.advance_frame(time);
    }

    /// Степень раскрытия панели 0 (свёрнута) … 1 (раскрыта) по текущему масштабу узла.
    pub(super) fn panel_progress(&self) -> f32 {
        let scale = self.scene.tree().scale(self.nodes.panel).map_or(1.0, |scale| scale.x);
        let collapsed = self.panel_collapsed.scale;
        // `collapsed < 1` всегда: кнопка меню меньше панели (проверяется раскладкой в тестах);
        // защита от деления на ноль на случай странного конфига.
        let span = (1.0 - collapsed).max(f64::EPSILON);
        ((scale - collapsed) / span).clamp(0.0, 1.0) as f32
    }

    /// Элементы в порядке рисования с итоговой геометрией (pt).
    pub(super) fn visuals(&self) -> Vec<NodeVisual> {
        self.scene.tree().draw_order()
    }

    /// Масштаб «пиксели на pt», выведенный сценой из ширины экрана.
    pub(super) fn px_per_pt(&self) -> f64 {
        self.scene.point_scale().pixels_per_point()
    }

    pub(super) fn nodes(&self) -> &ShowcaseNodes {
        &self.nodes
    }
}

/// Куда ездят кружки зума.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ZoomSpread {
    Together,
    Apart,
}

/// Состояние панели.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PanelState {
    Collapsed,
    Open,
}

/// Поверхность из логического размера в пикселях, которым пользуется рендер.
pub(super) fn surface_from_pixels(width_px: f32, height_px: f32) -> Result<SurfaceGeometry, ShowcaseError> {
    let usable = |value: f32| value.is_finite() && value >= 1.0 && value <= u32::MAX as f32;
    if !usable(width_px) || !usable(height_px) {
        return Err(ShowcaseError::UnusableSurface { width_px, height_px });
    }
    Ok(SurfaceGeometry {
        width_px: width_px.round() as u32,
        height_px: height_px.round() as u32,
        // Безопасной зоны у стенда нет: низ задан явными отступами раскладки.
        safe_area: Insets::ZERO,
    })
}
