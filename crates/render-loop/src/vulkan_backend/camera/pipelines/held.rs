//! Удержанный кадр и всё, что нужно, чтобы показать его вместо живого изображения (сессия 13).
//!
//! # Как держим кадр
//! В момент начала удержания ТЕКУЩАЯ выходная картинка ISP отцепляется от него целиком
//! (`ViewfinderIsp::detach_image`) и переходит сюда: ни копий на GPU, ни ожиданий, ни транзитного usage.
//! ISP продолжает работать с НОВОЙ картинкой (её выделит следующая запись) — скрытые кадры холодного старта
//! нового потока не трогают удержанную. Картинка в `SHADER_READ_ONLY_OPTIMAL` с видимой записью (так её
//! оставил ISP), компоновщик читает её как любое изображение видоискателя.
//!
//! # Владение и уничтожение
//! Картинку держит `CameraPipelines`, как и всё остальное, что читают кадры в полёте: `DisplayChain::release`
//! уничтожает пайплайны только после того, как цель освободила GPU. При окончании удержания картинка
//! возвращается ISP (`retire_detached_image`) и освобождается, когда завершится его следующая запись, —
//! то есть после всех чтений удержанной картинки (одна очередь, порядок записи).
//!
//! # Окно чтения
//! Содержимое картинки — левый верхний прямоугольник (ёмкость может быть больше). Удержанный кадр читается
//! через [`SourceWindow`]: его зеркало (как было при показе) и увеличение за пальцем.

use gpu_vulkan::source_window::{HorizontalMirror, SourceWindow};
use isp_vulkan::viewfinder::ViewfinderImage;

use super::{CameraPipelines, FrameTarget, LOG_TARGET, ScreenContent};
use crate::viewfinder::EffectivePlacement;
use crate::vulkan_backend::camera::continuity::PictureMeta;
use camera_contract::zoom::PreviewMirror;

/// Что показать на экране для записи кадра: решение машины непрерывности, переведённое в команду.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShowFrameAs {
    /// Живое изображение ISP.
    Live,
    /// Удержанный последний кадр, увеличенный вокруг центра (≥ 1).
    Held {
        /// Увеличение.
        magnification: f32,
    },
    /// Чёрный.
    Black,
}

/// Удержанный кадр: картинка и то, что о ней известно.
pub(super) struct HeldPicture {
    pub(super) image: ViewfinderImage,
    pub(super) meta: PictureMeta,
}

/// Состояние показа, не принадлежащее ISP: зеркало живого изображения и удержанный кадр.
#[derive(Default)]
pub(super) struct ViewState {
    /// Удержанный кадр (`None` — удержания нет).
    pub(super) held: Option<HeldPicture>,
    /// Зеркало живого изображения: свойство ПОСЛЕДНЕГО записанного кадра ISP (камера кадра, `FramedCrop::mirror`).
    pub(super) live_mirror: HorizontalMirror,
    /// Окно, которым скомпонован удержанный кадр в последний раз (то же окно читает подложка стекла).
    pub(super) held_window: Option<SourceWindow>,
}

/// Отцепить живую картинку не удалось.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HoldError {
    /// У ISP нет картинки (кадров ещё не было или она уже отцеплена).
    #[error("у ISP нет изображения, которое можно удержать")]
    NoLiveImage,
}

/// Зеркало из словаря зума → горизонтальное зеркало компоновщика.
pub(in crate::vulkan_backend::camera) fn mirror_of(mirror: PreviewMirror) -> HorizontalMirror {
    match mirror {
        PreviewMirror::None => HorizontalMirror::Off,
        PreviewMirror::Horizontal => HorizontalMirror::On,
    }
}

impl CameraPipelines {
    /// Начинает удержание: текущая выходная картинка ISP переходит в удержанный кадр. Вызывать ДО записи
    /// кадра, который иначе перезаписал бы её (кадр нового потока). Прежний удержанный кадр (быть не должно)
    /// возвращается ISP на уничтожение.
    pub fn hold_live_image(&mut self, meta: PictureMeta) -> Result<(), HoldError> {
        let image = self.isp.detach_image().ok_or(HoldError::NoLiveImage)?;
        self.release_held_picture();
        self.view.held = Some(HeldPicture { image, meta });
        Ok(())
    }

    /// Заканчивает удержание: картинка уходит на отложенное уничтожение (после следующей записи ISP).
    pub fn release_held_picture(&mut self) {
        self.view.held_window = None;
        if let Some(held) = self.view.held.take() {
            self.isp.retire_detached_image(held.image);
        }
    }

