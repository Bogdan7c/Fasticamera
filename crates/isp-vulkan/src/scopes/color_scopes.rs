//! [`ColorScopes`] — владелец цветовых приборов на GPU: пайплайны всех приборов, буфер счётчиков, параметры по
//! слотам кадров, картинка прибора и метки времени (решения 183, 188–190).
//!
//! # Жизненный цикл
//! Создаётся ОДИН раз, когда приборы открываются (render-loop создаёт между кадрами и бросает, когда закрываются:
//! закрытые приборы ничего не стоят). Картинку под размер запроса можно подготовить заранее
//! ([`ColorScopes::prepare_picture`]), чтобы запись кадра объектов GPU не создавала. Пайплайны ВСЕХ приборов строятся в [`ColorScopes::new`]; смена прибора,
//! осей вектороскопа и размера картинки позже пайплайнов не создаёт — меняется только выбор шейдера и данные.
//!
//! # Кадр ([`ColorScopes::record`])
//! 1. Все проверки (слот, настройки, вход, прибор, вместимость буфера, сборка параметров, лимиты dispatch, привязки)
//!    — ДО первой команды И ДО смены состояния: при ошибке командный буфер не тронут, действующая картинка остаётся на
//!    месте, счётчики диагностики не растут (новая картинка принимается только после всех проверок).
//! 2. Метки времени → обнуление счётчиков (`vkCmdFillBuffer`, ждёт чтений прошлого кадра) → накопление
//!    (`atomicAdd`) → барьер compute→compute → барьер картинки «прошлое чтение → запись» → показ →
//!    барьер «запись compute → чтение фрагментным шейдером».
//! 3. Картинка прибора после записи готова к чтению фрагментным шейдером.
//!
//! # Контракт вызывающего (поток рендера)
//! * `record` для слота — только когда fence прошлого сабмита этого слота сигнален (переписываются буферы
//!   параметров и наборы дескрипторов слота);
//! * после того, как fence слота сигнален, вызывающий ОБЯЗАТЕЛЬНО зовёт [`ColorScopes::release_slot_after_fence`]
//!   до следующего `record` слота: набор дескрипторов, привязанный в командный буфер, нельзя менять, пока
//!   владелец не подтвердит fence (вход и картинка приборов меняются от кадра к кадру); заодно уничтожаются
//!   картинки, заменённые сменой размера;
//! * команды отправляются в той же очереди и в том же порядке, в каком записаны (счётчики и картинка помнят
//!   своё состояние по порядку записи);
//! * перед `Drop` GPU не исполняет команд с объектами приборов.

use ash::vk;
use gpu_vulkan::compute::{
    BindingWrite, ComputePipeline, ComputePipelineDesc, DescriptorResource, LocalSize, WorkgroupCount,
};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::device_buffer::{DeviceBuffer, record_compute_pass_dependency};
use gpu_vulkan::gpu_timing::{GpuTimestamps, TimestampStage};
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};
use gpu_vulkan::source_window::HorizontalMirror;
use scope_contract::ScopeImageSize;
use scope_model::codes::{MAX_SHOWN_PIXELS, MAX_SHOWN_SIDE};
use scope_model::grid::{MAX_GRID_CELLS, MAX_GRID_SIDE};
use scope_model::vectorscope_geometry::MAX_VECTORSCOPE_SIZE;
use scope_model::{GraticulePrimitive, GridShape, MAX_GRATICULE_PRIMITIVES, ScopeModelSettings};

use super::accumulators::{
    ACCUMULATE_BINDINGS, AccumulatorSet, AccumulatorShader, SAMPLING_PUSH_BYTES, local_size, local_size_specialization,
    standard_accumulators,
};
use super::error::ScopeError;
use super::frame::{RecordedScope, ScopeFrame, ScopeInput, ScopePassCounters, ScopeSlotReport};
use super::parameter_blocks::{
    ACCUMULATE_BLOCK_BYTES, ACCUMULATE_WORDS_BYTES, MAX_PLANES, RENDER_BLOCK_BYTES, accumulate_words_bytes,
    decode_table_bytes, render_block_bytes,
};
use super::picture::{PictureChange, PictureTarget};
use super::rule::AccumulatorId;
use crate::shaders::blobs;
use crate::viewfinder::MAX_FRAME_SLOTS;
use crate::viewfinder::retirement::{RecordSerial, RetirementQueue};

