//! Фон кадра показа из настоящего выхода ISP видоискателя.
//!
//! Продукт кладёт под стекло картинку, которую только что записал ISP. Стенд делает так же: [`IspBackdrop`]
//! владеет работой `ViewfinderSaturation` (продуктовые умолчания, читатель — компоновщик, `Fragment`), её
//! конструктор уже записал один настоящий кадр ISP, а картинка лежит в `SHADER_READ_ONLY_OPTIMAL`.
//! Номер кадра фона задаёт каденция адаптера кадра показа: тот же номер — пирамида переиспользуется,
//! новый — перестраивается (как у нового кадра камеры).
//!
//! `BackdropFeed::backdrop_for_frame` не возвращает ошибок, а публикация подложки с окном fallible. Окно
//! проверяется один раз при создании (содержимое помещается в картинку); если на кадре публикация всё же
//! откажет, фон берётся целиком, а отказ ПЕРЕСЧИТЫВАЕТСЯ счётчиком — строка работы станет провалом
//! `BackdropFailed`, а не тихо неверной цифрой.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpu_vulkan::backdrop::{BackdropFrameNumber, BackdropSource, SubmissionSerial};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::source_window::SourceWindow;
use isp_vulkan::saturation::{SaturationWorkError, ViewfinderSaturation, ViewfinderSaturationConfig};
use ui_render_vulkan::saturation::BackdropFeed;

use super::inputs::{PipelineKind, ProductInputs};

/// Счётчик отказов публикации фона (делится между фоном и исполнителем).
#[derive(Debug, Clone, Default)]
pub struct BackdropFailures(Arc<AtomicU64>);

impl BackdropFailures {
    pub fn count(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    fn note(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// Фон из картинки ISP.
pub struct IspBackdrop {
    /// Держит картинку и память RAW-кадра, пока живёт фон.
    isp: ViewfinderSaturation,
    window: SourceWindow,
    failures: BackdropFailures,
}

impl IspBackdrop {
    /// Создаёт ISP с продуктовыми умолчаниями и проверяет, что подложка публикуется.
    pub fn new(
        context: &GpuContext,
        inputs: &ProductInputs,
        isp_slots: usize,
        failures: BackdropFailures,
    ) -> Result<Self, SaturationWorkError> {
        let mut config = ViewfinderSaturationConfig::default();
        config.isp.frame_slots = isp_slots;
        let params = inputs.params(PipelineKind::Viewfinder)?;
        let isp = ViewfinderSaturation::new_synthetic(context, config, params)?;
        let image = isp.image().ok_or_else(|| SaturationWorkError::EmptyFrame {
            what: "ISP фона не записал картинку".to_owned(),
        })?;
        let window = image.content_window();
        // Окно проверяется ОДИН раз здесь: отказ — ошибка создания работы, а не молчаливая подмена на кадре.
        BackdropSource::publish(
            image.storage_image(),
            image.color_space(),
            BackdropFrameNumber(0),
            SubmissionSerial(0),
        )
        .with_window(window)?;
        Ok(Self { isp, window, failures })
    }
}

impl BackdropFeed for IspBackdrop {
    fn backdrop_for_frame(&self, frame: BackdropFrameNumber) -> BackdropSource {
        let image = self
            .isp
            .image()
            .unwrap_or_else(|| unreachable!("наличие картинки проверено в конструкторе"));
        let published = BackdropSource::publish(image.storage_image(), image.color_space(), frame, SubmissionSerial(0));
        match published.clone().with_window(self.window) {
            Ok(source) => source,
            Err(error) => {
                // Невозможно после проверки в конструкторе; если случилось — это видно в результате.
                tracing::error!(target: "fasticamera::saturation", %error, "окно фона не публикуется");
                self.failures.note();
                published
            }
        }
    }
}