    /// Зеркало живого изображения (для подложки стекла).
    pub(super) fn view_live_mirror(&self) -> HorizontalMirror {
        self.view.live_mirror
    }

    /// Окно, которым читается живое изображение: содержимое последнего кадра + его зеркало.
    pub(super) fn live_window(&self) -> Option<SourceWindow> {
        self.isp
            .image()
            .map(|image| image.content_window().with_mirror(self.view.live_mirror))
    }

    /// Компоновка удержанного кадра с увеличением `magnification`; нет удержанного кадра или компоновка
    /// отказала — чёрный с причиной (при отсутствии кадра — без причины: это учтёт машина непрерывности).
    pub(super) fn compose_held_or_black(
        &mut self,
        frame: &FrameTarget,
        placement: EffectivePlacement,
        magnification: f32,
    ) -> ScreenContent {
        let Some(held) = self.view.held.as_ref() else {
            return self.black(frame, None);
        };
        let view = held.image.sampled_view();
        let window = match held
            .image
            .content_window()
            .with_mirror(mirror_of(held.meta.mirror))
            .with_magnification(magnification)
        {
            Ok(window) => window,
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "увеличение удержанного кадра отвергнуто: без увеличения");
                held.image.content_window().with_mirror(mirror_of(held.meta.mirror))
            }
        };
        match self.compose_windowed(frame, view, window, placement) {
            Ok(destination) => {
                self.view.held_window = Some(window);
                ScreenContent::Held { destination }
            }
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "компоновка удержанного кадра не записана: чёрный кадр");
                self.black(frame, Some(error.to_string()))
            }
        }
    }

    /// Подложка стекла из удержанного кадра с тем же окном, которым он скомпонован на экран.
    pub(super) fn held_backdrop_source(&self) -> Option<gpu_vulkan::backdrop::BackdropSource> {
        let held = self.view.held.as_ref()?;
        let window = self.view.held_window?;
        // Кадр записан давно, задолго до любого читающего сабмита: номер сабмита записи 0 «в прошлом».
        let source = held
            .image
            .backdrop_source(gpu_vulkan::backdrop::SubmissionSerial(0))
            .and_then(|source| source.with_window(window))
            .inspect_err(|error| {
                tracing::error!(target: LOG_TARGET, %error, "подложка стекла из удержанного кадра не опубликована");
            })
            .ok()?;
        Some(source)
    }
}

/// Тестовые швы чтения картинок (фича `test-support`): читается ровно содержимое кадра, а не вся картинка.
#[cfg(feature = "test-support")]
impl CameraPipelines {
    /// Содержимое живого изображения ISP (`Ok(None)` — кадров ещё не было).
    pub fn read_live_picture(
        &self,
    ) -> Result<Option<crate::vulkan_backend::ViewfinderReadback>, gpu_vulkan::error::GpuError> {
        self.isp.image().map(read_content).transpose()
    }

    /// Содержимое удержанного кадра (`Ok(None)` — удержания нет).
    pub fn read_held_picture(
        &self,
    ) -> Result<Option<crate::vulkan_backend::ViewfinderReadback>, gpu_vulkan::error::GpuError> {
        self.view
            .held
            .as_ref()
            .map(|held| read_content(&held.image))
            .transpose()
    }
}

/// Читает левый верхний прямоугольник содержимого картинки (картинка может быть больше — ёмкость под зум).
#[cfg(feature = "test-support")]
fn read_content(
    image: &ViewfinderImage,
) -> Result<crate::vulkan_backend::ViewfinderReadback, gpu_vulkan::error::GpuError> {
    let whole = read_whole_image(image)?;
    let (capacity, content) = (image.extent(), image.content_extent());
    let pixels = (0..content.height)
        .flat_map(|y| (0..content.width).map(move |x| (y * capacity.width + x) as usize))
        .filter_map(|index| whole.get(index).copied())
        .collect();
    Ok(crate::vulkan_backend::ViewfinderReadback {
        width: content.width,
        height: content.height,
        pixels,
    })
}

#[cfg(feature = "test-support")]
fn read_whole_image(image: &ViewfinderImage) -> Result<Vec<[f32; 4]>, gpu_vulkan::error::GpuError> {
    gpu_vulkan::kit_test_support::read_storage_image_rgba(image.storage_image(), image.current_use())
}
