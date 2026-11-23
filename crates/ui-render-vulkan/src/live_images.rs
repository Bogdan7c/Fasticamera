//! Живые картинки кадра (сессия 14h): изображения, которые владелец (рендер камеры) посчитал на GPU и
//! отдаёт UI на ОДИН кадр. Рендер UI их не загружает, не кэширует и не освобождает — только привязывает
//! к шейдеру и читает.
//!
//! # Договор производителя (обязателен; рендер UI его не проверяет и не может проверить)
//! К моменту, когда рендер UI записывает проход рисования, в ТОМ ЖЕ командном буфере и раньше него:
//! * картинка полностью записана (например, compute-шейдером прибора);
//! * её раскладка — `SHADER_READ_ONLY_OPTIMAL`, а запись видна фрагментному шейдеру: барьер
//!   [`StorageImage::record_compute_write_to_fragment_read`](gpu_vulkan::storage_image::StorageImage::record_compute_write_to_fragment_read)
//!   (`COMPUTE_SHADER` → `FRAGMENT_SHADER`, `SHADER_WRITE` → `SHADER_READ`);
//! * пиксели — цвет ЭКРАНА (кодировка и основные цвета текущего дисплея), альфа ПРЯМАЯ (не предумноженная),
//!   фон прозрачный; формат — любой, читаемый с линейной фильтрацией (`Rgba16Float` у приборов цвета);
//! * картинка и её представление живут, пока GPU не закончит кадр: владелец освобождает их по fence слота
//!   так же, как подложку стекла.
//!
//! Таблица заимствована на время записи кадра: рендер UI не хранит ни представления, ни картинки.

use gpu_vulkan::storage_image::SampledImageView;
use ui_draw::draw_list::LiveImage;
use ui_draw::picture::PictureSize;

/// Готовая к чтению живая картинка: представление и размер (из `extent` представления).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveImageInput {
    /// Представление с usage `SAMPLED`; договор раскладки и барьера — в документации модуля.
    pub view: SampledImageView,
}

impl LiveImageInput {
    /// Размер картинки в пикселях; для `ImageFit::Cover` и эталона. `None` у вырожденного размера:
    /// такую картинку рендер считает отсутствующей (деление на ноль в масштабе выборки).
    pub(crate) fn size(&self) -> Option<PictureSize> {
        let extent = self.view.extent;
        (extent.width > 0 && extent.height > 0).then(|| PictureSize::new(extent.width, extent.height))
    }
}

/// Таблица живых картинок одного кадра: какая [`LiveImage`] лежит в каком представлении.
///
/// Картинки, которых в таблице нет, на этом кадре отсутствуют: элементы с таким источником не рисуются
/// (счётчик `live_images_missing`), кадр не падает.
#[derive(Debug, Clone, Copy)]
pub struct LiveImages<'a> {
    entries: &'a [(LiveImage, LiveImageInput)],
}

impl<'a> LiveImages<'a> {
    /// Живых картинок на кадре нет.
    pub const fn none() -> Self {
        Self { entries: &[] }
    }

    /// Таблица из пар «картинка → представление». При повторе ключа действует первая запись.
    pub const fn new(entries: &'a [(LiveImage, LiveImageInput)]) -> Self {
        Self { entries }
    }

    /// Картинка кадра: `None`, если её нет в таблице или её размер вырожден.
    pub(crate) fn get(&self, image: LiveImage) -> Option<(LiveImageInput, PictureSize)> {
        let input = self.entries.iter().find(|(key, _)| *key == image)?.1;
        Some((input, input.size()?))
    }
}

#[cfg(test)]
mod tests {
    use ash::vk;
    use ash::vk::Handle;

    use super::*;

    fn input(width: u32, height: u32, raw_view: u64) -> LiveImageInput {
        LiveImageInput {
            view: SampledImageView {
                view: vk::ImageView::from_raw(raw_view),
                extent: vk::Extent2D { width, height },
                format: vk::Format::R16G16B16A16_SFLOAT,
            },
        }
    }

    #[test]
    fn lookup_returns_the_entry_and_its_size_and_none_is_empty() {
        let entries = [(LiveImage::ColorScope, input(64, 32, 7))];
        let table = LiveImages::new(&entries);
        let (found, size) = table.get(LiveImage::ColorScope).unwrap();
        assert_eq!(found.view.view.as_raw(), 7);
        assert_eq!(size, PictureSize::new(64, 32));
        assert!(LiveImages::none().get(LiveImage::ColorScope).is_none());
    }

    #[test]
    fn first_entry_wins_and_degenerate_size_counts_as_absent() {
        let entries = [
            (LiveImage::ColorScope, input(8, 8, 1)),
            (LiveImage::ColorScope, input(16, 16, 2)),
        ];
        let table = LiveImages::new(&entries);
        assert_eq!(table.get(LiveImage::ColorScope).unwrap().0.view.view.as_raw(), 1);
        let empty = [(LiveImage::ColorScope, input(0, 8, 3))];
        assert!(LiveImages::new(&empty).get(LiveImage::ColorScope).is_none());
    }
}
