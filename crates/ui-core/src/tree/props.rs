//! Визуальные свойства элемента и их сеттеры-намерения.

use super::arena::UiTree;
use super::error::TreeError;
use super::transform::ElementScale;
use crate::geometry::{Insets, Vector};
use crate::id::NodeId;

/// Виден ли элемент. Скрытый элемент не рисуется, не ловит касания (вместе с детьми), но
/// продолжает занимать место в вёрстке.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Visibility {
    #[default]
    Visible,
    Hidden,
}

/// Как элемент участвует в хит-тесте.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HitTestPolicy {
    /// Обычный: ловит касание в своей области, если дети промахнулись.
    #[default]
    Normal,
    /// Сам касания не ловит, но дети ловят (прозрачный контейнер поверх видоискателя).
    PassThrough,
    /// Элемент и всё поддерево для касаний не существуют (`isUserInteractionEnabled = false`).
    DisabledWithSubtree,
}

/// Докуда достаёт область касания элемента относительно его родителя.
///
/// По умолчанию — как UIKit: родитель, мимо области которого прошло касание, детей не опрашивает,
/// и расширенная область маленькой кнопки за краем родителя «мёртвая». `BeyondParent` — аналог
/// `TouchDelegate` Android: такой ребёнок ловит касание и за краем родителя (решение 278 плана
/// `user/foundation/`). Граница всё равно есть: сам родитель опрашивается, только если точка внутри
/// ЕГО родителя, поэтому окно прокрутки и любые обычные предки выше по-прежнему режут касание.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TouchReach {
    /// Касание за областью родителя до элемента не доходит (UIKit по умолчанию).
    #[default]
    WithinParent,
    /// Область элемента (рамка + расширение) работает и за краем родителя.
    BeyondParent,
}

/// Свойства элемента. Поля закрыты: меняются только через сеттеры [`UiTree`] с проверкой.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct NodeProps {
    pub opacity: f64,
    pub scale: ElementScale,
    pub rotation_radians: f64,
    pub translation: Vector,
    pub visibility: Visibility,
    pub hit_policy: HitTestPolicy,
    pub touch_outset: Insets,
    pub touch_reach: TouchReach,
}

impl Default for NodeProps {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            scale: ElementScale::IDENTITY,
            rotation_radians: 0.0,
            translation: Vector::ZERO,
            visibility: Visibility::Visible,
            hit_policy: HitTestPolicy::Normal,
            touch_outset: Insets::ZERO,
            touch_reach: TouchReach::WithinParent,
        }
    }
}

impl UiTree {
    /// Прозрачность элемента, 0..=1.
    pub fn opacity(&self, node: NodeId) -> Result<f64, TreeError> {
        Ok(self.require(node)?.props.opacity)
    }

    /// Задаёт прозрачность. Значение вне [0, 1] или NaN — [`TreeError::InvalidOpacity`], свойство не меняется.
    pub fn set_opacity(&mut self, node: NodeId, opacity: f64) -> Result<(), TreeError> {
        if !(0.0..=1.0).contains(&opacity) {
            return Err(TreeError::InvalidOpacity(opacity));
        }
        self.require_mut(node)?.props.opacity = opacity;
        Ok(())
    }

    /// Масштаб элемента вокруг его центра.
    pub fn scale(&self, node: NodeId) -> Result<ElementScale, TreeError> {
        Ok(self.require(node)?.props.scale)
    }

    /// Задаёт масштаб (визуальный, вёрстку не меняет). Нуль и не-числа — ошибка.
    pub fn set_scale(&mut self, node: NodeId, scale: ElementScale) -> Result<(), TreeError> {
        let is_valid = |factor: f64| factor.is_finite() && factor != 0.0;
        if !(is_valid(scale.x) && is_valid(scale.y)) {
            return Err(TreeError::InvalidScale { x: scale.x, y: scale.y });
        }
        self.require_mut(node)?.props.scale = scale;
        Ok(())
    }

    /// Поворот элемента вокруг центра, радианы (положительный — по часовой стрелке на экране,
    /// так как ось `y` направлена вниз).
    pub fn rotation_radians(&self, node: NodeId) -> Result<f64, TreeError> {
        Ok(self.require(node)?.props.rotation_radians)
    }

    /// Задаёт поворот (визуальный). Не-число — ошибка.
    pub fn set_rotation_radians(&mut self, node: NodeId, rotation_radians: f64) -> Result<(), TreeError> {
        if !rotation_radians.is_finite() {
            return Err(TreeError::InvalidRotation(rotation_radians));
        }
        self.require_mut(node)?.props.rotation_radians = rotation_radians;
        Ok(())
    }

    /// Визуальный сдвиг, pt (не влияет на `frame` и вёрстку; наследуется детьми).
    pub fn translation(&self, node: NodeId) -> Result<Vector, TreeError> {
        Ok(self.require(node)?.props.translation)
    }

    /// Задаёт визуальный сдвиг в pt родителя. Порядок композиции — см. `transform.rs`:
    /// `p' = c + T + R·S·(p − c)`. Не-конечные значения — ошибка.
    pub fn set_translation(&mut self, node: NodeId, translation: Vector) -> Result<(), TreeError> {
        if !(translation.dx.is_finite() && translation.dy.is_finite()) {
            return Err(TreeError::InvalidTranslation {
                dx: translation.dx,
                dy: translation.dy,
            });
        }
        self.require_mut(node)?.props.translation = translation;
        Ok(())
    }

    pub fn visibility(&self, node: NodeId) -> Result<Visibility, TreeError> {
        Ok(self.require(node)?.props.visibility)
    }

    pub fn set_visibility(&mut self, node: NodeId, visibility: Visibility) -> Result<(), TreeError> {
        self.require_mut(node)?.props.visibility = visibility;
        Ok(())
    }

    pub fn hit_test_policy(&self, node: NodeId) -> Result<HitTestPolicy, TreeError> {
        Ok(self.require(node)?.props.hit_policy)
    }

    pub fn set_hit_test_policy(&mut self, node: NodeId, policy: HitTestPolicy) -> Result<(), TreeError> {
        self.require_mut(node)?.props.hit_policy = policy;
        Ok(())
    }

    /// Расширение области касания за рамку (минимальная зона касания маленького значка).
    pub fn touch_outset(&self, node: NodeId) -> Result<Insets, TreeError> {
        Ok(self.require(node)?.props.touch_outset)
    }

    /// Задаёт расширение области касания: положительные значения растят зону наружу.
    pub fn set_touch_outset(&mut self, node: NodeId, outset: Insets) -> Result<(), TreeError> {
        let all_finite = [outset.top, outset.left, outset.bottom, outset.right]
            .iter()
            .all(|value| value.is_finite());
        if !all_finite {
            return Err(TreeError::InvalidTouchOutset(outset));
        }
        self.require_mut(node)?.props.touch_outset = outset;
        Ok(())
    }

    /// Докуда достаёт область касания элемента: в пределах родителя или и за его краем.
    pub fn touch_reach(&self, node: NodeId) -> Result<TouchReach, TreeError> {
        Ok(self.require(node)?.props.touch_reach)
    }

    /// Разрешает (или снова запрещает) области касания элемента работать за краем родителя.
    pub fn set_touch_reach(&mut self, node: NodeId, reach: TouchReach) -> Result<(), TreeError> {
        self.require_mut(node)?.props.touch_reach = reach;
        Ok(())
    }
}
