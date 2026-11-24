//! Сцена кадра: что рисовать и откуда брать фон. Адаптер экран камеры не знает — сцену подставляет вызывающий.

use gpu_vulkan::backdrop::{BackdropFrameNumber, BackdropSource};
use gpu_vulkan::compositor::LogicalRect;
use ui_draw::draw_list::{DrawList, LiveImage};

use super::config::UiSaturationConfigError;
use crate::live_images::LiveImageInput;

/// Источник кадров фона («выход ISP»). Владелец картинки фона публикует из неё подложку с номером кадра.
///
/// Номер кадра — часть ключа пирамиды: тот же номер → пирамида переиспользуется, новый → перестраивается.
/// Картинка источника обязана лежать в `SHADER_READ_ONLY_OPTIMAL` и пережить прогон.
pub trait BackdropFeed {
    /// Подложка «кадра камеры» с номером `frame` (картинка та же, меняется метаданные, как у нового кадра ISP).
    fn backdrop_for_frame(&self, frame: BackdropFrameNumber) -> BackdropSource;
}

/// Всё, что нужно нарисовать на каждый кадр работы. Адаптер владеет сценой на всё время прогона.
pub struct SaturationScene {
    /// Что рисует UI (в логических px цели, портрет).
    pub draw_list: DrawList,
    /// Живые картинки кадра (приборы цвета); пусто, если их нет. Представления обязаны пережить прогон.
    pub live_images: Vec<(LiveImage, LiveImageInput)>,
    /// Куда компоновщик кладёт картинку «видоискателя» (и где её видит стекло), логические px цели.
    pub viewfinder: LogicalRect,
    /// Откуда берётся фон.
    pub backdrop: Box<dyn BackdropFeed>,
}

impl SaturationScene {
    /// Проверяет сцену (GPU не трогает).
    pub(crate) fn validate(&self) -> Result<(), UiSaturationConfigError> {
        let LogicalRect { width, height, .. } = self.viewfinder;
        if width == 0 || height == 0 {
            return Err(UiSaturationConfigError::ZeroViewfinderRect { width, height });
        }
        Ok(())
    }
}