/// Имена точек меток времени.
const TIMESTAMP_POINTS: [&str; 3] = ["scopes-start", "accumulate-end", "render-end"];
const POINT_START: usize = 0;
const POINT_ACCUMULATE_END: usize = 1;
const POINT_RENDER_END: usize = 2;

/// Привязки шейдера показа: картинка прибора, параметры показа, счётчики (только чтение).
const RENDER_BINDINGS: [gpu_vulkan::compute::BindingKind; 3] = [
    gpu_vulkan::compute::BindingKind::StorageImageWrite,
    gpu_vulkan::compute::BindingKind::ReadOnlyStorageBuffer,
    gpu_vulkan::compute::BindingKind::ReadOnlyStorageBuffer,
];

/// Конфигурация [`ColorScopes`].
#[derive(Debug, Clone, PartialEq)]
pub struct ColorScopesConfig {
    /// Число слотов кадров (кадров в полёте).
    pub frame_slots: usize,
    /// Настройки, под наибольшую сетку которых рассчитан буфер счётчиков. Позже в `record` можно подавать
    /// любые настройки, чьи сетки не больше (меньшие размеры без пересоздания).
    pub counter_sizing: ScopeModelSettings,
}

impl Default for ColorScopesConfig {
    fn default() -> Self {
        Self {
            frame_slots: 2,
            counter_sizing: ScopeModelSettings::default(),
        }
    }
}

/// Цветовые приборы на GPU (см. описание модуля).
pub struct ColorScopes {
    accumulators: AccumulatorSet,
    render_pipeline: ComputePipeline,
    counters: DeviceBuffer,
    counter_capacity_cells: usize,
    accumulate_parameters: Vec<HostBuffer>,
    render_parameters: Vec<HostBuffer>,
    picture: Option<PictureTarget>,
    retired_pictures: RetirementQueue<PictureTarget>,
    timestamps: GpuTimestamps,
    /// Номер последней записи каждого слота (`None` — слот не записывался).
    slot_records: Vec<Option<RecordSerial>>,
    next_serial: RecordSerial,
    pass_counters: ScopePassCounters,
    /// Форма сетки последней записи (для чтения счётчиков в тестах).
    last_grid: Option<GridShape>,
    local_size: LocalSize,
    context: GpuContext,
}

impl std::fmt::Debug for ColorScopes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ColorScopes")
            .field("accumulator_pipelines", &self.accumulators.len())
            .field("counter_capacity_cells", &self.counter_capacity_cells)
            .field("picture", &self.picture)
            .field("pass_counters", &self.pass_counters)
            .finish()
    }
}

/// Проверенный вход: размер содержимого и зеркало.
struct CheckedInput {
    content: vk::Extent2D,
    mirrored: bool,
}

/// Проверенный и упакованный прибор кадра: всё, что пишется в буферы и привязки.
struct RulePlan {
    accumulator: AccumulatorId,
    grid: GridShape,
    words_bytes: Vec<u8>,
    decode_table_bytes: Option<Vec<u8>>,
    render_bytes: Vec<u8>,
}

/// Итог проверок кадра: всё, что нужно записи команд. Получается ДО любой смены состояния владельца.
struct CheckedFrame {
    slot: usize,
    plan: RulePlan,
    push: [u8; SAMPLING_PUSH_BYTES as usize],
    accumulate_groups: WorkgroupCount,
    render_groups: WorkgroupCount,
    /// Картинка нужного размера, созданная проверками, если действующей нет или она другого размера. Принимается
    /// владельцем ([`ColorScopes::adopt_picture`]) только после того, как ВСЕ проверки пройдены.
    new_picture: Option<PictureTarget>,
}

