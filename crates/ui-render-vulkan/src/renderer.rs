//! [`UiRenderer`]: записывает проход UI (фигуры, текст, иконки, стекло) поверх уже нарисованного кадра.
//!
//! # Владение
//! Рендер владеет конвейерами, атласом SDF, буферами экземпляров по слотам кадра и пирамидой размытия
//! (одной на все слоты: её содержимое зависит только от фона, а чтение прошлых кадров защищено барьером
//! «чтение → запись» ступеней). Цель, командный буфер, fence и подложку держит вызывающий.
//!
//! # Порядок `record_frame`
//! 1. Все проверки и упаковка (в том числе пределы картинок) — ДО команд (ошибка не оставляет следов в цели и пирамиде).
//! 2. Если есть стекло: ступени под логический размер; при смене ключа фона — барьер чтения фона и перестройка.
//! 3. Картинки: память под новые выделяется ДО первой команды, копии пишутся в команды кадра сразу после метки
//!    `Start` (вне проходов), кэш и отложенное освобождение — `picture_cache`.
//! 4. Загрузка буферов слота и запись привязок (повтор тех же ресурсов — пустая операция).
//! 5. `record_between_color_passes`, затем один проход `Preserve`: пакеты по порядку списка.

use ash::vk;
use gpu_vulkan::color_target::ColorTargetPreviousUse;
use gpu_vulkan::compute::{BindingWrite, DescriptorResource};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::raster::{AttachmentLoad, ColorAttachment, DrawCall, RasterPass, record_between_color_passes};
use gpu_vulkan::sampled_texture::{SampledTexture, TextureDesc, TextureFormat};
use gpu_vulkan::source_window::SourceWindow;
use ui_draw::atlas::sdf_atlas;
use ui_draw::geometry::PxSize;

use crate::config::{MAX_IMAGE_BINDINGS, MAX_PYRAMID_LEVELS, UiRendererConfig};
use crate::counters::UiRenderCounters;
use crate::error::UiRenderError;
use crate::frame::{BackdropInput, FrameMarker, PyramidOutcome, UiFrame, UiFrameReport};
use crate::pack::FramePush;
use crate::picture_cache::PictureCache;
use crate::pipelines::{GLASS_FIRST_PYRAMID_BINDING, IMAGE_FIRST_SAMPLER_BINDING, UiPipelines, UiShaderSet};
use crate::plan::{Batch, BatchKind, FramePlan, build_plan};
use crate::pyramid::{Pyramid, PyramidKey, RebuildSource};
use crate::scissor::{GlassDrawArea, logical_rect_to_scissor};
use crate::slots::SlotBuffers;

/// Всё, что нужно вызову рисования пакета, кроме самого пакета.
#[derive(Clone, Copy)]
struct BatchContext<'a> {
    slot: usize,
    quarter_turns: u32,
    target: &'a ColorAttachment,
    frame: &'a UiFrame<'a>,
    plan: &'a FramePlan,
}

/// Пирамида и то, что известно о её содержимом.
struct LivePyramid {
    pyramid: Pyramid,
    /// Ключ содержимого; `None` — содержимого нет (только создана или кадр упал на полпути).
    key: Option<PyramidKey>,
    /// Рисовали ли в ступени хоть раз (определяет раскладку «до» при следующей подготовке).
    rendered_once: bool,
}

/// Прежний набор ступеней, который ещё могут читать кадры в полёте: живёт, пока их слоты не освобождены.
struct RetiredPyramid {
    _pyramid: Pyramid,
    waiting_slots: Vec<usize>,
}

/// Рендер UI.
pub struct UiRenderer {
    context: GpuContext,
    config: UiRendererConfig,
    pipelines: UiPipelines,
    /// Какую часть квада стеклянной группы рисовать (боевой — `Tight`).
    glass_area: GlassDrawArea,
    /// Атлас SDF: живёт, пока живы наборы дескрипторов, ссылающиеся на него.
    _atlas: SampledTexture,
    slot_buffers: Vec<SlotBuffers>,
    slot_in_flight: Vec<bool>,
    pyramid: Option<LivePyramid>,
    retired: Vec<RetiredPyramid>,
    /// Текстуры картинок: загрузка в команды кадра, вытеснение, отложенное освобождение.
    pictures: PictureCache,
    counters: UiRenderCounters,
}

