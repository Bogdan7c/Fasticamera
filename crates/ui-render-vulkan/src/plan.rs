//! План кадра: список рисования → байты буферов экземпляров и пакеты вызовов рисования.
//!
//! Чистая логика без GPU. Все проверки (словарь, пределы) здесь — до первой записи команд, поэтому
//! отвергнутый кадр не оставляет следов ни в цели, ни в пирамиде.

use ash::vk;
use ui_draw::draw_list::{ClippedLeaf, DrawItem, DrawList, ImageSource, LiveImage, clipped_leaves};
use ui_draw::geometry::PxRect;
use ui_draw::picture::UiPicture;

use crate::config::UiRenderLimits;
use crate::error::{LimitKind, UiRenderError};
use crate::live_images::LiveImages;
use crate::pack::{ImageSamplerSlot, pack_glass_group, pack_icon, pack_image, pack_shape, pack_text};

/// Вид пакета: каждому соответствует свой конвейер.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchKind {
    Shape,
    Glyph,
    Glass,
    Image,
}

/// Прямоугольная обрезка пакета в ЛОГИЧЕСКИХ пикселях (до поворота цели).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum BatchClip {
    /// Обрезки нет: рисуется вся цель.
    WholeTarget,
    /// Целочисленный прямоугольник пикселей, оставленных обрезкой ([`ui_draw::draw_list::clip_pixel_rect`]).
    /// Он целый ещё до поворота цели, поэтому поворот и scissor не округляют его границы.
    PixelRect(PxRect),
}

/// Подряд идущие элементы одного вида и с одной обрезкой: один вызов рисования, порядок рисования сохраняется.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Batch {
    pub kind: BatchKind,
    /// Обрезка всех элементов пакета; соседи с другой обрезкой в пакет не склеиваются.
    pub clip: BatchClip,
    /// Номер первого экземпляра в буфере своего вида.
    pub first_instance: u32,
    pub instance_count: u32,
}

/// Что лежит в одном сэмплере картинок кадра. Номер в списке [`FramePlan::sampler_slots`] = номер сэмплера
/// в шейдере. Картинки из памяти и живые делят одни и те же `MAX_IMAGE_BINDINGS` привязок.
#[derive(Debug, Clone)]
pub(crate) enum SamplerSlot {
    /// Картинка из памяти: текстуру даёт кэш ([`crate::picture_cache::PictureCache`]).
    Picture(UiPicture),
    /// Живая картинка с GPU: привязывается напрямую по представлению, без загрузки, кэша и вытеснения.
    Live { image: LiveImage, view: vk::ImageView },
}

/// Готовый к загрузке кадр.
#[derive(Debug, Default)]
pub(crate) struct FramePlan {
    pub shape_bytes: Vec<u8>,
    pub glyph_bytes: Vec<u8>,
    pub group_bytes: Vec<u8>,
    pub member_bytes: Vec<u8>,
    pub image_bytes: Vec<u8>,
    /// Разные источники картинок кадра в порядке первого появления: номер в списке = номер сэмплера в шейдере.
    pub sampler_slots: Vec<SamplerSlot>,
    pub batches: Vec<Batch>,
    /// Область возможного вклада каждой стеклянной группы кадра (логические px), по номеру группы: из неё
    /// рендер делает scissor отдельного вызова рисования группы.
    pub glass_scissors: Vec<PxRect>,
    pub shape_count: u32,
    pub glyph_count: u32,
    pub group_count: u32,
    /// Элементов-картинок в кадре (картинки из памяти и нарисованные живые).
    pub image_count: u32,
    /// Из них живых картинок, найденных в таблице кадра.
    pub live_images_drawn: u32,
    /// Элементов с живой картинкой, которой в таблице нет: в план не попали.
    pub live_images_missing: u32,
}

impl FramePlan {
    /// Картинки из памяти, которые нужны кадру (для кэша текстур), в порядке сэмплеров.
    pub(crate) fn pictures(&self) -> impl Iterator<Item = &UiPicture> {
        self.sampler_slots.iter().filter_map(|slot| match slot {
            SamplerSlot::Picture(picture) => Some(picture),
            SamplerSlot::Live { .. } => None,
        })
    }