impl ColorScopes {
    /// Создаёт приборы со стандартными шейдерами (`scope-model`).
    pub fn new(context: &GpuContext, config: &ColorScopesConfig) -> Result<Self, ScopeError> {
        Self::with_extra_accumulators(context, config, &[])
    }

    /// Создаёт приборы со стандартными шейдерами и дополнительными (новые приборы, подставные приборы тестов).
    /// Все пайплайны строятся здесь.
    pub fn with_extra_accumulators(
        context: &GpuContext,
        config: &ColorScopesConfig,
        extra: &[AccumulatorShader],
    ) -> Result<Self, ScopeError> {
        if config.frame_slots == 0 || config.frame_slots > MAX_FRAME_SLOTS {
            return Err(ScopeError::InvalidConfig {
                what: format!("frame_slots {} вне 1..={MAX_FRAME_SLOTS}", config.frame_slots),
            });
        }
        config.counter_sizing.validate()?;
        let local_size = local_size()?;
        // Размер группы — против лимитов ЭТОГО устройства.
        local_size.validate_for_device(context)?;
        let mut shaders = standard_accumulators().to_vec();
        shaders.extend_from_slice(extra);
        let accumulators = AccumulatorSet::new(context, &shaders, local_size, config.frame_slots)?;
        let render_pipeline = ComputePipeline::new(
            context,
            &ComputePipelineDesc {
                label: "scope-render",
                spirv: blobs::SCOPE_RENDER_COMP,
                bindings: &RENDER_BINDINGS,
                push_constant_bytes: 0,
                specialization: &local_size_specialization(local_size),
                frame_slots: config.frame_slots,
            },
        )?;
        let counter_capacity_cells = largest_grid_cells(&config.counter_sizing);
        let counters = DeviceBuffer::new(context, counter_capacity_cells as u64 * 4)?;
        let make_buffers = |bytes: usize| {
            (0..config.frame_slots)
                .map(|_| HostBuffer::new(context, bytes as u64, HostBufferUsage::ReadOnlyStorage))
                .collect::<Result<Vec<_>, _>>()
        };
        Ok(Self {
            accumulators,
            render_pipeline,
            counters,
            counter_capacity_cells,
            accumulate_parameters: make_buffers(ACCUMULATE_BLOCK_BYTES)?,
            render_parameters: make_buffers(RENDER_BLOCK_BYTES)?,
            picture: None,
            retired_pictures: RetirementQueue::default(),
            timestamps: GpuTimestamps::new(context, config.frame_slots, &TIMESTAMP_POINTS)?,
            slot_records: vec![None; config.frame_slots],
            next_serial: RecordSerial(0),
            pass_counters: ScopePassCounters::default(),
            last_grid: None,
            local_size,
            context: context.clone(),
        })
    }

    /// Число слотов кадров.
    pub fn frame_slots(&self) -> usize {
        self.slot_records.len()
    }

    /// Сколько пайплайнов накопления создано (все — при создании; позже число не меняется).
    pub fn accumulator_pipeline_count(&self) -> usize {
        self.accumulators.len()
    }

    /// На сколько ячеек рассчитан буфер счётчиков.
    pub fn counter_capacity_cells(&self) -> usize {
        self.counter_capacity_cells
    }

    /// Счётчики диагностики.
    pub fn pass_counters(&self) -> ScopePassCounters {
        self.pass_counters
    }

    /// Картинка прибора последней записи (`None` — кадров ещё не было).
    pub fn output_view(&self) -> Option<gpu_vulkan::storage_image::SampledImageView> {
        self.picture.as_ref().map(PictureTarget::sampled_view)
    }

    /// Сколько картинок ждут завершения GPU перед уничтожением.
    pub fn retired_pictures(&self) -> usize {
        self.retired_pictures.len()
    }

