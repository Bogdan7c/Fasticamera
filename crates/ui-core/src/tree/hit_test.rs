//! Хит-тест: путь от самого глубокого попавшего элемента к корню (как `hitTest(_:with:)` в UIKit).
//!
//! Правила:
//! * дети проверяются сверху вниз — в обратном порядке рисования;
//! * скрытый элемент, элемент с `DisabledWithSubtree` и с прозрачностью ниже порога не ловят
//!   касание **вместе с поддеревом** (как UIKit: такой view вообще не опрашивает подвиды);
//! * точка вне области элемента (рамка + расширение области касания) — дети **не проверяются**
//!   (UIKit по умолчанию не находит детей за границей родителя; расширение родителя учитывается);
//! * масштаб, поворот и сдвиг элемента и всех предков учитываются: точка переводится в локальную
//!   систему обратным преобразованием по цепочке от корня вниз;
//! * `PassThrough`: сам элемент касание не ловит, но дети ловят; в путь он попадает только как
//!   предок пойманного ребёнка.
//!
//! Хит-тест использует рамки последней вёрстки: до первого `layout()` у не-корневых элементов
//! они нулевые, и они промахиваются.

use serde::{Deserialize, Serialize};

use super::arena::UiTree;
use super::error::TreeError;
use super::props::{HitTestPolicy, Visibility};
use crate::geometry::Point;
use crate::id::NodeId;

/// Настройки хит-теста.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HitTestConfig {
    /// Элемент с прозрачностью строго ниже порога касания не ловит.
    /// `[review]` 0,01: UIKit `UIView.hitTest(_:with:)` — view с «alpha level less than 0.01» не
    /// участвует в хит-тесте (формулировка по памяти, живая страница Apple не сверялась).
    pub min_hittable_opacity: f64,
}

impl Default for HitTestConfig {
    fn default() -> Self {
        Self {
            min_hittable_opacity: 0.01,
        }
    }
}

impl HitTestConfig {
    /// Проверяет порог.
    pub fn validate(&self) -> Result<(), TreeError> {
        if (0.0..=1.0).contains(&self.min_hittable_opacity) {
            Ok(())
        } else {
            Err(TreeError::InvalidHitTestThreshold(self.min_hittable_opacity))
        }
    }
}

impl UiTree {
    /// Путь хит-теста с настройками по умолчанию: от самого глубокого элемента к корню.
    /// Пусто, если касание никого не поймало.
    pub fn hit_path(&self, point: Point) -> Vec<NodeId> {
        self.hit_path_with(point, &HitTestConfig::default())
    }

    /// То же с явными настройками. Порог вызывающий проверяет сам через [`HitTestConfig::validate`]
    /// (NaN-порог сделал бы все элементы ловящими).
    pub fn hit_path_with(&self, point: Point, config: &HitTestConfig) -> Vec<NodeId> {
        if !(point.x.is_finite() && point.y.is_finite()) {
            return Vec::new();
        }
        self.hit_node(self.root(), point, config).unwrap_or_default()
    }

    /// `point_in_parent` — точка в системе родителя (для корня — экранная).
    fn hit_node(&self, id: NodeId, point_in_parent: Point, config: &HitTestConfig) -> Option<Vec<NodeId>> {
        let node = self.node(id)?;
        let props = &node.props;
        let is_excluded = props.visibility == Visibility::Hidden
            || props.hit_policy == HitTestPolicy::DisabledWithSubtree
            || props.opacity < config.min_hittable_opacity;
        if is_excluded {
            return None;
        }
        let local_point = node.transform().invert(point_in_parent)?;
        if !node.frame.outset_by(props.touch_outset).contains(local_point) {
            return None;
        }
        for &child in node.children.iter().rev() {
            if let Some(mut path) = self.hit_node(child, local_point, config) {
                path.push(id);
                return Some(path);
            }
        }
        match props.hit_policy {
            HitTestPolicy::PassThrough => None,
            HitTestPolicy::Normal | HitTestPolicy::DisabledWithSubtree => Some(vec![id]),
        }
    }
}
