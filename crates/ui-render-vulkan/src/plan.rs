//! План кадра: список рисования → байты буферов экземпляров и пакеты вызовов рисования.
//!
//! Чистая логика без GPU. Все проверки (словарь, пределы) здесь — до первой записи команд, поэтому
//! отвергнутый кадр не оставляет следов ни в цели, ни в пирамиде.

use ui_draw::draw_list::{DrawItem, DrawList};

use crate::config::UiRenderLimits;
use crate::error::{LimitKind, UiRenderError};
use crate::pack::{pack_glass_group, pack_icon, pack_shape, pack_text};

/// Вид пакета: каждому соответствует свой конвейер.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchKind {
    Shape,
    Glyph,
    Glass,
}

/// Подряд идущие элементы одного вида: один вызов рисования, порядок рисования сохраняется.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Batch {
    pub kind: BatchKind,
    /// Номер первого экземпляра в буфере своего вида.
    pub first_instance: u32,
    pub instance_count: u32,
}

/// Готовый к загрузке кадр.
#[derive(Debug, Default)]
pub(crate) struct FramePlan {
    pub shape_bytes: Vec<u8>,
    pub glyph_bytes: Vec<u8>,
    pub group_bytes: Vec<u8>,
    pub member_bytes: Vec<u8>,
    pub batches: Vec<Batch>,
    pub shape_count: u32,
    pub glyph_count: u32,
    pub group_count: u32,
}

impl FramePlan {
    /// Есть ли в кадре стекло (нужна ли пирамида размытия).
    pub(crate) fn has_glass(&self) -> bool {
        self.group_count > 0
    }

    fn push_batch(&mut self, kind: BatchKind, first_instance: u32, added: u32) {
        if added == 0 {
            return;
        }
        match self.batches.last_mut() {
            Some(last) if last.kind == kind => last.instance_count += added,
            _ => self.batches.push(Batch {
                kind,
                first_instance,
                instance_count: added,
            }),
        }
    }
}

/// Сколько экземпляров каждого вида даст список (без упаковки).
fn count_instances(list: &DrawList) -> (usize, usize, usize) {
    let (mut shapes, mut glyphs, mut groups) = (0, 0, 0);
    for item in &list.items {
        if item.bounds_px().is_none() {
            continue;
        }
        match item {
            DrawItem::Shape(_) => shapes += 1,
            DrawItem::Glass(_) => groups += 1,
            DrawItem::Text(text) => glyphs += text.run.glyphs.len(),
            DrawItem::Icon(_) => glyphs += 1,
        }
    }
    (shapes, glyphs, groups)
}

fn check_limit(kind: LimitKind, requested: usize, limit: u32) -> Result<(), UiRenderError> {
    if requested > limit as usize {
        Err(UiRenderError::LimitExceeded { kind, requested, limit })
    } else {
        Ok(())
    }
}

/// Строит план: проверяет словарь и пределы, упаковывает экземпляры, склеивает соседние элементы одного вида.
/// Элементы без границ (пустой текст, пустая группа, нечисловая матрица) пропускаются: рисовать нечего.
pub(crate) fn build_plan(list: &DrawList, limits: &UiRenderLimits) -> Result<FramePlan, UiRenderError> {
    list.validate()?;
    let (shapes, glyphs, groups) = count_instances(list);
    check_limit(LimitKind::ShapeInstances, shapes, limits.max_shape_instances)?;
    check_limit(LimitKind::GlyphInstances, glyphs, limits.max_glyph_instances)?;
    check_limit(LimitKind::GlassGroups, groups, limits.max_glass_groups)?;

    let mut plan = FramePlan::default();
    let mut member_total = 0_usize;
    for (index, item) in list.items.iter().enumerate() {
        let Some(bounds) = item.bounds_px() else { continue };
        match item {
            DrawItem::Shape(shape) => {
                pack_shape(index, shape, &bounds, &mut plan.shape_bytes)?;
                plan.push_batch(BatchKind::Shape, plan.shape_count, 1);
                plan.shape_count += 1;
            }
            DrawItem::Glass(group) => {
                pack_glass_group(
                    index,
                    group,
                    &bounds,
                    member_total,
                    &mut plan.group_bytes,
                    &mut plan.member_bytes,
                )?;
                member_total += group.members.len();
                plan.push_batch(BatchKind::Glass, plan.group_count, 1);
                plan.group_count += 1;
            }
            DrawItem::Text(text) => {
                let added = pack_text(text, &mut plan.glyph_bytes) as u32;
                plan.push_batch(BatchKind::Glyph, plan.glyph_count, added);
                plan.glyph_count += added;
            }
            DrawItem::Icon(icon) => {
                pack_icon(icon, &mut plan.glyph_bytes);
                plan.push_batch(BatchKind::Glyph, plan.glyph_count, 1);
                plan.glyph_count += 1;
            }
        }
    }
    Ok(plan)
}