    /// Готовит картинку прибора размера `size` ЗАРАНЕЕ (между кадрами), чтобы первая запись не создавала объектов
    /// GPU внутри кадра. Размер тот же — ничего не делает. Прежняя картинка другого размера уходит в отложенное
    /// уничтожение и остаётся читаемой для кадров, которые её уже взяли; содержимое новой неопределено, пока
    /// [`Self::record`] её не запишет (вызывающий не отдаёт её читателям раньше).
    pub fn prepare_picture(&mut self, size: ScopeImageSize) -> Result<PictureChange, ScopeError> {
        let new_picture = self.picture_candidate(size)?;
        Ok(self.adopt_picture(new_picture, self.next_serial))
    }

    /// Есть ли готовая картинка именно этого размера (запись её не создаёт).
    pub fn is_picture_ready_for(&self, size: ScopeImageSize) -> bool {
        self.picture.as_ref().is_some_and(|picture| picture.size() == size)
    }

    /// Записывает кадр приборов в `command_buffer` (вне динамического рендеринга). См. описание модуля и
    /// контракт [`ScopeInput`].
    pub fn record(
        &mut self,
        command_buffer: vk::CommandBuffer,
        frame: &ScopeFrame<'_>,
    ) -> Result<RecordedScope, ScopeError> {
        // --- Проверки: до первой команды и до любой смены состояния картинки и счётчиков ---
        let mut checked = self.check_frame(frame)?;
        // --- Все проверки пройдены: новая картинка (если нужна) становится действующей ---
        let serial = self.next_serial;
        self.adopt_picture(checked.new_picture.take(), serial);
        let CheckedFrame {
            slot,
            plan,
            push,
            accumulate_groups,
            render_groups,
            ..
        } = checked;
        let accumulate_enabled = !accumulate_groups.is_empty();
        let picture = self.picture.as_ref().ok_or_else(|| ScopeError::InvalidConfig {
            what: "картинка прибора не создана (внутренняя ошибка порядка вызовов)".to_owned(),
        })?;
        let counter_bytes = plan.grid.cell_count() as u64 * 4;

        // --- Команды: дальше ошибок проверки быть не может ---
        self.timestamps.record_reset(command_buffer, slot)?;
        self.timestamps
            .record_point(command_buffer, slot, POINT_START, TimestampStage::AllCommandsDone)?;
        self.counters.record_clear_for_compute(command_buffer, counter_bytes)?;
        if accumulate_enabled {
            self.accumulators
                .get(plan.accumulator)?
                .record_dispatch(command_buffer, slot, &push, accumulate_groups)?;
        }
        self.timestamps.record_point(
            command_buffer,
            slot,
            POINT_ACCUMULATE_END,
            TimestampStage::ComputeShaderDone,
        )?;
        // Записи счётчиков накопления видны показу (и RAW, и WAW закрыты одной глобальной зависимостью).
        record_compute_pass_dependency(&self.context, command_buffer);
        // Картинку читали фрагментный шейдер UI и, возможно, другие стадии: ждём их все (WAR).
        picture
            .storage()
            .record_prepare_for_compute_write_after_any_reader(command_buffer, picture.current_use());
        self.render_pipeline
            .record_dispatch(command_buffer, slot, &[], render_groups)?;
        self.timestamps.record_point(
            command_buffer,
            slot,
            POINT_RENDER_END,
            TimestampStage::ComputeShaderDone,
        )?;
        picture.storage().record_compute_write_to_fragment_read(command_buffer);

        // --- Итоги записи: состояние меняется только после успешных команд ---
        let recorded = RecordedScope {
            output: picture.sampled_view(),
            size: picture.size(),
            picture_generation: picture.generation(),
            grid: plan.grid,
            accumulator: plan.accumulator,
        };
        if let Some(picture) = self.picture.as_mut() {
            picture.mark_published_to_fragment_reader();
        }
        self.slot_records[slot] = Some(serial);
        self.next_serial = RecordSerial(serial.0 + 1);
        self.last_grid = Some(plan.grid);
        self.pass_counters.recorded += 1;
        self.pass_counters.accumulate_dispatches += u64::from(accumulate_enabled);
        self.pass_counters.render_dispatches += 1;
        // Замену картинки (`picture_recreations`) считает `adopt_picture` в момент замены, а не здесь.
        Ok(recorded)
    }

