//! [`ViewfinderIsp`] — владелец слитого прохода видоискателя: изображение выхода, варианты пайплайна,
//! uniform-параметры, буферы статистики AWB и метки времени по слотам кадров.

use ash::vk;
use gpu_vulkan::backdrop::{BackdropFrameNumber, BackdropSource, SubmissionSerial};
use gpu_vulkan::compute::{BindingWrite, ComputePipeline, LocalSize, WorkgroupCount};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::gpu_timing::{GpuTimestamps, TimestampCollect, TimestampStage};
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use gpu_vulkan::storage_image::{
    SampledImageView, StorageImage, StorageImageDesc, StorageImagePreviousUse, StorageImageUsage,
};
use gpu_vulkan::swapchain::DisplayColorSpace;
use isp_model::params::IspParams;
use isp_model::statistics::AwbStatistics;

use super::awb_reduction::{AwbReduction, SubgroupArithmeticSupport};
use super::config::ViewfinderIspConfig;
use super::error::ViewfinderError;
use super::geometry::{ViewfinderGeometry, check_geometry};
use super::image::ViewfinderImage;
use super::retirement::{RecordSerial, RetirementQueue};
use super::stages::{IspStage, IspStageSet};
use super::statistics::AwbTileBuffers;
use super::uniforms::{VIEWFINDER_UNIFORM_BYTES, check_params, pack_uniforms};
use super::variants::{
    BINDING_AWB_TILES, BINDING_LENS_SHADING_MAP, BINDING_LOOK, BINDING_LOOK_LUT, BINDING_OUTPUT, BINDING_PARAMETERS,
    BINDING_SOURCE, BINDING_TONE_BASE, SourceKind, VariantCache, VariantKey,
};
use crate::frame_style::FrameStyleBuffers;
use crate::lens_shading_map::LensShadingMapBuffer;
use crate::tone_base::ToneBaseKind;

/// Имена точек меток времени: начало и конец dispatch ISP.
const TIMESTAMP_POINTS: [&str; 2] = ["isp-start", "isp-end"];
const POINT_START: usize = 0;
const POINT_END: usize = 1;

/// Всё, что нужно для записи одного кадра.
#[derive(Debug, Clone, Copy)]
pub struct ViewfinderFrame<'a> {
    /// Кадр камеры. Вызывающий УЖЕ записал его приём для стадии `ConsumerStage::Compute`
    /// (`MosaicSource::record_acquire_for`) и сам вернёт его камере; ISP барьеров владения не пишет.
    pub source: &'a MosaicSource,
    /// Параметры обработки (версия `CURRENT_ISP_PARAMS_VERSION`, демозаик Preview, гамма экрана).
    pub params: &'a IspParams,
    /// Полезная область, кадрирование и ориентация сенсора.
    pub geometry: ViewfinderGeometry,
    /// Цветовое пространство цели показа: параметры обязаны быть посчитаны под его гамму.
    pub display_color_space: DisplayColorSpace,
    /// Включённые стадии (продукт — `IspStageSet::ALL`).
    pub stages: IspStageSet,
    /// Номер кадра (для подложки стекла).
    pub frame: BackdropFrameNumber,
    /// Слот кадра: его fence вызывающий дождался (или слот ещё не использовался).
    pub frame_slot: usize,
    /// Кто читает изображение после записи (компоновщик — `Fragment`, стекло — `Compute`): для барьера
    /// видимости записи. Перед СЛЕДУЮЩЕЙ записью ISP ждёт чтений на всех шейдерных стадиях, а не только
    /// на этой: подложку стекла (`backdrop_source`) могут читать и другие стадии.
    pub reader: ConsumerStage,
}

/// Итог записи кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedViewfinderFrame {
    /// Что читать компоновщику: вся картинка (ёмкость) и формат. Содержимое кадра — `content`.
    pub output: SampledImageView,
    /// Размер содержимого записанного кадра: левый верхний прямоугольник `output` (читать только его).
    pub content: vk::Extent2D,
    /// Номер записанного кадра.
    pub frame: BackdropFrameNumber,
}