impl std::fmt::Debug for UiRenderer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "UiRenderer({} слотов, {:?})",
            self.config.frame_slots, self.counters
        )
    }
}

impl UiRenderer {
    /// Создаёт рендер: проверяет настройки, строит конвейеры, загружает атлас (блокирующая разовая отправка —
    /// путь инициализации, не кадра) и привязывает буферы слотов.
    pub fn new(context: &GpuContext, config: &UiRendererConfig) -> Result<Self, UiRenderError> {
        Self::with_shader_set(context, config, UiShaderSet::Product)
    }

    /// То же, что [`UiRenderer::new`], но с выбором набора шейдеров (боевой или замороженный эталон тестов).
    pub(crate) fn with_shader_set(
        context: &GpuContext,
        config: &UiRendererConfig,
        shader_set: UiShaderSet,
    ) -> Result<Self, UiRenderError> {
        Self::with_shader_set_and_glass_area(context, config, shader_set, GlassDrawArea::Tight)
    }

    /// Полный конструктор: набор шейдеров и часть квада стекла (второе нужно только тестам).
    pub(crate) fn with_shader_set_and_glass_area(
        context: &GpuContext,
        config: &UiRendererConfig,
        shader_set: UiShaderSet,
        glass_area: GlassDrawArea,
    ) -> Result<Self, UiRenderError> {
        config.validate()?;
        let mut pipelines = UiPipelines::new(context, config, shader_set)?;
        let atlas_data = sdf_atlas();
        let atlas = SampledTexture::upload(
            context,
            &TextureDesc {
                extent: vk::Extent2D {
                    width: atlas_data.width,
                    height: atlas_data.height,
                },
                format: TextureFormat::R8Unorm,
                label: "ui-sdf-atlas",
            },
            atlas_data.texels,
        )?;
        let pictures = PictureCache::new(context, config.pictures)?;
        let mut slot_buffers = Vec::with_capacity(config.frame_slots);
        for slot in 0..config.frame_slots {
            let buffers = SlotBuffers::new(context, &config.limits)?;
            // Буферы слота постоянны: привязываем один раз, дальше повторная запись тех же ресурсов — пустая операция.
            pipelines.shape.bind(
                slot,
                &[BindingWrite {
                    binding: 0,
                    resource: buffers.shapes.binding(),
                }],
            )?;
            pipelines.glyph.bind(
                slot,
                &[
                    BindingWrite {
                        binding: 0,
                        resource: buffers.glyphs.binding(),
                    },
                    BindingWrite {
                        binding: 1,
                        resource: atlas.read_only_binding(),
                    },
                ],
            )?;
            // Картинок ещё нет: все сэмплеры смотрят в заглушку, чтобы набор был валиден. Реальные текстуры кадра
            // записываются перед рисованием (`upload_and_bind_slot`).
            let mut image_writes = vec![BindingWrite {
                binding: 0,
                resource: buffers.images.binding(),
            }];
            image_writes.extend(sampler_writes(&pictures.bindings_for(&[])));
            pipelines.image.bind(slot, &image_writes)?;
            slot_buffers.push(buffers);
        }
        Ok(Self {
            context: context.clone(),
            config: *config,
            pipelines,
            glass_area,
            _atlas: atlas,
            slot_buffers,
            slot_in_flight: vec![false; config.frame_slots],
            pyramid: None,
            retired: Vec::new(),
            pictures,
            counters: UiRenderCounters::default(),
        })
    }

    /// Счётчики диагностики.
    pub fn counters(&self) -> UiRenderCounters {
        self.counters
    }

    /// Сколько картинок сейчас лежит в видеопамяти (диагностика и тесты вытеснения).
    pub fn cached_picture_count(&self) -> usize {
        self.pictures.cached_pictures()
    }

    /// Настройки, с которыми создан рендер.
    pub fn config(&self) -> &UiRendererConfig {
        &self.config
    }