    /// Все проверки кадра. Порядок: данные кадра → правило → форма dispatch → параметры слота → привязки. Привязка
    /// шейдера показа (она одна ссылается на НОВУЮ картинку) идёт ПОСЛЕДНЕЙ из отказывающих шагов: если любая
    /// проверка отказала, новая картинка нигде не записана (набор дескрипторов не хранит ссылку на объект, который
    /// тут же будет уничтожен), а действующая картинка и счётчики не тронуты.
    fn check_frame(&mut self, frame: &ScopeFrame<'_>) -> Result<CheckedFrame, ScopeError> {
        let slot = self.check_slot(frame.frame_slot)?;
        frame.settings.validate()?;
        let input = check_input(&frame.input)?;
        let plan = self.plan_rule(frame)?;
        let new_picture = self.picture_candidate(frame.picture_size)?;
        let stride = frame.settings.sample_stride;
        let samples_x = stride.samples_along(input.content.width);
        let samples_y = stride.samples_along(input.content.height);
        let accumulate_groups = WorkgroupCount::covering(samples_x, samples_y, self.local_size);
        let render_groups =
            WorkgroupCount::covering(frame.picture_size.width(), frame.picture_size.height(), self.local_size);
        let push = sampling_push_bytes(&input, stride.get(), stride.offset(), samples_x, samples_y);

        // Форма dispatch (лимиты устройства) не зависит от привязок: проверяем до любых записей в наборы.
        let accumulate_enabled = !accumulate_groups.is_empty();
        if accumulate_enabled {
            self.accumulators
                .get(plan.accumulator)?
                .check_dispatch_shape(SAMPLING_PUSH_BYTES as usize, accumulate_groups)?;
        }
        self.render_pipeline.check_dispatch_shape(0, render_groups)?;

        self.accumulate_parameters[slot].write_bytes(0, &plan.words_bytes)?;
        if let Some(table) = &plan.decode_table_bytes {
            self.accumulate_parameters[slot].write_bytes(ACCUMULATE_WORDS_BYTES as u64, table)?;
        }
        self.render_parameters[slot].write_bytes(0, &plan.render_bytes)?;

        let accumulate_writes = [
            BindingWrite {
                binding: 0,
                resource: DescriptorResource::ReadOnlyImage {
                    view: frame.input.view.view,
                },
            },
            BindingWrite {
                binding: 1,
                resource: self.accumulate_parameters[slot].binding(),
            },
            BindingWrite {
                binding: 2,
                resource: self.counters.read_write_binding(),
            },
        ];
        let accumulate_pipeline = self.accumulators.get_mut(plan.accumulator)?;
        accumulate_pipeline.bind(slot, &accumulate_writes)?;
        if accumulate_enabled {
            accumulate_pipeline.check_dispatch(slot, SAMPLING_PUSH_BYTES as usize, accumulate_groups)?;
        }
        // Картинка, которой пишет показ: новая (ещё не принятая) или действующая.
        let picture = match (&new_picture, &self.picture) {
            (Some(candidate), _) => candidate,
            (None, Some(current)) => current,
            (None, None) => {
                return Err(ScopeError::InvalidConfig {
                    what: "картинка прибора не создана (внутренняя ошибка порядка вызовов)".to_owned(),
                });
            }
        };
        self.render_pipeline.bind(
            slot,
            &[
                BindingWrite {
                    binding: 0,
                    resource: picture.storage().storage_binding(),
                },
                BindingWrite {
                    binding: 1,
                    resource: self.render_parameters[slot].binding(),
                },
                BindingWrite {
                    binding: 2,
                    resource: self.counters.read_only_binding(),
                },
            ],
        )?;
        // Готовность набора шейдера показа после успешной привязки повторяет `record_dispatch`; отказать здесь после
        // привязки новой картинки он не может, поэтому отдельной проверки нет.
        Ok(CheckedFrame {
            slot,
            plan,
            push,
            accumulate_groups,
            render_groups,
            new_picture,
        })
    }

