//! Арена элементов: вставка, удаление поддерева, связи, рамки.

use super::error::TreeError;
use super::props::NodeProps;
use super::transform::CenteredTransform;
use crate::geometry::{Insets, Point, Rect, Size};
use crate::id::NodeId;
use crate::layout::{ContainerLayout, Placement};

/// Данные одного элемента. Поля видны только внутри `tree`; снаружи — методы [`UiTree`].
#[derive(Debug, Clone)]
pub(super) struct NodeData {
    pub parent: Option<NodeId>,
    /// Порядок рисования: последний — сверху.
    pub children: Vec<NodeId>,
    pub label: &'static str,
    pub props: NodeProps,
    pub placement: Placement,
    pub container: ContainerLayout,
    pub safe_insets: Insets,
    /// Абсолютная рамка в pt без визуального преобразования; обновляется `layout()`.
    pub frame: Rect,
}

impl NodeData {
    fn new(parent: Option<NodeId>, label: &'static str, frame: Rect) -> Self {
        Self {
            parent,
            children: Vec::new(),
            label,
            props: NodeProps::default(),
            placement: Placement::default(),
            container: ContainerLayout::default(),
            safe_insets: Insets::ZERO,
            frame,
        }
    }
}

/// Место в арене: поколение растёт при каждом освобождении.
#[derive(Debug, Clone)]
struct Slot {
    generation: u32,
    node: Option<NodeData>,
}

/// Дерево элементов экрана.
#[derive(Debug, Clone)]
pub struct UiTree {
    slots: Vec<Slot>,
    /// Свободные места; берётся последнее (LIFO) — порядок переиспользования детерминирован.
    free_slots: Vec<u32>,
    root: NodeId,
}

impl UiTree {
    /// Дерево с корнем-экраном размером `screen_size` (pt) и безопасной зоной `safe_area`.
    /// Рамка корня — `(0, 0, screen_size)`.
    pub fn new(screen_size: Size, safe_area: Insets) -> Result<Self, TreeError> {
        validate_screen(screen_size, safe_area)?;
        let root = NodeId::new(0, 0);
        let mut root_data = NodeData::new(None, "root", Rect::new(Point::ORIGIN, screen_size));
        root_data.safe_insets = safe_area;
        Ok(Self {
            slots: vec![Slot {
                generation: 0,
                node: Some(root_data),
            }],
            free_slots: Vec::new(),
            root,
        })
    }

    /// Корень дерева.
    pub fn root(&self) -> NodeId {
        self.root
    }

    /// Меняет размер экрана и безопасную зону (поворот, смена окна). Рамки устаревают до `layout()`.
    pub fn resize_screen(&mut self, screen_size: Size, safe_area: Insets) -> Result<(), TreeError> {
        validate_screen(screen_size, safe_area)?;
        let root = self.require_mut(self.root)?;
        root.frame = Rect::new(Point::ORIGIN, screen_size);
        root.safe_insets = safe_area;
        Ok(())
    }

    /// Жив ли элемент с таким идентификатором (поколение должно совпасть).
    pub fn contains(&self, node: NodeId) -> bool {
        self.node(node).is_some()
    }

