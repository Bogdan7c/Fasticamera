//! Адаптер «кадр показа» под стенд насыщения (`gpu_vulkan::saturation`): цена кадра UI на телефоне
//! (сессия 16a, пакет P4-ui).
//!
//! # Зачем (простыми словами)
//! На Adreno 740 миллисекунды GPU «в темпе vsync» — не цена работы: драйвер сам занижает частоту, когда
//! работы мало. Честная цена — пропускная способность при подряд идущих кадрах без показа. Этот адаптер
//! превращает то, что продукт делает на один показанный кадр ПОСЛЕ ISP, в [`SaturationWorkload`]: стенд
//! гонит его без пауз и считает кадры в секунду.
//!
//! # Один «кадр» работы
//! 1. Компоновщик (`ImageCompositor`) кладёт картинку «видоискателя» в цель размера экрана (внеэкранный
//!    `ColorTarget`, формат как у swapchain телефона; показа нет).
//! 2. Барьер между цветовыми проходами (`record_between_color_passes`).
//! 3. Проход UI (`UiRenderer::record_frame_marked`): пирамида размытия фона + фигуры, подписи, иконки, стекло.
//!
//! Состав кадра ([`FrameComposition`]) позволяет мерить части по отдельности и получать цену каждой
//! вычитанием; каденция фона ([`BackdropCadence`]) — переключать «новый кадр камеры на каждом кадре»
//! (пирамида строится каждый раз) и «тот же кадр» (пирамида переиспользуется, как на тиках 120 Гц).
//!
//! # Границы
//! * Адаптер НЕ знает экран камеры: список рисования, живые картинки и источник фона передаёт вызывающий
//!   ([`SaturationScene`]). Сценарий приложения позже подставит настоящий экран.
//! * У каждого слота своя цель (как у swapchain): соседние кадры в полёте не спорят за одну картинку.
//! * В `record_frame` память не выделяется самим адаптером; времена проходов копятся в `PassTimeLog` ядра.
//! * Ошибки освобождения слота (трейт не умеет их возвращать) не теряются: первая откладывается и
//!   возвращается следующим `record_frame`, счётчик — [`UiFrameSaturation::release_failures`].
//!
//! # Отступления от продукта
//! Поворота экрана нет (`Identity`); фон задаёт вызывающий (на ПК синтетика fp16, у продукта выход ISP);
//! метки времени (`ALL_COMMANDS`) между проходами слегка мешают конвейеризации, поэтому доли по проходам
//! ориентировочные, а цена кадра — это кадры в секунду стенда.

mod config;
mod pass_timer;
mod record;
mod scene;
#[cfg(feature = "test-support")]
mod synthetic;

use gpu_vulkan::color_target::{ColorTarget, ColorTargetDesc, ColorTargetPreviousUse};
use gpu_vulkan::compositor::ImageCompositor;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::saturation::{
    DigestError, FrameRecording, PassTimeSummary, SaturationWorkload, SlotId, WorkloadDigest, digest_color_target,
};

pub use self::config::{
    BackdropCadence, DEFAULT_PASS_TIME_WINDOW_FRAMES, DEFAULT_TARGET_EXTENT, DEFAULT_TARGET_FORMAT, FrameComposition,
    UiFrameSaturationConfig, UiSaturationConfigError,
};
pub use self::scene::{BackdropFeed, SaturationScene};

use self::pass_timer::PassTimer;
use crate::config::UiRendererConfig;
use crate::counters::UiRenderCounters;
use crate::error::UiRenderError;
use crate::renderer::UiRenderer;