    /// Вызывающий подтверждает: fence сабмита, в который записан кадр слота, сигнален — буферы и наборы слота
    /// снова можно переписывать. Слот вне диапазона — `SlotOutOfRange`.
    pub fn release_slot_after_fence(&mut self, slot: usize) -> Result<(), UiRenderError> {
        self.check_slot_range(slot)?;
        let level_stages = self.pipelines.level_stages;
        self.pipelines.shape.release_slot_after_fence(slot)?;
        self.pipelines.glyph.release_slot_after_fence(slot)?;
        self.pipelines.glass.release_slot_after_fence(slot)?;
        self.pipelines.image.release_slot_after_fence(slot)?;
        self.pipelines.blur_from_backdrop.release_slot_after_fence(slot)?;
        for stage in 0..level_stages {
            self.pipelines
                .blur_from_level
                .release_slot_after_fence(slot * level_stages + stage)?;
        }
        self.slot_in_flight[slot] = false;
        // Staging копий и вытесненные текстуры, ждавшие этот слот, больше GPU не нужны.
        self.pictures.release_slot_after_fence(slot);
        // Прежние ступени больше не нужны, когда освобождены все слоты, что были в полёте при их выводе из работы.
        for retired in &mut self.retired {
            retired.waiting_slots.retain(|waiting| *waiting != slot);
        }
        self.retired.retain(|retired| !retired.waiting_slots.is_empty());
        Ok(())
    }

    /// Успешно записанный кадр слота `slot` НЕ отправлен (сбой сабмита, смена поколения цели): буфер кадра
    /// пошёл на выброс, и всё, что он должен был сделать на GPU, не сделано. Поэтому:
    /// * картинки, загруженные этим кадром, выбрасываются из кэша — иначе следующий кадр рисовал бы текстуру,
    ///   в которую копия так и не легла (счётчик `pictures_discarded_unsubmitted`);
    /// * содержимому пирамиды, перестроенной этим кадром, верить нельзя — как после ошибки `Gpu`.
    ///
    /// Сам слот освобождает обычный `release_slot_after_fence`, когда кольцо выдаст его снова.
    pub fn frame_not_submitted(&mut self, slot: usize) -> Result<(), UiRenderError> {
        self.check_slot_range(slot)?;
        let others: Vec<usize> = self
            .slots_in_flight()
            .into_iter()
            .filter(|in_flight| *in_flight != slot)
            .collect();
        let discarded = self.pictures.frame_not_submitted(slot, &others);
        self.counters.pictures_discarded_unsubmitted += u64::from(discarded);
        if let Some(live) = self.pyramid.as_mut() {
            live.key = None;
            live.rendered_once = false;
        }
        Ok(())
    }

    /// Записывает кадр в `command_buffer`: пирамида (если нужна), барьер и проход UI в `target`.
    /// Эквивалент [`Self::record_frame_marked`] без меток.
    pub fn record_frame(
        &mut self,
        command_buffer: vk::CommandBuffer,
        slot: usize,
        target: ColorAttachment,
        frame: &UiFrame<'_>,
    ) -> Result<UiFrameReport, UiRenderError> {
        self.record_frame_marked(command_buffer, slot, target, frame, &mut |_, _| {})
    }