/// Что собрано со слота после его fence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewfinderSlotReport {
    /// GPU-время dispatch ISP (`NotReady` — ещё нет, никогда не ждём).
    pub isp_gpu_time: TimestampCollect,
    /// Статистика AWB последней записи слота; `None` — стадия была выключена, слот не записывался
    /// или статистику этой записи уже забрали.
    pub awb_statistics: Option<AwbStatistics>,
}

/// Что известно о слоте.
#[derive(Debug, Clone, Copy, Default)]
struct SlotState {
    /// Номер последней записи этого слота.
    last_record: Option<RecordSerial>,
    /// Статистика последней записи ещё не забрана.
    awb_pending: bool,
}

/// ISP видоискателя (см. модуль `viewfinder`).
///
/// # Контракт вызывающего (поток рендера)
/// * `record` для слота — только когда fence прошлого сабмита этого слота сигнален (переписываются
///   uniform-буфер, набор дескрипторов и буфер статистики слота);
/// * записанные команды отправляются в той же очереди и в том же порядке, в каком записаны
///   (картинка помнит свою раскладку по порядку записи);
/// * `collect(slot)` — после того, как вызывающий увидел fence слота, и ОБЯЗАТЕЛЬНО между двумя `record`
///   одного слота: он подтверждает, что набор дескрипторов слота свободен (иначе следующий `record`
///   слота с другим кадром камеры — `InvalidState`, см. `ComputePipeline::bind`), и разрешает уничтожить
///   выведенные из обращения объекты (`retirement`);
/// * подложку стекла брать заново каждый кадр (`backdrop_source`) или проверять `is_backdrop_current`
///   перед каждой записью чтения (контракт потребителя — модуль `gpu_vulkan::backdrop`);
/// * перед `Drop` GPU не исполняет команд с объектами ISP (как у остальных владельцев Vulkan).
pub struct ViewfinderIsp {
    variants: VariantCache,
    image: Option<ViewfinderImage>,
    retired_images: RetirementQueue<ViewfinderImage>,
    retired_pipelines: RetirementQueue<ComputePipeline>,
    uniform_buffers: Vec<HostBuffer>,
    /// Стиль кадра по слотам: блок основы тона (пишется каждый кадр: мал, меняется с подтяжкой AE), блок цвета вида
    /// и таблица вида (пишется, только когда сменилась).
    style_buffers: Vec<FrameStyleBuffers>,
    /// Карта виньетирования по слотам: пишется, только когда карта (или полезная область) изменилась.
    lens_shading_maps: Vec<LensShadingMapBuffer>,
    awb_buffers: AwbTileBuffers,
    timestamps: GpuTimestamps,
    slots: Vec<SlotState>,
    next_serial: RecordSerial,
    config: ViewfinderIspConfig,
    local_size: LocalSize,
    context: GpuContext,
}

impl std::fmt::Debug for ViewfinderIsp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ViewfinderIsp")
            .field("image", &self.image)
            .field("variants", &self.variants.len())
            .field("next_serial", &self.next_serial)
            .finish()
    }
}

