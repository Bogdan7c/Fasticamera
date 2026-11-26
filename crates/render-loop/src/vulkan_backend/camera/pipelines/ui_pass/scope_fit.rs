//! Совпадение размера картинки прибора с размером, в котором её рисует экран.
//!
//! Экран просит у рендера картинку прибора размером `size_px` своего элемента рисования и кладёт её 1:1
//! (`ImageFit::Stretch` без масштаба; решение 203). Рендер же создаёт картинку не мгновенно: после смены размера
//! запроса (свайп прибора с прямоугольным на квадратный вектороскоп, поворот телефона) UI-кадр уже рисует элемент
//! НОВОГО размера, а действующая картинка ещё СТАРОГО — она остаётся показанной, пока первая запись не заменит её
//! (`ScopePass::prepare_between_frames`). Растянутая в чужой размер картинка — овал вместо круга вектороскопа.
//!
//! Правило: картинка прибора отдаётся рисованию UI на кадре, только если КАЖДЫЙ элемент рисования картинки
//! прибора этого кадра имеет размер, равный размеру картинки. Иначе элемент на этом кадре не рисуется
//! (`live_images_missing`) — один-два кадра панель пуста, зато никогда не бывает растянутой картинки.

use gpu_vulkan::storage_image::SampledImageView;
use ui_draw::draw_list::{DrawItem, DrawList, ImageSource, LiveImage, clipped_leaves};

/// Годится ли `picture` для всех элементов картинки прибора списка рисования. Нет таких элементов — годится
/// (рисовать нечего, отдача безвредна и ничего не меняет).
pub(super) fn scope_items_fit_picture(draw_list: &DrawList, picture: &SampledImageView) -> bool {
    let (width, height) = (picture.extent.width as f32, picture.extent.height as f32);
    // Листья списка вместе с содержимым групп обрезки (прокручиваемых списков): картинка прибора может лежать и там.
    clipped_leaves(&draw_list.items).all(|leaf| match leaf.item {
        DrawItem::Image(image) if image.source == ImageSource::Live(LiveImage::ColorScope) => {
            image.size_px.width == width && image.size_px.height == height
        }
        // Остальные листья картинки прибора не рисуют; группы обрезки обход раскрывает и сам не выдаёт.
        DrawItem::Shape(_)
        | DrawItem::Glass(_)
        | DrawItem::Text(_)
        | DrawItem::Icon(_)
        | DrawItem::Image(_)
        | DrawItem::Clipped(_) => true,
    })
}

#[cfg(test)]
mod tests {
    use ash::vk;
    use ui_draw::color::Opacity;
    use ui_draw::draw_list::{ImageClip, ImageFit, ImageItem};
    use ui_draw::geometry::PxSize;
    use ui_draw::light::LightDirection;
    use ui_draw::transform::Affine2;

    use super::*;

    fn picture(width: u32, height: u32) -> SampledImageView {
        SampledImageView {
            view: vk::ImageView::null(),
            extent: vk::Extent2D { width, height },
            format: vk::Format::R16G16B16A16_SFLOAT,
        }
    }

    fn scope_item(width: f32, height: f32) -> DrawItem {
        DrawItem::Image(ImageItem {
            source: ImageSource::Live(LiveImage::ColorScope),
            transform: Affine2::translate(10.0, 10.0),
            size_px: PxSize::new(width, height),
            fit: ImageFit::Stretch,
            clip: ImageClip::None,
            opacity: Opacity::OPAQUE,
        })
    }

    fn list(items: Vec<DrawItem>) -> DrawList {
        let mut list = DrawList::new(LightDirection::from_angle_degrees(45.0));
        list.items = items;
        list
    }

    #[test]
    fn an_item_of_exactly_the_picture_size_is_drawn() {
        assert!(scope_items_fit_picture(
            &list(vec![scope_item(540.0, 360.0)]),
            &picture(540, 360)
        ));
    }

    #[test]
    fn a_stale_picture_of_another_size_would_be_stretched_so_it_is_not_handed_over() {
        // Новый вектороскоп квадратный, а картинка прежняя — прямоугольник waveform: овал.
        let square_item = list(vec![scope_item(360.0, 360.0)]);
        assert!(!scope_items_fit_picture(&square_item, &picture(540, 360)));
        // Обратный случай: элемент ещё прежнего размера, а картинка уже новая.
        assert!(!scope_items_fit_picture(
            &list(vec![scope_item(540.0, 360.0)]),
            &picture(360, 360)
        ));
        // Одна сторона совпала, другая нет — всё равно растяжение.
        assert!(!scope_items_fit_picture(
            &list(vec![scope_item(540.0, 300.0)]),
            &picture(540, 360)
        ));
    }

    #[test]
    fn one_mismatching_item_among_several_blocks_the_hand_over_and_a_list_without_scope_items_does_not() {
        let mixed = list(vec![scope_item(540.0, 360.0), scope_item(100.0, 100.0)]);
        assert!(!scope_items_fit_picture(&mixed, &picture(540, 360)));
        assert!(scope_items_fit_picture(&list(Vec::new()), &picture(540, 360)));
    }
}