    /// То же с метками: `mark` вызывается в точках [`FrameMarker`] вне динамического рендеринга
    /// (вызывающий пишет в них метки времени GPU).
    ///
    /// Вызывается ПОСЛЕ того, как предыдущий проход (композитор) закрыл рендеринг в `target`. Ошибки до записи
    /// команд (все, кроме `Gpu`) безопасны: буфер и цель не тронуты. После `Gpu` буфер годится только на выброс.
    pub fn record_frame_marked(
        &mut self,
        command_buffer: vk::CommandBuffer,
        slot: usize,
        target: ColorAttachment,
        frame: &UiFrame<'_>,
        mark: &mut dyn FnMut(FrameMarker, vk::CommandBuffer),
    ) -> Result<UiFrameReport, UiRenderError> {
        match self.record_frame_inner(command_buffer, slot, target, frame, mark) {
            Ok(report) => {
                self.slot_in_flight[slot] = true;
                let evicted = self.pictures.finish_frame(slot, self.slots_in_flight());
                self.counters.picture_evictions += u64::from(evicted);
                self.counters.frames_recorded += 1;
                self.counters.images_drawn += u64::from(report.image_instances);
                self.counters.live_images_drawn += u64::from(report.live_images_drawn);
                self.counters.live_images_missing += u64::from(report.live_images_missing);
                self.counters.picture_uploads += u64::from(report.picture_uploads);
                self.counters.picture_cache_hits += u64::from(report.picture_cache_hits);
                self.counters.draw_calls += u64::from(report.draw_calls);
                self.counters.shape_instances += u64::from(report.shape_instances);
                self.counters.glyph_instances += u64::from(report.glyph_instances);
                self.counters.glass_groups += u64::from(report.glass_groups);
                match report.pyramid {
                    PyramidOutcome::Rebuilt => self.counters.pyramid_rebuilds += 1,
                    PyramidOutcome::Reused => self.counters.pyramid_reuses += 1,
                    PyramidOutcome::NotNeeded => {}
                }
                Ok(report)
            }
            Err(error) => {
                self.counters.frames_failed += 1;
                if matches!(error, UiRenderError::LimitExceeded { .. }) {
                    self.counters.limit_overflows += 1;
                }
                if matches!(error, UiRenderError::PictureTooLarge { .. }) {
                    self.counters.pictures_rejected += 1;
                }
                if matches!(error, UiRenderError::Gpu(_)) {
                    // Ошибка GPU случается ПОСЛЕ начала записи команд, а вызывающий может отправить
                    // частично записанный буфер. Поэтому:
                    // * содержимому и раскладке ступеней пирамиды верить нельзя — следующая перестройка
                    //   начинается с «не использовалась» (переход из UNDEFINED допустим из любой
                    //   раскладки, содержимое всё равно пересчитывается);
                    // * слот мог попасть в отправленный буфер (привязки, буферы экземпляров) — он
                    //   «в полёте», пока владелец не подтвердит fence (`release_slot_after_fence`).
                    if let Some(live) = self.pyramid.as_mut() {
                        live.key = None;
                        live.rendered_once = false;
                    }
                    self.slot_in_flight[slot] = true;
                }
                // Копии картинок, записанные в этот кадр, могли не выполниться: текстуры выбрасываются из кэша.
                // До записи команд (все ошибки кроме `Gpu`) кэш не тронут — вызов ничего не меняет.
                let waiting = self.slots_in_flight();
                self.pictures.abort_frame(&waiting);
                Err(error)
            }
        }
    }

    /// Слоты, чьи буферы и наборы GPU ещё может читать.
    fn slots_in_flight(&self) -> Vec<usize> {
        (0..self.config.frame_slots)
            .filter(|index| self.slot_in_flight[*index])
            .collect()
    }

    fn check_slot_range(&self, slot: usize) -> Result<(), UiRenderError> {
        if slot < self.config.frame_slots {
            Ok(())
        } else {
            Err(UiRenderError::SlotOutOfRange {
                slot,
                slots: self.config.frame_slots,
            })
        }
    }

    /// Все проверки входа; возвращает логический размер в целых px.
    fn check_frame(
        &self,
        slot: usize,
        target: &ColorAttachment,
        frame: &UiFrame<'_>,
    ) -> Result<(u32, u32), UiRenderError> {
        self.check_slot_range(slot)?;
        if self.slot_in_flight[slot] {
            return Err(UiRenderError::SlotInFlight { slot });
        }
        if target.format() != self.config.target_format {
            return Err(UiRenderError::TargetFormatMismatch {
                expected: self.config.target_format,
                actual: target.format(),
            });
        }
        let PxSize { width, height } = frame.logical_size;
        if !(width.is_finite() && height.is_finite() && width >= 1.0 && height >= 1.0) {
            return Err(UiRenderError::InvalidLogicalSize { width, height });
        }
        let logical = (width.round() as u32, height.round() as u32);
        frame
            .surface_transform
            .quarter_turns()
            .map_err(UiRenderError::UnsupportedSurfaceTransform)?;
        let expected = if frame.surface_transform.swaps_extent() {
            vk::Extent2D {
                width: logical.1,
                height: logical.0,
            }
        } else {
            vk::Extent2D {
                width: logical.0,
                height: logical.1,
            }
        };
        if target.extent() != expected {
            return Err(UiRenderError::TargetSizeMismatch {
                expected,
                actual: target.extent(),
            });
        }
        Ok(logical)
    }