    /// Есть ли в кадре стекло (нужна ли пирамида размытия).
    pub(crate) fn has_glass(&self) -> bool {
        self.group_count > 0
    }

    /// Номер сэмплера кадра для картинки из памяти: та же картинка (по версии) получает прежний номер.
    fn sampler_of_picture(&mut self, picture: &UiPicture) -> usize {
        let known = self
            .sampler_slots
            .iter()
            .position(|slot| matches!(slot, SamplerSlot::Picture(known) if known.version() == picture.version()));
        known.unwrap_or_else(|| {
            self.sampler_slots.push(SamplerSlot::Picture(picture.clone()));
            self.sampler_slots.len() - 1
        })
    }

    /// Номер сэмплера кадра для живой картинки: один сэмплер на картинку, сколько бы элементов её ни рисовало.
    fn sampler_of_live(&mut self, image: LiveImage, view: vk::ImageView) -> usize {
        let known = self
            .sampler_slots
            .iter()
            .position(|slot| matches!(slot, SamplerSlot::Live { image: known, .. } if *known == image));
        known.unwrap_or_else(|| {
            self.sampler_slots.push(SamplerSlot::Live { image, view });
            self.sampler_slots.len() - 1
        })
    }

    fn push_batch(&mut self, kind: BatchKind, clip: BatchClip, first_instance: u32, added: u32) {
        if added == 0 {
            return;
        }
        match self.batches.last_mut() {
            Some(last) if last.kind == kind && last.clip == clip => last.instance_count += added,
            _ => self.batches.push(Batch {
                kind,
                clip,
                first_instance,
                instance_count: added,
            }),
        }
    }
}

/// Лист списка, который попадёт в кадр: общее решение «рисовать или пропустить» для подсчёта пределов,
/// проверки картинок и упаковки (чтобы они не могли разойтись).
struct PlannedLeaf<'a> {
    /// Номер элемента верхнего уровня (для сообщений об ошибках упаковки).
    index: usize,
    item: &'a DrawItem,
    /// Границы листа без обрезки: от них зависят вершины квада.
    bounds: PxRect,
    clip: BatchClip,
    /// Квад рисования стекла, уже обрезанный пикселями обрезки: из него рендер делает scissor группы.
    /// Для листа без обрезки — прежний квад (`None` у вырожденного стекла, как и раньше).
    glass_scissor: Option<PxRect>,
}

