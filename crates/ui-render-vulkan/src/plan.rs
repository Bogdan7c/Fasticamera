//! План кадра: список рисования → байты буферов экземпляров и пакеты вызовов рисования.
//!
//! Чистая логика без GPU. Все проверки (словарь, пределы) здесь — до первой записи команд, поэтому
//! отвергнутый кадр не оставляет следов ни в цели, ни в пирамиде.

use ui_draw::draw_list::{DrawItem, DrawList};
use ui_draw::picture::UiPicture;

use crate::config::UiRenderLimits;
use crate::error::{LimitKind, UiRenderError};
use crate::pack::{pack_glass_group, pack_icon, pack_image, pack_shape, pack_text};

/// Вид пакета: каждому соответствует свой конвейер.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchKind {
    Shape,
    Glyph,
    Glass,
    Image,
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
    pub image_bytes: Vec<u8>,
    /// Разные картинки кадра в порядке первого появления: номер в списке = номер сэмплера в шейдере.
    pub pictures: Vec<UiPicture>,
    pub batches: Vec<Batch>,
    pub shape_count: u32,
    pub glyph_count: u32,
    pub group_count: u32,
    pub image_count: u32,
}

impl FramePlan {
    /// Есть ли в кадре стекло (нужна ли пирамида размытия).
    pub(crate) fn has_glass(&self) -> bool {
        self.group_count > 0
    }