    /// Число живых элементов (включая корень).
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|slot| slot.node.is_some()).count()
    }

    /// Всегда `false`: корень существует всегда. Нужен паре к `len` (clippy).
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Добавляет нового ребёнка в конец (рисуется поверх остальных детей).
    pub fn insert_child(&mut self, parent: NodeId, label: &'static str) -> Result<NodeId, TreeError> {
        let child_count = self.require(parent)?.children.len();
        self.insert_child_at(parent, child_count, label)
    }

    /// Добавляет нового ребёнка в позицию `index` порядка рисования (`index == число детей` — в конец).
    pub fn insert_child_at(&mut self, parent: NodeId, index: usize, label: &'static str) -> Result<NodeId, TreeError> {
        let child_count = self.require(parent)?.children.len();
        if index > child_count {
            return Err(TreeError::ChildIndexOutOfRange {
                parent,
                index,
                child_count,
            });
        }
        let child = self.allocate(NodeData::new(Some(parent), label, Rect::default()))?;
        self.require_mut(parent)?.children.insert(index, child);
        Ok(child)
    }

    /// Удаляет элемент вместе с поддеревом. Возвращает идентификаторы всех удалённых элементов
    /// (сначала сам элемент, затем потомки в порядке обхода в глубину): по ним вызывающий
    /// отменяет жесты и анимации. Корень — [`TreeError::CannotRemoveRoot`].
    pub fn remove_subtree(&mut self, node: NodeId) -> Result<Vec<NodeId>, TreeError> {
        if node == self.root {
            return Err(TreeError::CannotRemoveRoot);
        }
        let parent = self.require(node)?.parent;
        if let Some(parent) = parent {
            self.require_mut(parent)?.children.retain(|&child| child != node);
        }
        let mut removed = Vec::new();
        let mut pending = vec![node];
        while let Some(current) = pending.pop() {
            let Some(data) = self.release(current) else { continue };
            // Детей кладём в обратном порядке, чтобы обход шёл слева направо.
            pending.extend(data.children.iter().rev());
            removed.push(current);
        }
        Ok(removed)
    }

    /// Родитель элемента (`None` у корня).
    pub fn parent(&self, node: NodeId) -> Result<Option<NodeId>, TreeError> {
        Ok(self.require(node)?.parent)
    }

    /// Дети в порядке рисования (последний — сверху).
    pub fn children(&self, node: NodeId) -> Result<&[NodeId], TreeError> {
        Ok(&self.require(node)?.children)
    }

    /// Метка элемента для отладки и тестов.
    pub fn label(&self, node: NodeId) -> Result<&'static str, TreeError> {
        Ok(self.require(node)?.label)
    }

    /// Рамка после последней вёрстки, pt, без визуального преобразования. До первой вёрстки
    /// у не-корневых элементов — нулевая.
    pub fn frame(&self, node: NodeId) -> Result<Rect, TreeError> {
        Ok(self.require(node)?.frame)
    }

    /// Ограничивающий прямоугольник того, что видно на экране: рамка с учётом масштаба, поворота и сдвига
    /// самого элемента и всех его предков.
    pub fn visual_bounds(&self, node: NodeId) -> Result<Rect, TreeError> {
        let frame = self.require(node)?.frame;
        let mut corners = [
            Point::new(frame.min_x(), frame.min_y()),
            Point::new(frame.max_x(), frame.min_y()),
            Point::new(frame.max_x(), frame.max_y()),
            Point::new(frame.min_x(), frame.max_y()),
        ];
        let mut current = Some(node);
        while let Some(id) = current {
            let data = self.require(id)?;
            let transform = data.transform();
            for corner in &mut corners {
                *corner = transform.apply(*corner);
            }
            current = data.parent;
        }
        let min_x = corners.iter().map(|c| c.x).fold(f64::INFINITY, f64::min);
        let max_x = corners.iter().map(|c| c.x).fold(f64::NEG_INFINITY, f64::max);
        let min_y = corners.iter().map(|c| c.y).fold(f64::INFINITY, f64::min);
        let max_y = corners.iter().map(|c| c.y).fold(f64::NEG_INFINITY, f64::max);
        Ok(Rect::from_xywh(min_x, min_y, max_x - min_x, max_y - min_y))
    }

    // ---- внутренний API модуля `tree` и вёрстки ----

    pub(super) fn node(&self, id: NodeId) -> Option<&NodeData> {
        let slot = self.slots.get(id.slot() as usize)?;
        if slot.generation == id.generation() {
            slot.node.as_ref()
        } else {
            None
        }
    }

    pub(super) fn require(&self, id: NodeId) -> Result<&NodeData, TreeError> {
        self.node(id).ok_or(TreeError::UnknownNode(id))
    }

    pub(super) fn require_mut(&mut self, id: NodeId) -> Result<&mut NodeData, TreeError> {
        let slot = self
            .slots
            .get_mut(id.slot() as usize)
            .filter(|slot| slot.generation == id.generation());
        slot.and_then(|slot| slot.node.as_mut())
            .ok_or(TreeError::UnknownNode(id))
    }

    /// Записывает рассчитанные вёрсткой рамки (вызывается только из `layout()`).
    pub(crate) fn apply_frames(&mut self, frames: &[(NodeId, Rect)]) {
        for &(id, frame) in frames {
            if let Ok(data) = self.require_mut(id) {
                data.frame = frame;
            }
        }
    }

    /// Занимает свободное место (с его текущим поколением) или заводит новое.
    fn allocate(&mut self, data: NodeData) -> Result<NodeId, TreeError> {
        if let Some(slot_index) = self.free_slots.pop() {
            let slot = &mut self.slots[slot_index as usize];
            slot.node = Some(data);
            return Ok(NodeId::new(slot_index, slot.generation));
        }
        let slot_index = u32::try_from(self.slots.len()).map_err(|_| TreeError::CapacityExceeded)?;
        self.slots.push(Slot {
            generation: 0,
            node: Some(data),
        });
        Ok(NodeId::new(slot_index, 0))
    }

    /// Освобождает место: поколение растёт, старые идентификаторы перестают действовать.
    /// Место, у которого поколение исчерпано (`u32::MAX`), навсегда выводится из оборота,
    /// чтобы идентификатор никогда не совпал с чужим.
    fn release(&mut self, id: NodeId) -> Option<NodeData> {
        let slot = self
            .slots
            .get_mut(id.slot() as usize)
            .filter(|slot| slot.generation == id.generation())?;
        let data = slot.node.take()?;
        if let Some(next_generation) = slot.generation.checked_add(1) {
            slot.generation = next_generation;
            self.free_slots.push(id.slot());
        }
        Some(data)
    }
}

impl NodeData {
    /// Визуальное преобразование элемента вокруг центра его рамки.
    pub(super) fn transform(&self) -> CenteredTransform {
        CenteredTransform {
            center: self.frame.center(),
            scale: self.props.scale,
            rotation_radians: self.props.rotation_radians,
            translation: self.props.translation,
        }
    }
}

fn validate_screen(screen_size: Size, safe_area: Insets) -> Result<(), TreeError> {
    let size_ok = screen_size.width.is_finite()
        && screen_size.height.is_finite()
        && screen_size.width >= 0.0
        && screen_size.height >= 0.0;
    let insets_ok = [safe_area.top, safe_area.left, safe_area.bottom, safe_area.right]
        .iter()
        .all(|v| v.is_finite());
    if size_ok && insets_ok {
        Ok(())
    } else {
        Err(TreeError::InvalidScreen)
    }
}