    /// Вызывающий подтверждает: fence последнего сабмита слота сигнален. Освобождает наборы дескрипторов слота у
    /// ВСЕХ пайплайнов, уничтожает картинки, которые не нужны ни одной незавершённой записи, и забирает метки
    /// времени (никогда не ждёт GPU: метки, до которых он не дошёл, — `NotReady`).
    pub fn release_slot_after_fence(&mut self, frame_slot: usize) -> Result<ScopeSlotReport, ScopeError> {
        let slot = self.check_slot(frame_slot)?;
        if let Some(completed) = self.slot_records[slot] {
            let released = self.retired_pictures.release_completed(completed);
            if released > 0 {
                tracing::debug!(target: "fasticamera::isp_scopes", released, "заменённые картинки приборов уничтожены");
            }
        }
        self.accumulators.release_slot_after_fence(slot)?;
        self.render_pipeline.release_slot_after_fence(slot)?;
        Ok(ScopeSlotReport {
            gpu_time: self.timestamps.try_collect(slot)?,
        })
    }

    fn check_slot(&self, frame_slot: usize) -> Result<usize, ScopeError> {
        if frame_slot < self.slot_records.len() {
            Ok(frame_slot)
        } else {
            Err(ScopeError::FrameSlotOutOfRange {
                slot: frame_slot,
                slots: self.slot_records.len(),
            })
        }
    }

    /// Проверяет прибор кадра и упаковывает его параметры. Ничего не пишет на GPU.
    fn plan_rule(&self, frame: &ScopeFrame<'_>) -> Result<RulePlan, ScopeError> {
        let rule = frame.rule;
        let accumulator = rule.accumulator();
        // Незарегистрированный шейдер — отказ ДО всего остального.
        self.accumulators.get(accumulator)?;
        let grid = rule.grid();
        if grid.cell_count() > self.counter_capacity_cells {
            return Err(ScopeError::GridExceedsCounterCapacity {
                cells: grid.cell_count(),
                capacity: self.counter_capacity_cells,
            });
        }
        if grid.planes() as usize > MAX_PLANES {
            return Err(ScopeError::TooManyPlanes {
                planes: grid.planes(),
                max: MAX_PLANES as u32,
            });
        }
        let colors = rule.trace_colors(&frame.settings.render);
        if colors.len() != grid.planes() as usize {
            return Err(ScopeError::TraceColorCountMismatch {
                colors: colors.len(),
                planes: grid.planes(),
            });
        }
        let graticule: Vec<GraticulePrimitive> = rule.graticule(&frame.settings.render);
        if graticule.len() > MAX_GRATICULE_PRIMITIVES {
            return Err(ScopeError::TooManyGraticulePrimitives {
                count: graticule.len(),
                max: MAX_GRATICULE_PRIMITIVES,
            });
        }
        let parameters = rule.accumulate_parameters()?;
        Ok(RulePlan {
            accumulator,
            grid,
            words_bytes: accumulate_words_bytes(grid, &parameters)?,
            decode_table_bytes: parameters.decode_table.map(decode_table_bytes),
            render_bytes: render_block_bytes(grid, &colors, &graticule, &frame.settings.render),
        })
    }

    /// Новая картинка размера `size`, если действующей нет или она другого размера (`None` — действующая годится).
    /// Только создаёт объект: состояние владельца не меняется, при отказе проверок кадра картинка просто
    /// уничтожается.
    fn picture_candidate(&self, size: ScopeImageSize) -> Result<Option<PictureTarget>, ScopeError> {
        if self.is_picture_ready_for(size) {
            return Ok(None);
        }
        PictureTarget::new(&self.context, size).map(Some)
    }