impl ViewfinderIsp {
    /// Создаёт ISP: буферы по слотам и метки времени. Пайплайны и изображение — лениво, на первом кадре.
    pub fn new(context: &GpuContext, config: &ViewfinderIspConfig) -> Result<Self, ViewfinderError> {
        config.validate()?;
        let local_size = config.local_size()?;
        // Размер группы из конфига — против лимитов ЭТОГО устройства (спецификация гарантирует только 128 нитей).
        local_size.validate_for_device(context)?;
        // Быстрый путь статистики — только если устройство принимает SPIR-V с арифметикой subgroup.
        let awb_reduction = config
            .awb_reduction
            .resolve(SubgroupArithmeticSupport::of_device(context.capabilities()));
        tracing::debug!(
            target: "fasticamera::isp_viewfinder",
            ?awb_reduction,
            policy = ?config.awb_reduction,
            "способ сведения статистики AWB"
        );
        let uniform_buffers = (0..config.frame_slots)
            .map(|_| {
                HostBuffer::new(
                    context,
                    VIEWFINDER_UNIFORM_BYTES as u64,
                    HostBufferUsage::UniformParameters,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let style_buffers = (0..config.frame_slots)
            .map(|_| FrameStyleBuffers::new(context, config.look_lut))
            .collect::<Result<Vec<_>, _>>()?;
        let lens_shading_maps = (0..config.frame_slots)
            .map(|_| LensShadingMapBuffer::new(context, config.lens_shading_map))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            variants: VariantCache::new(
                config.pipeline_variant_capacity,
                local_size,
                config.frame_slots,
                awb_reduction,
            ),
            image: None,
            retired_images: RetirementQueue::default(),
            retired_pipelines: RetirementQueue::default(),
            uniform_buffers,
            style_buffers,
            lens_shading_maps,
            awb_buffers: AwbTileBuffers::new(context, config.frame_slots, config.awb_statistics.grid)?,
            timestamps: GpuTimestamps::new(context, config.frame_slots, &TIMESTAMP_POINTS)?,
            slots: vec![SlotState::default(); config.frame_slots],
            next_serial: RecordSerial(0),
            config: *config,
            local_size,
            context: context.clone(),
        })
    }

    /// Способ сведения статистики AWB (решён при создании по конфигу и возможностям устройства).
    pub fn awb_reduction(&self) -> AwbReduction {
        self.variants.awb_reduction()
    }

    /// Число слотов кадров.
    pub fn frame_slots(&self) -> usize {
        self.slots.len()
    }

    /// Изображение последнего записанного кадра (`None` — кадров ещё не было).
    pub fn image(&self) -> Option<&ViewfinderImage> {
        self.image.as_ref()
    }

    /// Подложка стекла из последнего записанного кадра; `written_by` — номер сабмита, в который
    /// вызывающий отправил эту запись (`None` — кадров ещё не было).
    pub fn backdrop_source(&self, written_by: SubmissionSerial) -> Option<BackdropSource> {
        self.image.as_ref().and_then(|image| {
            image
                .backdrop_source(written_by)
                .inspect_err(|error| {
                    tracing::error!(target: "fasticamera::isp_viewfinder", %error, "подложка стекла не опубликована");
                })
                .ok()
        })
    }

    /// Отцепляет текущую выходную картинку от ISP и отдаёт её вызывающему целиком (удержание последнего
    /// показанного кадра при смене потока): ISP больше в неё не пишет, следующая запись выделит новую
    /// картинку. Нулевая цена на горячем пути — ни копий, ни ожиданий GPU: картинка просто меняет владельца.
    ///
    /// Вызывающий обязан вернуть картинку через [`Self::retire_detached_image`], а не бросить: освобождение
    /// ждёт GPU (сабмиты, читавшие картинку, ещё могут быть в полёте).
    pub fn detach_image(&mut self) -> Option<ViewfinderImage> {
        self.image.take()
    }

    /// Возвращает отцепленную картинку на уничтожение: она освободится, когда завершится первая запись ISP,
    /// сделанная ПОСЛЕ этого вызова (сабмиты идут в порядке записи на одной очереди, значит все чтения
    /// удержанной картинки к тому времени закончены). Вызывать до `record` кадра, который не читает картинку.
    pub fn retire_detached_image(&mut self, image: ViewfinderImage) {
        self.retired_images.retire(image, self.next_serial);
    }

    /// Актуальна ли подложка: опубликована из ТЕКУЩЕГО изображения видоискателя. `false` — изображение
    /// с тех пор пересоздано (смена размера) и старое ждёт уничтожения: подложку надо выбросить, не
    /// записав с ней ни одной команды (см. контракт потребителя в `gpu_vulkan::backdrop`).
    pub fn is_backdrop_current(&self, backdrop: &BackdropSource) -> bool {
        self.image
            .as_ref()
            .is_some_and(|image| backdrop.is_from(image.storage_image()))
    }

    /// Сколько вариантов пайплайна создано и держится в кэше.
    pub fn cached_pipeline_variants(&self) -> usize {
        self.variants.len()
    }

    /// Сколько объектов ждут завершения GPU перед уничтожением (изображения и пайплайны).
    pub fn retired_objects(&self) -> usize {
        self.retired_images.len() + self.retired_pipelines.len()
    }

    /// Записывает слитый проход кадра в `command_buffer` (вне динамического рендеринга).
    ///
    /// Порядок команд: сброс меток слота → обнуление статистики (если стадия включена) → барьер
    /// изображения «прошлое чтение → запись» → метка начала → dispatch → метка конца → барьер
    /// «запись → чтение `reader`» → барьер статистики «запись → хост».
    ///
    /// Все проверки (параметры, геометрия, статистика, слот, вариант, привязки, лимит групп dispatch) — до
    /// первой команды: при ошибке командный буфер не тронут.
    pub fn record(
        &mut self,
        command_buffer: vk::CommandBuffer,
        frame: &ViewfinderFrame<'_>,
    ) -> Result<RecordedViewfinderFrame, ViewfinderError> {
        let slot = self.check_slot(frame.frame_slot)?;
        check_params(frame.params, frame.display_color_space)?;
        let geometry = check_geometry(&frame.geometry, frame.source)?;
        let statistics_enabled = frame.stages.contains(IspStage::AwbStatistics);
        if statistics_enabled {
            self.config.awb_statistics.check_for_mosaic(frame.geometry.crop.size)?;
        }
        let serial = self.next_serial;
        self.ensure_image(geometry.allocation, frame.display_color_space, serial)?;
        let image = self.image.as_ref().ok_or(ViewfinderError::InvalidConfig {
            what: "изображение не создано (внутренняя ошибка порядка вызовов)".to_owned(),
        })?;
        // «Пропуск» виньетирования в параметрах вырезает стадию из прохода (константа специализации).
        let stages = frame
            .stages
            .resolved_for_lens_shading(&frame.params.lens_shading)
            .resolved_for_look_lut(&frame.params.look);
        let key = VariantKey {
            source: SourceKind::of(frame.source),
            format: image.format(),
            stages,
            tone_base: ToneBaseKind::of(frame.params),
        };
        if let Some(evicted) = self.variants.prepare(&self.context, key, serial)? {
            self.retired_pipelines.retire(evicted, serial);
        }
        let uniforms = pack_uniforms(frame.params, &geometry, &self.config.awb_statistics);
        self.uniform_buffers[slot].write_bytes(0, &uniforms)?;
        // Основа тона, цвет вида и таблица: таблица больше буфера — ошибка кадра ДО команд.
        self.style_buffers[slot].prepare(frame.params)?;
        let style = self.style_buffers[slot].bindings();
        // Карта больше буфера — ошибка кадра ДО команд; карта не изменилась — буфер не пишется.
        self.lens_shading_maps[slot].prepare(&frame.params.lens_shading, geometry.valid_area.size)?;
        let writes = [
            BindingWrite {
                binding: BINDING_SOURCE,
                resource: frame.source.compute_binding(),
            },
            BindingWrite {
                binding: BINDING_PARAMETERS,
                resource: self.uniform_buffers[slot].binding(),
            },
            BindingWrite {
                binding: BINDING_OUTPUT,
                resource: image.storage_image().storage_binding(),
            },
            BindingWrite {
                binding: BINDING_AWB_TILES,
                resource: self.awb_buffers.slot(slot)?.binding(),
            },
            BindingWrite {
                binding: BINDING_LENS_SHADING_MAP,
                resource: self.lens_shading_maps[slot].binding(),
            },
            BindingWrite {
                binding: BINDING_TONE_BASE,
                resource: style.tone_base,
            },
            BindingWrite {
                binding: BINDING_LOOK,
                resource: style.look,
            },
            BindingWrite {
                binding: BINDING_LOOK_LUT,
                resource: style.look_lut,
            },
        ];
        let pipeline = self.variants.pipeline_mut(&key)?;
        pipeline.bind(slot, &writes)?;
        let groups = WorkgroupCount::covering(geometry.output.width, geometry.output.height, self.local_size);
        // Лимит групп устройства и готовность набора — тоже до команд (без push-констант).
        pipeline.check_dispatch(slot, 0, groups)?;
        let awb_slot_buffer = self.awb_buffers.slot(slot)?;

        // --- Команды: дальше ошибок проверки быть не может ---
        self.timestamps.record_reset(command_buffer, slot)?;
        if statistics_enabled {
            awb_slot_buffer.record_clear_for_compute(command_buffer)?;
        }
        // WAR: ждём чтений прошлого кадра на ВСЕХ шейдерных стадиях — кроме записанного `reader`, кадр
        // могли читать потребители подложки (стекло в compute), о которых ISP не знает.
        image
            .storage_image()
            .record_prepare_for_compute_write_after_any_reader(command_buffer, image.current_use());
        // Метка начала — после завершения всех предыдущих команд: отрезок = только наш dispatch.
        self.timestamps
            .record_point(command_buffer, slot, POINT_START, TimestampStage::AllCommandsDone)?;
        self.variants
            .pipeline_mut(&key)?
            .record_dispatch(command_buffer, slot, &[], groups)?;
        self.timestamps
            .record_point(command_buffer, slot, POINT_END, TimestampStage::ComputeShaderDone)?;
        image
            .storage_image()
            .record_compute_write_to_sampled_read(command_buffer, frame.reader);
        if statistics_enabled {
            awb_slot_buffer.record_compute_write_to_host_read(command_buffer);
        }

        let output = image.sampled_view();
        if let Some(image) = self.image.as_mut() {
            image.mark_written(
                frame.frame,
                frame.display_color_space,
                geometry.output,
                StorageImagePreviousUse::SampledRead(frame.reader),
            );
        }
        self.slots[slot] = SlotState {
            last_record: Some(serial),
            awb_pending: statistics_enabled,
        };
        self.next_serial = RecordSerial(serial.0 + 1);
        Ok(RecordedViewfinderFrame {
            output,
            content: geometry.output,
            frame: frame.frame,
        })
    }

    /// Забирает итоги слота. Вызывать ПОСЛЕ того, как вызывающий увидел fence слота; никогда не ждёт
    /// GPU (метки, до которых GPU не дошёл, — `NotReady`). Заодно уничтожает объекты, которые
    /// больше не нужны ни одной незавершённой записи.
    pub fn collect(&mut self, frame_slot: usize) -> Result<ViewfinderSlotReport, ViewfinderError> {
        let slot = self.check_slot(frame_slot)?;
        let state = self.slots[slot];
        if let Some(completed) = state.last_record {
            self.retired_images.release_completed(completed);
            self.retired_pipelines.release_completed(completed);
        }
        // Fence слота сигнален (контракт): наборы дескрипторов слота снова можно переписывать.
        self.variants.release_slot_after_fence(slot)?;
        let isp_gpu_time = self.timestamps.try_collect(slot)?;
        let awb_statistics = if state.awb_pending {
            self.slots[slot].awb_pending = false;
            Some(self.awb_buffers.read(slot)?)
        } else {
            None
        };
        Ok(ViewfinderSlotReport {
            isp_gpu_time,
            awb_statistics,
        })
    }

    fn check_slot(&self, frame_slot: usize) -> Result<usize, ViewfinderError> {
        if frame_slot < self.slots.len() {
            Ok(frame_slot)
        } else {
            Err(ViewfinderError::FrameSlotOutOfRange {
                slot: frame_slot,
                slots: self.slots.len(),
            })
        }
    }

    /// Создаёт изображение нужной ЁМКОСТИ (при зуме она не меняется — картинка не пересоздаётся); прежнее
    /// (другой ёмкости) уходит в отложенное уничтожение.
    fn ensure_image(
        &mut self,
        extent: vk::Extent2D,
        color_space: DisplayColorSpace,
        serial: RecordSerial,
    ) -> Result<(), ViewfinderError> {
        if self.image.as_ref().is_some_and(|image| image.extent() == extent) {
            return Ok(());
        }
        let storage = StorageImage::new(
            &self.context,
            &StorageImageDesc {
                extent,
                format_policy: self.config.storage_format.policy(),
                usage: StorageImageUsage::ComputeWriteSampledRead,
            },
        )?;
        if let Some(previous) = self.image.replace(ViewfinderImage::new(storage, color_space)) {
            tracing::debug!(
                target: "fasticamera::isp_viewfinder",
                from = ?previous.extent(),
                to = ?extent,
                "изображение видоискателя пересоздано под новый размер"
            );
            self.retired_images.retire(previous, serial);
        }
        Ok(())
    }
}
