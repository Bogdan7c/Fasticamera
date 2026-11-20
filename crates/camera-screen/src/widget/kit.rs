//! Обёртка над `UiScene`: создание узлов по абсолютным рамкам (pt), показ/скрытие, поворот значков.
//!
//! Все виджеты строят узлы здесь, поэтому правила «после смены структуры — `layout()`» и «скрытый узел не ловит
//! касания» соблюдаются в одном месте.

use ui_core::animator::{AnimatedProperty, AnimationRequest, PropertyKey, StartVelocity};
use ui_core::geometry::{Insets, Rect};
use ui_core::gesture::{RecognizerId, RecognizerSpec};
use ui_core::id::NodeId;
use ui_core::layout::{AnchorRule, AxisPlacement, Placement, ReferenceArea};
use ui_core::scene::UiScene;
use ui_core::spring::Spring;
use ui_core::time::MonotonicNanos;
use ui_core::tree::{HitTestPolicy, Visibility};

use super::error::{WidgetError, element};
use super::intent::ScreenGesture;

/// Сцена экрана и список узлов, которые поворачиваются по ориентации телефона.
pub(crate) struct SceneKit {
    scene: UiScene<ScreenGesture>,
    /// Содержимое-значки: поворачиваются на месте (рамки раскладки не меняются).
    rotating: Vec<NodeId>,
    /// Угол поворота значков, к которому уже едем (радианы, без свёртки по кругу).
    rotation_target_rad: f64,
    layout_dirty: bool,
}

impl SceneKit {
    pub(crate) fn new(scene: UiScene<ScreenGesture>) -> Self {
        Self {
            scene,
            rotating: Vec::new(),
            rotation_target_rad: 0.0,
            layout_dirty: false,
        }
    }

    pub(crate) fn scene(&self) -> &UiScene<ScreenGesture> {
        &self.scene
    }

    pub(crate) fn scene_mut(&mut self) -> &mut UiScene<ScreenGesture> {
        &mut self.scene
    }

    /// Якорь «от левого верха родителя»: рамка `rect` (абсолютная) внутри родителя с абсолютной рамкой `parent`.
    fn placement_in(rect: Rect, parent: Rect) -> Placement {
        Placement::Anchored(AnchorRule {
            area: ReferenceArea::Frame,
            horizontal: AxisPlacement::Start {
                inset: rect.min_x() - parent.min_x(),
                length: rect.size.width,
            },
            vertical: AxisPlacement::Start {
                inset: rect.min_y() - parent.min_y(),
                length: rect.size.height,
            },
        })
    }

    /// Новый узел с абсолютной рамкой `rect` внутри `parent` (рамка родителя — `parent_rect`).
    pub(crate) fn add_node(
        &mut self,
        parent: NodeId,
        label: &'static str,
        rect: Rect,
        parent_rect: Rect,
    ) -> Result<NodeId, WidgetError> {
        let node = self.scene.insert_child(parent, label)?;
        self.scene.set_placement(node, Self::placement_in(rect, parent_rect))?;
        self.layout_dirty = true;
        Ok(node)
    }

    /// Новый узел-содержимое: занимает всю рамку хоста и поворачивается по ориентации телефона.
    pub(crate) fn add_rotating_content(&mut self, host: NodeId, label: &'static str) -> Result<NodeId, WidgetError> {
        let node = self.scene.insert_child(host, label)?;
        self.scene
            .set_placement(node, Placement::Anchored(AnchorRule::fill_parent()))?;
        self.rotating.push(node);
        self.layout_dirty = true;
        Ok(node)
    }

    /// Переставить узел (рамка абсолютная). Вызывать только при реальном изменении рамки.
    pub(crate) fn move_node(&mut self, node: NodeId, rect: Rect, parent_rect: Rect) -> Result<(), WidgetError> {
        self.scene.set_placement(node, Self::placement_in(rect, parent_rect))?;
        self.layout_dirty = true;
        Ok(())
    }

    pub(crate) fn add_recognizer(
        &mut self,
        node: NodeId,
        spec: RecognizerSpec,
        intent: ScreenGesture,
    ) -> Result<RecognizerId, WidgetError> {
        Ok(self.scene.add_recognizer(node, spec, intent)?)
    }

    /// Показать узел (виден и ловит касания) или спрятать (не рисуется и не ловит касаний вместе с детьми).
    pub(crate) fn set_shown(&mut self, node: NodeId, shown: bool) -> Result<(), WidgetError> {
        let (visibility, policy) = if shown {
            (Visibility::Visible, HitTestPolicy::Normal)
        } else {
            (Visibility::Hidden, HitTestPolicy::DisabledWithSubtree)
        };
        self.scene.set_visibility(node, visibility)?;
        self.scene.set_hit_test_policy(node, policy)?;
        Ok(())
    }

    /// Ловит ли узел (и его поддерево) касания, не меняя видимости: так затвор может плавно растворяться
    /// под шторкой и при этом уже не нажиматься.
    pub(crate) fn set_interactive(&mut self, node: NodeId, interactive: bool) -> Result<(), WidgetError> {
        let policy = if interactive {
            HitTestPolicy::Normal
        } else {
            HitTestPolicy::DisabledWithSubtree
        };
        self.scene.set_hit_test_policy(node, policy)?;
        Ok(())
    }

    /// Прозрачность узла пружиной (`None` — сразу).
    pub(crate) fn fade_to(
        &mut self,
        node: NodeId,
        opacity: f64,
        spring: Spring,
        now: MonotonicNanos,
    ) -> Result<(), WidgetError> {
        self.scene.animate(AnimationRequest {
            key: PropertyKey {
                node,
                property: AnimatedProperty::Opacity,
            },
            target: opacity,
            spring,
            velocity: StartVelocity::Inherit,
            now,
        })?;
        Ok(())
    }