    /// Принимает картинку, созданную [`Self::picture_candidate`]: прежняя (другого размера) уходит в отложенное
    /// уничтожение с номером записи `serial`. Единственное место, где меняется картинка и растёт счётчик замен.
    fn adopt_picture(&mut self, new_picture: Option<PictureTarget>, serial: RecordSerial) -> PictureChange {
        let Some(created) = new_picture else {
            return PictureChange::Unchanged;
        };
        let size = created.size();
        let Some(previous) = self.picture.replace(created) else {
            return PictureChange::Created;
        };
        tracing::debug!(
            target: "fasticamera::isp_scopes",
            from = ?previous.size(),
            to = ?size,
            "картинка прибора пересоздана под новый размер"
        );
        self.retired_pictures.retire(previous, serial);
        self.pass_counters.picture_recreations += 1;
        PictureChange::Replaced
    }

    /// Форма сетки последней записи (чтение счётчиков в тестах).
    #[cfg(feature = "test-support")]
    pub(super) fn last_grid(&self) -> Option<GridShape> {
        self.last_grid
    }

    /// Буфер счётчиков (чтение в тестах).
    #[cfg(feature = "test-support")]
    pub(super) fn counters_buffer(&self) -> &DeviceBuffer {
        &self.counters
    }

    /// Картинка (чтение в тестах).
    #[cfg(feature = "test-support")]
    pub(super) fn picture_target(&self) -> Option<&PictureTarget> {
        self.picture.as_ref()
    }
}

/// Проверяет вход: формат, окно внутри картинки, предел размера показанной картинки.
fn check_input(input: &ScopeInput) -> Result<CheckedInput, ScopeError> {
    match input.view.format {
        vk::Format::A2B10G10R10_UNORM_PACK32 | vk::Format::R16G16B16A16_SFLOAT => {}
        format => return Err(ScopeError::UnsupportedInputFormat { format }),
    }
    input
        .window
        .validate_within(input.view.extent)
        .map_err(ScopeError::InputWindow)?;
    let content = input.window.content();
    if content.width > MAX_SHOWN_SIDE
        || content.height > MAX_SHOWN_SIDE
        || u64::from(content.width) * u64::from(content.height) > MAX_SHOWN_PIXELS
    {
        return Err(ScopeError::InputTooLarge {
            width: content.width,
            height: content.height,
        });
    }
    Ok(CheckedInput {
        content,
        mirrored: input.window.mirror() == HorizontalMirror::On,
    })
}

/// Push-константы прохода накопления (`Sampling` в `scope_accumulate_common.glsl`, 7 слов).
fn sampling_push_bytes(input: &CheckedInput, stride: u32, offset: u32, samples_x: u32, samples_y: u32) -> [u8; 28] {
    let words = [
        input.content.width,
        input.content.height,
        stride,
        offset,
        samples_x,
        samples_y,
        u32::from(input.mirrored),
    ];
    let mut bytes = [0_u8; 28];
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(words) {
        chunk.copy_from_slice(&word.to_ne_bytes());
    }
    bytes
}

/// Ячеек в наибольшей сетке настроек: waveform/parade (`столбцы × уровни × 3`) или вектороскоп (`сторона²`).
fn largest_grid_cells(settings: &ScopeModelSettings) -> usize {
    let trace = settings.trace_columns as usize * settings.trace_levels as usize * MAX_PLANES;
    let vectorscope = settings.vectorscope_size as usize * settings.vectorscope_size as usize;
    trace.max(vectorscope)
}

// Пределы эталона, на которые опирается шейдер, закреплены на этапе компиляции: если эталон их увеличит,
// сборка остановится, а не переполнит `uint` шейдера молча.
const _: () = {
    // x · columns < 2^32 (положение отсчёта в waveform/parade).
    assert!(MAX_SHOWN_SIDE as u64 * MAX_GRID_SIDE as u64 <= u32::MAX as u64);
    // Индекс ячейки помещается в uint.
    assert!(MAX_GRID_CELLS <= u32::MAX as u64);
    assert!(MAX_VECTORSCOPE_SIZE <= MAX_GRID_SIDE);
    assert!(ACCUMULATE_BINDINGS.len() == 3);
};