    fn record_frame_inner(
        &mut self,
        command_buffer: vk::CommandBuffer,
        slot: usize,
        target: ColorAttachment,
        frame: &UiFrame<'_>,
        mark: &mut dyn FnMut(FrameMarker, vk::CommandBuffer),
    ) -> Result<UiFrameReport, UiRenderError> {
        let logical = self.check_frame(slot, &target, frame)?;
        let plan = build_plan(frame.draw_list, &self.config.limits, frame.live_images)?;
        // Память под новые картинки и их staging — до первой команды (ошибка здесь не оставляет следов в буфере).
        let (staged_uploads, picture_cache_hits) = self.pictures.stage_missing(&self.context, plan.pictures())?;
        let quarter_turns = frame
            .surface_transform
            .quarter_turns()
            .map_err(UiRenderError::UnsupportedSurfaceTransform)?;

        mark(FrameMarker::Start, command_buffer);
        // Копии новых картинок — в команды кадра, до всех проходов (внутри динамического рендеринга копии запрещены).
        let picture_uploads = self.pictures.record_staged_uploads(command_buffer, staged_uploads)?;
        let outcome = if plan.has_glass() {
            self.ensure_pyramid_current(command_buffer, slot, logical, frame)?
        } else {
            PyramidOutcome::NotNeeded
        };
        self.upload_and_bind_slot(slot, &plan, frame)?;
        mark(FrameMarker::PyramidDone, command_buffer);

        let draw_calls = if plan.batches.is_empty() {
            0
        } else {
            record_between_color_passes(&self.context, command_buffer);
            let mut pass = RasterPass::begin(&self.context, command_buffer, target, AttachmentLoad::Preserve)?;
            let mut calls = 0_u32;
            for batch in &plan.batches {
                let context = BatchContext {
                    slot,
                    quarter_turns,
                    target: &target,
                    frame,
                    plan: &plan,
                };
                self.draw_batch(&mut pass, batch, &context)?;
                calls += 1;
            }
            pass.end();
            calls
        };
        mark(FrameMarker::UiPassDone, command_buffer);
        Ok(UiFrameReport {
            pyramid: outcome,
            draw_calls,
            shape_instances: plan.shape_count,
            glyph_instances: plan.glyph_count,
            glass_groups: plan.group_count,
            image_instances: plan.image_count,
            live_images_drawn: plan.live_images_drawn,
            live_images_missing: plan.live_images_missing,
            picture_uploads,
            picture_cache_hits,
        })
    }