    /// Визуально сдвигает узел по вертикали на `offset_pt` (pt, вниз положительно) пружиной. Хит-тест `ui-core`
    /// учитывает сдвиг узла, поэтому зона касания едет вместе с картинкой. Так поднимается ряд затвора над
    /// карточкой (решение 103): рамка раскладки не меняется, меняется только сдвиг.
    pub(crate) fn slide_vertically_to(
        &mut self,
        node: NodeId,
        offset_pt: f64,
        spring: Spring,
        now: MonotonicNanos,
    ) -> Result<(), WidgetError> {
        self.scene.animate(AnimationRequest {
            key: PropertyKey {
                node,
                property: AnimatedProperty::TranslationY,
            },
            target: offset_pt,
            spring,
            velocity: StartVelocity::Inherit,
            now,
        })?;
        Ok(())
    }

    /// Сдвигает узел по вертикали на `offset_pt` сразу, без анимации (Reduce Motion: без перемещений); идущая
    /// анимация сдвига снимается.
    pub(crate) fn jump_vertically_to(&mut self, node: NodeId, offset_pt: f64) -> Result<(), WidgetError> {
        self.scene
            .set_property(node, AnimatedProperty::TranslationY, offset_pt)?;
        Ok(())
    }

    /// Сдвигает узел по горизонтали на `offset_pt` пружиной (лента зума едет под неподвижной выбранной кнопкой).
    pub(crate) fn slide_horizontally_to(
        &mut self,
        node: NodeId,
        offset_pt: f64,
        spring: Spring,
        now: MonotonicNanos,
    ) -> Result<(), WidgetError> {
        self.scene.animate(AnimationRequest {
            key: PropertyKey {
                node,
                property: AnimatedProperty::TranslationX,
            },
            target: offset_pt,
            spring,
            velocity: StartVelocity::Inherit,
            now,
        })?;
        Ok(())
    }

    /// Сдвигает узел по горизонтали сразу, без анимации; идущая анимация сдвига снимается.
    pub(crate) fn jump_horizontally_to(&mut self, node: NodeId, offset_pt: f64) -> Result<(), WidgetError> {
        self.scene
            .set_property(node, AnimatedProperty::TranslationX, offset_pt)?;
        Ok(())
    }

    /// Текущий визуальный сдвиг узла по горизонтали, pt (с учётом идущей анимации).
    pub(crate) fn horizontal_offset_pt(&self, node: NodeId) -> Result<f64, WidgetError> {
        self.scene
            .tree()
            .translation(node)
            .map(|translation| translation.dx)
            .map_err(element("сдвиг узла"))
    }

    /// Текущий визуальный сдвиг узла по вертикали, pt (с учётом идущей анимации).
    pub(crate) fn vertical_offset_pt(&self, node: NodeId) -> Result<f64, WidgetError> {
        self.scene
            .tree()
            .translation(node)
            .map(|translation| translation.dy)
            .map_err(element("сдвиг узла"))
    }

    /// Расширяет зону касания узла с рамкой `frame` до квадрата не меньше `min_side_pt` (HIG: ≥ 44 pt), не
    /// меняя рамку раскладки: круглая кнопка Ø 36 рисуется как есть, а попасть в неё проще.
    pub(crate) fn ensure_min_touch_side(
        &mut self,
        node: NodeId,
        frame: Rect,
        min_side_pt: f64,
    ) -> Result<(), WidgetError> {
        let horizontal = ((min_side_pt - frame.size.width) * 0.5).max(0.0);
        let vertical = ((min_side_pt - frame.size.height) * 0.5).max(0.0);
        self.scene.set_touch_outset(
            node,
            Insets {
                top: vertical,
                left: horizontal,
                bottom: vertical,
                right: horizontal,
            },
        )?;
        Ok(())
    }

    /// Пересчитать вёрстку, если рамки менялись.
    pub(crate) fn relayout_if_needed(&mut self) -> Result<(), WidgetError> {
        if self.layout_dirty {
            self.scene.layout()?;
            self.layout_dirty = false;
        }
        Ok(())
    }

    /// Целевой угол значков по ориентации: поворот по кратчайшему пути от текущей цели (−90° → 180° — это
    /// 90° в сторону, а не 270° обратно). Возвращает `true`, если цель изменилась.
    pub(crate) fn rotate_icons(
        &mut self,
        desired_clockwise_degrees: f32,
        spring: Spring,
        now: MonotonicNanos,
    ) -> Result<bool, WidgetError> {
        let desired = f64::from(desired_clockwise_degrees).to_radians();
        let full_turn = std::f64::consts::TAU;
        let mut delta = (desired - self.rotation_target_rad).rem_euclid(full_turn);
        if delta > std::f64::consts::PI {
            delta -= full_turn;
        }
        if delta.abs() < 1e-9 {
            return Ok(false);
        }
        self.rotation_target_rad += delta;
        for node in self.rotating.clone() {
            self.scene.animate(AnimationRequest {
                key: PropertyKey {
                    node,
                    property: AnimatedProperty::Rotation,
                },
                target: self.rotation_target_rad,
                spring,
                velocity: StartVelocity::Inherit,
                now,
            })?;
        }
        Ok(true)
    }

    /// Угол, к которому едут значки, радианы (для тестов и диагностики).
    #[cfg(test)]
    pub(crate) fn rotation_target_rad(&self) -> f64 {
        self.rotation_target_rad
    }
}