/// Решает судьбу листа. `None` — лист пропускается целиком: нечего рисовать (нет границ), обрезка
/// схлопнулась или квад рисования лежит вне пикселей обрезки.
fn plan_leaf(leaf: ClippedLeaf<'_>) -> Option<PlannedLeaf<'_>> {
    let bounds = leaf.item.bounds_px()?;
    // Квад рисования уже границ только у стекла; у остальных это те же границы, второй раз их не считаем.
    let draw_rect = match leaf.item {
        DrawItem::Glass(group) => group.draw_rect_px(),
        _ => Some(bounds),
    };
    let (clip, glass_scissor) = match leaf.clip.pixel_rect() {
        None => (BatchClip::WholeTarget, draw_rect),
        Some(pixels) => (BatchClip::PixelRect(pixels), Some(draw_rect?.intersection(&pixels)?)),
    };
    Some(PlannedLeaf {
        index: leaf.top_level_index,
        item: leaf.item,
        bounds,
        clip,
        glass_scissor,
    })
}

/// Сколько экземпляров каждого вида даст список (без упаковки).
/// Элементы с живой картинкой, которой на кадре нет, не считаются: они не будут нарисованы.
/// Пропущенные обрезкой листья тоже не считаются.
fn count_instances(list: &DrawList, live: LiveImages<'_>) -> InstanceCounts {
    let mut counts = InstanceCounts::default();
    for planned in clipped_leaves(&list.items).filter_map(plan_leaf) {
        match planned.item {
            DrawItem::Shape(_) => counts.shapes += 1,
            DrawItem::Glass(_) => counts.groups += 1,
            DrawItem::Text(text) => counts.glyphs += text.run.glyphs.len(),
            DrawItem::Icon(_) => counts.glyphs += 1,
            DrawItem::Image(image) => {
                if live_image_is_available(&image.source, live) {
                    counts.images += 1;
                }
            }
            // Группы обрезки обход раскрывает, до этой точки доходят только листья.
            DrawItem::Clipped(_) => {}
        }
    }
    counts
}

/// Можно ли рисовать источник на этом кадре: картинка из памяти — всегда, живая — если она в таблице.
fn live_image_is_available(source: &ImageSource, live: LiveImages<'_>) -> bool {
    source.live_image().is_none_or(|image| live.get(image).is_some())
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
    for planned in clipped_leaves(&list.items).filter_map(plan_leaf) {
        let DrawItem::Image(image) = planned.item else { continue };
        // Живые картинки не загружаются: их размер ограничен самим владельцем изображения.
        let Some(picture) = image.source.as_picture() else {
            continue;
        };
        let size = picture.size();
        if size.width.max(size.height) > limits.max_image_side_px {
            return Err(UiRenderError::PictureTooLarge {
                version: picture.version(),
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
/// Элемент с живой картинкой, которой нет в `live`, тоже пропускается (и считается в `live_images_missing`):
/// это штатное состояние кадра (прибор ещё не посчитан или закрыт), а не ошибка списка.
pub(crate) fn build_plan(
    list: &DrawList,
    limits: &UiRenderLimits,
    live: LiveImages<'_>,
) -> Result<FramePlan, UiRenderError> {
    list.validate()?;
    let counts = count_instances(list, live);
    check_limit(LimitKind::ShapeInstances, counts.shapes, limits.max_shape_instances)?;
    check_limit(LimitKind::GlyphInstances, counts.glyphs, limits.max_glyph_instances)?;
    check_limit(LimitKind::GlassGroups, counts.groups, limits.max_glass_groups)?;
    // Лимит считает ЭЛЕМЕНТЫ-картинки; разных картинок их не больше, поэтому сэмплеров хватит.
    check_limit(LimitKind::Images, counts.images, limits.max_images)?;
    check_picture_sizes(list, limits)?;

    let mut plan = FramePlan::default();
    let mut member_total = 0_usize;
    for planned in clipped_leaves(&list.items).filter_map(plan_leaf) {
        let PlannedLeaf {
            index,
            item,
            bounds,
            clip,
            glass_scissor,
        } = planned;
        match item {
            DrawItem::Shape(shape) => {
                pack_shape(index, shape, &bounds, &mut plan.shape_bytes)?;
                plan.push_batch(BatchKind::Shape, clip, plan.shape_count, 1);
                plan.shape_count += 1;
            }
            DrawItem::Glass(group) => {
                // Вершины квада остаются прежними (`bounds` = консервативные границы группы). Точки средней
                // яркости берутся по отдельному прямоугольнику адаптации под самой панелью (решение 220).
                // Область, где группа может дать вклад, уже: фрагменты вне неё отсекает scissor
                // (`glass_scissors`), до шейдера они не доходят. Внутри обрезки область ещё и пересекается с ней;
                // прямоугольник адаптации яркости и вершины квада от обрезки не зависят.
                let luma_area = group.adaptation_rect_px().unwrap_or(bounds);
                if let Some(visible_draw_rect) = glass_scissor {
                    plan.glass_scissors.push(visible_draw_rect);
                }
                pack_glass_group(
                    index,
                    group,
                    &bounds,
                    &luma_area,
                    member_total,
                    &mut plan.group_bytes,
                    &mut plan.member_bytes,
                )?;
                member_total += group.members.len();
                plan.push_batch(BatchKind::Glass, clip, plan.group_count, 1);
                plan.group_count += 1;
            }
            DrawItem::Text(text) => {
                let added = pack_text(text, &mut plan.glyph_bytes) as u32;
                plan.push_batch(BatchKind::Glyph, clip, plan.glyph_count, added);
                plan.glyph_count += added;
            }
            DrawItem::Icon(icon) => {
                pack_icon(icon, &mut plan.glyph_bytes);
                plan.push_batch(BatchKind::Glyph, clip, plan.glyph_count, 1);
                plan.glyph_count += 1;
            }
            DrawItem::Image(image) => {
                let slot = match &image.source {
                    ImageSource::Picture(picture) => ImageSamplerSlot {
                        index: plan.sampler_of_picture(picture),
                        source_size: picture.size(),
                    },
                    ImageSource::Live(live_image) => {
                        let Some((input, size)) = live.get(*live_image) else {
                            plan.live_images_missing += 1;
                            continue;
                        };
                        plan.live_images_drawn += 1;
                        ImageSamplerSlot {
                            index: plan.sampler_of_live(*live_image, input.view.view),
                            source_size: size,
                        }
                    }
                };
                pack_image(index, image, &bounds, slot, &mut plan.image_bytes)?;
                plan.push_batch(BatchKind::Image, clip, plan.image_count, 1);
                plan.image_count += 1;
            }
            // Группы обрезки обход раскрывает, до этой точки доходят только листья.
            DrawItem::Clipped(_) => {}
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
    use ui_draw::draw_list::{ImageClip, ImageFit, ImageItem, ImageSource, ShapeItem};
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
            source: picture.clone().into(),
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

    /// Номер сэмплера, записанный в `index`-м экземпляре картинки (`uv.w` — 20-й `f32` экземпляра).
    fn texture_index_of_instance(plan: &FramePlan, index: usize) -> f32 {
        let start = index * IMAGE_INSTANCE_BYTES + 19 * 4;
        f32::from_ne_bytes(plan.image_bytes[start..start + 4].try_into().unwrap())
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
            LiveImages::none(),
        )
        .unwrap();
        assert_eq!(
            plan.batches,
            vec![
                Batch {
                    kind: BatchKind::Image,
                    clip: BatchClip::WholeTarget,
                    first_instance: 0,
                    instance_count: 2
                },
                Batch {
                    kind: BatchKind::Shape,
                    clip: BatchClip::WholeTarget,
                    first_instance: 0,
                    instance_count: 1
                },
                Batch {
                    kind: BatchKind::Image,
                    clip: BatchClip::WholeTarget,
                    first_instance: 2,
                    instance_count: 1
                },
            ],
            "соседние картинки — один вызов, фигура между ними разрывает пакет, порядок списка сохранён"
        );
        assert_eq!((plan.image_count, plan.shape_count), (3, 1));
        assert_eq!(plan.image_bytes.len(), 3 * IMAGE_INSTANCE_BYTES);
        // Две разные картинки → два сэмплера; повтор первой берёт прежний номер.
        assert_eq!(plan.sampler_slots.len(), 2);
        assert_eq!(plan.pictures().count(), 2);
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
            LiveImages::none(),
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
        let error = build_plan(
            &list(vec![shape(), image(&huge)]),
            &UiRenderLimits::default(),
            LiveImages::none(),
        )
        .unwrap_err();
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
        assert!(
            build_plan(
                &list(vec![image(&edge)]),
                &UiRenderLimits::default(),
                LiveImages::none()
            )
            .is_ok()
        );
    }

    // ---- Живые картинки (14h) ----

    fn scope_item() -> DrawItem {
        DrawItem::Image(ImageItem {
            source: ImageSource::Live(LiveImage::ColorScope),
            transform: Affine2::translate(30.0, 30.0),
            size_px: PxSize::new(40.0, 20.0),
            fit: ImageFit::Stretch,
            clip: ImageClip::None,
            opacity: Opacity::OPAQUE,
        })
    }

    fn scope_entry(raw_view: u64, width: u32, height: u32) -> (LiveImage, crate::live_images::LiveImageInput) {
        use ash::vk::Handle;
        (
            LiveImage::ColorScope,
            crate::live_images::LiveImageInput {
                view: gpu_vulkan::storage_image::SampledImageView {
                    view: vk::ImageView::from_raw(raw_view),
                    extent: vk::Extent2D { width, height },
                    format: vk::Format::R16G16B16A16_SFLOAT,
                },
            },
        )
    }

    #[test]
    fn present_live_image_shares_one_sampler_with_pictures_and_keeps_list_order() {
        let first = picture(4, 4);
        let entries = [scope_entry(11, 256, 128)];
        let plan = build_plan(
            &list(vec![image(&first), scope_item(), scope_item(), shape()]),
            &UiRenderLimits {
                max_images: 3,
                ..Default::default()
            },
            LiveImages::new(&entries),
        )
        .unwrap();
        // Картинка из памяти — сэмплер 0, прибор — сэмплер 1, второй элемент того же прибора — снова 1.
        assert_eq!(plan.sampler_slots.len(), 2);
        assert!(matches!(
            plan.sampler_slots[1],
            SamplerSlot::Live {
                image: LiveImage::ColorScope,
                ..
            }
        ));
        assert_eq!(
            [0, 1, 2].map(|index| texture_index_of_instance(&plan, index)),
            [0.0, 1.0, 1.0]
        );
        assert_eq!(
            (plan.image_count, plan.live_images_drawn, plan.live_images_missing),
            (3, 2, 0)
        );
        assert_eq!(
            plan.pictures().count(),
            1,
            "кэш текстур видит только картинки из памяти"
        );
    }

    #[test]
    fn missing_live_image_adds_no_instance_no_batch_and_no_sampler() {
        let plan = build_plan(
            &list(vec![shape(), scope_item(), shape()]),
            &UiRenderLimits::default(),
            LiveImages::none(),
        )
        .unwrap();
        assert_eq!(
            (plan.image_count, plan.live_images_drawn, plan.live_images_missing),
            (0, 0, 1)
        );
        assert!(plan.image_bytes.is_empty() && plan.sampler_slots.is_empty());
        assert_eq!(
            plan.batches,
            vec![Batch {
                kind: BatchKind::Shape,
                clip: BatchClip::WholeTarget,
                first_instance: 0,
                instance_count: 2
            }],
            "фигуры по обе стороны отсутствующей картинки склеиваются в один вызов"
        );
    }

    #[test]
    fn missing_live_images_do_not_count_against_the_image_limit() {
        let one = picture(4, 4);
        let plan = build_plan(
            &list(vec![scope_item(), image(&one), scope_item()]),
            &UiRenderLimits {
                max_images: 1,
                ..Default::default()
            },
            LiveImages::none(),
        )
        .unwrap();
        assert_eq!((plan.image_count, plan.live_images_missing), (1, 2));
    }

    #[test]
    fn degenerate_live_extent_counts_as_missing() {
        let entries = [scope_entry(5, 0, 64)];
        let plan = build_plan(
            &list(vec![scope_item()]),
            &UiRenderLimits::default(),
            LiveImages::new(&entries),
        )
        .unwrap();
        assert_eq!((plan.image_count, plan.live_images_missing), (0, 1));
    }

    #[test]
    fn live_cover_scale_follows_the_extent_reported_by_the_view() {
        let entries = [scope_entry(5, 64, 16)];
        let mut cover = scope_item();
        if let DrawItem::Image(item) = &mut cover {
            item.fit = ImageFit::Cover;
            item.size_px = PxSize::new(20.0, 20.0);
        }
        let plan = build_plan(
            &list(vec![cover]),
            &UiRenderLimits::default(),
            LiveImages::new(&entries),
        )
        .unwrap();
        let float = |index: usize| f32::from_ne_bytes(plan.image_bytes[index * 4..index * 4 + 4].try_into().unwrap());
        // Рамка 20×20 над источником 64×16: ограничивает высота (масштаб 20/16), по x 1/(1,25·64) = 1/80, по y 1/20.
        assert_eq!((float(16), float(17)), (1.0 / 80.0, 1.0 / 20.0));
    }
}

#[cfg(test)]
#[path = "plan_clip_tests.rs"]
mod clip_tests;