    /// Ступени нужного размера и содержимого: создаёт/пересоздаёт, перестраивает при смене ключа.
    fn ensure_pyramid_current(
        &mut self,
        command_buffer: vk::CommandBuffer,
        slot: usize,
        logical: (u32, u32),
        frame: &UiFrame<'_>,
    ) -> Result<PyramidOutcome, UiRenderError> {
        let size_changed = self.pyramid.as_ref().is_none_or(|live| live.pyramid.logical != logical);
        if size_changed {
            let created = Pyramid::new(&self.context, logical, &self.config.pyramid)?;
            if let Some(old) = self.pyramid.take() {
                // Кадры в полёте ещё могут читать прежние ступени: держим их, пока эти слоты не освободятся.
                let waiting_slots: Vec<usize> = (0..self.config.frame_slots)
                    .filter(|index| self.slot_in_flight[*index])
                    .collect();
                if !waiting_slots.is_empty() {
                    self.retired.push(RetiredPyramid {
                        _pyramid: old.pyramid,
                        waiting_slots,
                    });
                }
                self.counters.pyramid_recreations += 1;
            }
            self.pyramid = Some(LivePyramid {
                pyramid: created,
                key: None,
                rendered_once: false,
            });
        }
        let (key, source) = match frame.backdrop {
            BackdropInput::Live { source, placement } => (
                PyramidKey::live(source, &placement.destination),
                RebuildSource::Live {
                    source,
                    destination: placement.destination,
                },
            ),
            BackdropInput::Absent { fill } => (PyramidKey::absent(fill), RebuildSource::Absent { fill }),
        };
        let Some(live) = self.pyramid.as_mut() else {
            return Err(UiRenderError::Gpu(GpuError::InvalidState {
                what: "пирамида не создана".to_owned(),
            }));
        };
        if live.key == Some(key) {
            return Ok(PyramidOutcome::Reused);
        }
        let previous_use = if live.rendered_once {
            ColorTargetPreviousUse::SampledRead(
                gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage::Fragment,
            )
        } else {
            ColorTargetPreviousUse::NotUsedYet
        };
        live.pyramid.record_rebuild(
            &self.context,
            command_buffer,
            &mut self.pipelines,
            slot,
            source,
            previous_use,
        )?;
        live.rendered_once = true;
        live.key = Some(key);
        Ok(PyramidOutcome::Rebuilt)
    }

    /// Загружает экземпляры в буферы слота и (для стекла) записывает привязки фона и уровней.
    fn upload_and_bind_slot(
        &mut self,
        slot: usize,
        plan: &FramePlan,
        frame: &UiFrame<'_>,
    ) -> Result<(), UiRenderError> {
        let buffers = &mut self.slot_buffers[slot];
        for (buffer, bytes) in [
            (&mut buffers.shapes, &plan.shape_bytes),
            (&mut buffers.glyphs, &plan.glyph_bytes),
            (&mut buffers.groups, &plan.group_bytes),
            (&mut buffers.members, &plan.member_bytes),
            (&mut buffers.images, &plan.image_bytes),
        ] {
            if !bytes.is_empty() {
                buffer.write_bytes(0, bytes)?;
            }
        }
        if plan.image_count > 0 {
            let mut image_writes = vec![BindingWrite {
                binding: 0,
                resource: self.slot_buffers[slot].images.binding(),
            }];
            image_writes.extend(sampler_writes(&self.pictures.bindings_for(&plan.sampler_slots)));
            self.pipelines.image.bind(slot, &image_writes)?;
        }
        if !plan.has_glass() {
            return Ok(());
        }
        let Some(live) = self.pyramid.as_ref() else {
            return Err(UiRenderError::Gpu(GpuError::InvalidState {
                what: "пирамида не создана".to_owned(),
            }));
        };
        let backdrop_resource = match frame.backdrop {
            BackdropInput::Live { source, .. } => DescriptorResource::ReadOnlyImage {
                view: source.view().view,
            },
            // Absent: фон не читается (флаг в push-константах), но привязка обязана быть валидной.
            BackdropInput::Absent { .. } => live.pyramid.level_binding(1),
        };
        let buffers = &self.slot_buffers[slot];
        let mut writes = vec![
            BindingWrite {
                binding: 0,
                resource: buffers.groups.binding(),
            },
            BindingWrite {
                binding: 1,
                resource: buffers.members.binding(),
            },
            BindingWrite {
                binding: 2,
                resource: backdrop_resource,
            },
        ];
        for level in 1..=MAX_PYRAMID_LEVELS {
            writes.push(BindingWrite {
                binding: GLASS_FIRST_PYRAMID_BINDING + level - 1,
                resource: live.pyramid.level_binding(level as usize),
            });
        }
        self.pipelines.glass.bind(slot, &writes)?;
        Ok(())
    }