/// Ошибка создания адаптера. Настройки проверяются ДО создания объектов GPU.
#[derive(Debug, thiserror::Error)]
pub enum UiSaturationError {
    /// Неверные настройки или сцена; GPU не трогали.
    #[error("неверные настройки адаптера кадра UI: {0}")]
    InvalidConfig(#[from] UiSaturationConfigError),
    /// Не удалось создать цель, компоновщик или метки времени.
    #[error("ресурсы GPU адаптера кадра UI: {0}")]
    Gpu(#[from] GpuError),
    /// Не удалось создать рендер UI.
    #[error("рендер UI адаптера: {0}")]
    Renderer(#[from] UiRenderError),
}

/// Кадр показа (компоновщик + UI) как работа стенда насыщения.
pub struct UiFrameSaturation {
    config: UiFrameSaturationConfig,
    scene: SaturationScene,
    /// Контекст GPU (нужен барьерам и очистке между проходами).
    context: GpuContext,
    /// Порядок полей = порядок уничтожения: сначала рендер и компоновщик, потом цели (контекст держит каждый).
    renderer: UiRenderer,
    compositor: ImageCompositor,
    /// По цели на слот.
    targets: Vec<ColorTarget>,
    /// Рисовали ли в цель слота хоть раз (первый переход идёт из `NotUsedYet`).
    target_used: Vec<bool>,
    timer: PassTimer,
    frames_recorded: u64,
    last_recorded_slot: Option<usize>,
    /// Первая ошибка освобождения слота, ещё не отданная вызывающему.
    pending_release_failure: Option<GpuError>,
    release_failures: u64,
}

impl UiFrameSaturation {
    /// Проверяет настройки и сцену (ДО GPU), создаёт цели по слотам, компоновщик, рендер UI и метки времени.
    pub fn new(
        context: &GpuContext,
        config: UiFrameSaturationConfig,
        scene: SaturationScene,
    ) -> Result<Self, UiSaturationError> {
        config.validate()?;
        scene.validate()?;
        let slots = config.frames_in_flight.get();
        let target_format = config.target_format.vk_format();
        let mut targets = Vec::with_capacity(slots);
        for _ in 0..slots {
            targets.push(ColorTarget::new(
                context,
                &ColorTargetDesc {
                    extent: config.target_extent,
                    format: config.target_format,
                    label: "saturation-ui-frame",
                },
            )?);
        }
        let compositor = ImageCompositor::new(context, target_format, slots)?;
        let mut renderer_config = UiRendererConfig::new(target_format, slots);
        renderer_config.pyramid = config.pyramid;
        let renderer = UiRenderer::new(context, &renderer_config)?;
        let timer = PassTimer::new(
            context,
            slots,
            config.composition.pass_labels(),
            config.pass_time_window_frames,
        )?;
        Ok(Self {
            config,
            scene,
            context: context.clone(),
            renderer,
            compositor,
            targets,
            target_used: vec![false; slots],
            timer,
            frames_recorded: 0,
            last_recorded_slot: None,
            pending_release_failure: None,
            release_failures: 0,
        })
    }

    /// Настройки, с которыми создан адаптер.
    pub fn config(&self) -> &UiFrameSaturationConfig {
        &self.config
    }

    /// Сколько кадров записано успешно (прогрев и хвост тоже).
    pub fn frames_recorded(&self) -> u64 {
        self.frames_recorded
    }

    /// Слот последнего записанного кадра: в его цели лежит последняя картинка (для readback в тестах).
    pub fn last_recorded_slot(&self) -> Option<usize> {
        self.last_recorded_slot
    }

    /// Цель слота. `None` — слота нет. Читать можно только когда GPU закончил кадр (после прогона стенда).
    pub fn target(&self, slot: usize) -> Option<&ColorTarget> {
        self.targets.get(slot)
    }

    /// Дайджест цели последнего записанного кадра (то, что получил бы экран). Звать после конца прогона стенда и
    /// `wait_idle`: чтение блокирующее, вне окна замера; работает без `test-support`. Цель после чтения остаётся в
    /// прежней раскладке. Кадров ещё не было — [`DigestError::NothingToRead`].
    pub fn output_digest(&self) -> Result<WorkloadDigest, DigestError> {
        let nothing = || DigestError::NothingToRead {
            what: "кадр показа ещё не записан".to_owned(),
        };
        let slot = self.last_recorded_slot.ok_or_else(nothing)?;
        let target = self.targets.get(slot).ok_or_else(nothing)?;
        // Последняя запись в цель — проход рисования (компоновщик или UI): состояние «Rendered».
        let digest = digest_color_target(target, ColorTargetPreviousUse::Rendered)?;
        Ok(WorkloadDigest::single("target", digest))
    }

    /// Счётчики рендера UI (перестройки и переиспользования пирамиды и т. д.).
    pub fn renderer_counters(&self) -> UiRenderCounters {
        self.renderer.counters()
    }

    /// Времена проходов за последние кадры (метки — по составу кадра).
    pub fn pass_times(&self) -> PassTimeSummary {
        self.timer.summary()
    }

    /// Забыть накопленные времена проходов.
    pub fn clear_pass_times(&mut self) {
        self.timer.clear();
    }

    /// Сколько раз освобождение слота закончилось ошибкой (в норме 0).
    pub fn release_failures(&self) -> u64 {
        self.release_failures
    }

    /// Запоминает ошибку освобождения: первая будет возвращена следующим `record_frame`.
    fn note_release_failure(&mut self, error: GpuError) {
        self.release_failures += 1;
        self.pending_release_failure.get_or_insert(error);
    }
}

impl SaturationWorkload for UiFrameSaturation {
    fn record_frame(&mut self, recording: FrameRecording) -> Result<(), GpuError> {
        self.record_one_frame(recording)
    }

    fn release_slot_after_fence(&mut self, slot: SlotId) {
        let index = slot.index();
        if self.config.composition.includes_compositor()
            && let Err(error) = self.compositor.release_slot_after_fence(index)
        {
            self.note_release_failure(error);
        }
        if self.config.composition.includes_ui()
            && let Err(error) = self.renderer.release_slot_after_fence(index)
        {
            self.note_release_failure(record::renderer_error_as_gpu_error(error));
        }
        if let Err(error) = self.timer.collect_after_fence(index) {
            self.note_release_failure(error);
        }
    }
}