    /// Номер сэмплера кадра для картинки: та же картинка (по версии) получает прежний номер.
    fn texture_index_of(&mut self, picture: &UiPicture) -> usize {
        if let Some(position) = self
            .pictures
            .iter()
            .position(|known| known.version() == picture.version())
        {
            return position;
        }
        self.pictures.push(picture.clone());
        self.pictures.len() - 1
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
fn count_instances(list: &DrawList) -> InstanceCounts {
    let mut counts = InstanceCounts::default();
    for item in &list.items {
        if item.bounds_px().is_none() {
            continue;
        }
        match item {
            DrawItem::Shape(_) => counts.shapes += 1,
            DrawItem::Glass(_) => counts.groups += 1,
            DrawItem::Text(text) => counts.glyphs += text.run.glyphs.len(),
            DrawItem::Icon(_) => counts.glyphs += 1,
            DrawItem::Image(_) => counts.images += 1,
        }
    }
    counts
}

/// Число экземпляров по видам.
#[derive(Default)]
struct InstanceCounts {
    shapes: usize,
    glyphs: usize,
    groups: usize,
    images: usize,
}

/// Отвергает картинки с чрезмерной стороной: до первой команды и до выделения памяти.
fn check_picture_sizes(list: &DrawList, limits: &UiRenderLimits) -> Result<(), UiRenderError> {
    for item in &list.items {
        let DrawItem::Image(image) = item else { continue };
        if item.bounds_px().is_none() {
            continue;
        }
        let size = image.picture.size();
        if size.width.max(size.height) > limits.max_image_side_px {
            return Err(UiRenderError::PictureTooLarge {
                version: image.picture.version(),
                size,
                limit: limits.max_image_side_px,
            });
        }
    }
    Ok(())
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
    let counts = count_instances(list);
    check_limit(LimitKind::ShapeInstances, counts.shapes, limits.max_shape_instances)?;
    check_limit(LimitKind::GlyphInstances, counts.glyphs, limits.max_glyph_instances)?;
    check_limit(LimitKind::GlassGroups, counts.groups, limits.max_glass_groups)?;
    // Лимит считает ЭЛЕМЕНТЫ-картинки; разных картинок их не больше, поэтому сэмплеров хватит.
    check_limit(LimitKind::Images, counts.images, limits.max_images)?;
    check_picture_sizes(list, limits)?;

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
            DrawItem::Image(image) => {
                let texture_index = plan.texture_index_of(&image.picture);
                pack_image(index, image, &bounds, texture_index, &mut plan.image_bytes)?;
                plan.push_batch(BatchKind::Image, plan.image_count, 1);
                plan.image_count += 1;
            }
        }
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, reason = "тесты")]

    use super::*;
    use crate::pack::IMAGE_INSTANCE_BYTES;
    use ui_draw::color::{DisplayColor, Opacity};
    use ui_draw::draw_list::{ImageClip, ImageFit, ImageItem, ShapeItem};
    use ui_draw::geometry::PxSize;
    use ui_draw::light::LightDirection;
    use ui_draw::picture::{PictureEncoding, PictureSize};
    use ui_draw::shape::ShapeGeometry;
    use ui_draw::transform::Affine2;

    fn picture(width: u32, height: u32) -> UiPicture {
        UiPicture::from_rgba8(
            PictureSize::new(width, height),
            PictureEncoding::Srgb,
            vec![0; (width * height * 4) as usize],
        )
        .unwrap()
    }

    fn image(picture: &UiPicture) -> DrawItem {
        DrawItem::Image(ImageItem {
            picture: picture.clone(),
            transform: Affine2::translate(30.0, 30.0),
            size_px: PxSize::new(20.0, 20.0),
            fit: ImageFit::Cover,
            clip: ImageClip::Circle,
            opacity: Opacity::OPAQUE,
        })
    }

    fn shape() -> DrawItem {
        DrawItem::Shape(ShapeItem {
            geometry: ShapeGeometry::Circle { diameter: 10.0 },
            transform: Affine2::translate(10.0, 10.0),
            fill: DisplayColor::WHITE,
            stroke: None,
            shadow: None,
            opacity: Opacity::OPAQUE,
        })
    }

    fn list(items: Vec<DrawItem>) -> DrawList {
        let mut list = DrawList::new(LightDirection::from_angle_degrees(45.0));
        list.items = items;
        list
    }

    /// Номер сэмплера, записанный в `index`-м экземпляре картинки (последний `f32` экземпляра).
    fn texture_index_of_instance(plan: &FramePlan, index: usize) -> f32 {
        let end = (index + 1) * IMAGE_INSTANCE_BYTES;
        f32::from_ne_bytes(plan.image_bytes[end - 4..end].try_into().unwrap())
    }

    #[test]
    fn images_batch_by_neighbours_and_keep_list_order_around_shapes() {
        let (first, second) = (picture(4, 4), picture(8, 4));
        let plan = build_plan(
            &list(vec![image(&first), image(&second), shape(), image(&first)]),
            &UiRenderLimits {
                max_images: 3,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            plan.batches,
            vec![
                Batch {
                    kind: BatchKind::Image,
                    first_instance: 0,
                    instance_count: 2
                },
                Batch {
                    kind: BatchKind::Shape,
                    first_instance: 0,
                    instance_count: 1
                },
                Batch {
                    kind: BatchKind::Image,
                    first_instance: 2,
                    instance_count: 1
                },
            ],
            "соседние картинки — один вызов, фигура между ними разрывает пакет, порядок списка сохранён"
        );
        assert_eq!((plan.image_count, plan.shape_count), (3, 1));
        assert_eq!(plan.image_bytes.len(), 3 * IMAGE_INSTANCE_BYTES);
        // Две разные картинки → два сэмплера; повтор первой берёт прежний номер.
        assert_eq!(plan.pictures.len(), 2);
        assert_eq!(
            [0, 1, 2].map(|index| texture_index_of_instance(&plan, index)),
            [0.0, 1.0, 0.0]
        );
    }

    #[test]
    fn more_image_items_than_the_limit_is_an_error_and_plans_nothing() {
        let one = picture(4, 4);
        let error = build_plan(
            &list(vec![image(&one), image(&one), image(&one)]),
            &UiRenderLimits {
                max_images: 2,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                UiRenderError::LimitExceeded {
                    kind: LimitKind::Images,
                    requested: 3,
                    limit: 2
                }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn oversized_picture_is_rejected_with_its_version_and_size() {
        let huge = picture(2000, 10);
        let error = build_plan(&list(vec![shape(), image(&huge)]), &UiRenderLimits::default()).unwrap_err();
        match error {
            UiRenderError::PictureTooLarge { version, size, limit } => {
                assert_eq!(version, huge.version());
                assert_eq!(size, PictureSize::new(2000, 10));
                assert_eq!(limit, 1024);
            }
            other => panic!("ожидали PictureTooLarge, получили {other:?}"),
        }
        // Ровно на пределе — принимается.
        let edge = picture(1024, 4);
        assert!(build_plan(&list(vec![image(&edge)]), &UiRenderLimits::default()).is_ok());
    }
}