    fn frame_push(
        &self,
        quarter_turns: u32,
        target: &ColorAttachment,
        frame: &UiFrame<'_>,
        first_instance: u32,
    ) -> FramePush {
        let (placement, live, fill, backdrop_window) = match frame.backdrop {
            BackdropInput::Live { placement, source } => {
                let d = placement.destination;
                (
                    [d.origin.x, d.origin.y, d.size.width, d.size.height],
                    true,
                    [0.0; 4],
                    source.window().mapping(),
                )
            }
            BackdropInput::Absent { fill } => (
                [0.0; 4],
                false,
                [fill.r, fill.g, fill.b, fill.a],
                SourceWindow::of_content(vk::Extent2D { width: 1, height: 1 }).mapping(),
            ),
        };
        let light = frame.draw_list.light.as_vector();
        FramePush {
            logical_size: [frame.logical_size.width, frame.logical_size.height],
            physical_size: [target.extent().width as f32, target.extent().height as f32],
            placement,
            light: [light.dx, light.dy],
            pyramid_levels: self.config.pyramid.levels,
            backdrop_live: live,
            absent_fill: fill,
            atlas_spread_texels: sdf_atlas().spread_texels,
            quarter_turns,
            first_instance,
            backdrop_window,
        }
    }

    /// Стекло: каждая группа — отдельный вызов рисования со scissor по области её возможного вклада.
    /// Вершины квада остаются прежними (координаты фона, интерполированные по нему, не меняются), поэтому
    /// картинка бит-в-бит та же, а фрагменты вне области до шейдера не доходят. Группа, чья область целиком
    /// вне цели, не рисуется вовсе: вклад ровно нулевой.
    fn draw_glass_groups_tight(
        &self,
        pass: &mut RasterPass<'_>,
        batch: &Batch,
        context: &BatchContext<'_>,
    ) -> Result<(), UiRenderError> {
        let BatchContext {
            slot,
            quarter_turns,
            target,
            frame,
            plan,
        } = *context;
        for group_number in batch.first_instance..batch.first_instance + batch.instance_count {
            let Some(contribution_area) = plan.glass_scissors.get(group_number as usize) else {
                return Err(UiRenderError::Gpu(GpuError::InvalidState {
                    what: format!("у стеклянной группы {group_number} нет области вклада: план кадра повреждён"),
                }));
            };
            let Some(scissor) = logical_rect_to_scissor(contribution_area, quarter_turns, target.extent()) else {
                continue;
            };
            let push = self.frame_push(quarter_turns, target, frame, group_number).to_bytes();
            pass.draw(
                &self.pipelines.glass,
                slot,
                &push,
                DrawCall {
                    vertices_per_instance: 6,
                    instance_count: 1,
                    scissor: Some(scissor),
                },
            )?;
        }
        Ok(())
    }

    fn draw_batch(
        &self,
        pass: &mut RasterPass<'_>,
        batch: &Batch,
        context: &BatchContext<'_>,
    ) -> Result<(), UiRenderError> {
        if batch.kind == BatchKind::Glass && self.glass_area == GlassDrawArea::Tight {
            return self.draw_glass_groups_tight(pass, batch, context);
        }
        let BatchContext {
            slot,
            quarter_turns,
            target,
            frame,
            ..
        } = *context;
        let pipeline = match batch.kind {
            BatchKind::Shape => &self.pipelines.shape,
            BatchKind::Glyph => &self.pipelines.glyph,
            BatchKind::Glass => &self.pipelines.glass,
            BatchKind::Image => &self.pipelines.image,
        };
        let push = self
            .frame_push(quarter_turns, target, frame, batch.first_instance)
            .to_bytes();
        pass.draw(
            pipeline,
            slot,
            &push,
            DrawCall {
                vertices_per_instance: 6,
                instance_count: batch.instance_count,
                scissor: None,
            },
        )?;
        Ok(())
    }
}

/// Записи привязок сэмплеров картинок: ресурсы по порядку начиная с `IMAGE_FIRST_SAMPLER_BINDING`.
fn sampler_writes(resources: &[DescriptorResource]) -> Vec<BindingWrite> {
    debug_assert_eq!(resources.len(), MAX_IMAGE_BINDINGS as usize);
    (IMAGE_FIRST_SAMPLER_BINDING..)
        .zip(resources)
        .map(|(binding, resource)| BindingWrite {
            binding,
            resource: *resource,
        })
        .collect()
}
