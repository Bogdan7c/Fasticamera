//! [`GpuTimestamps`] — метки времени GPU по слотам кадров: «сколько миллисекунд GPU стоит стадия».
//!
//! Решение 45 (бюджет «стадия → мс GPU → мА»): время каждой стадии ISP меряем `vkCmdWriteTimestamp2`
//! в нашем коде, на каждом кадре и в release — без root и без внешних профилировщиков.
//!
//! # Модель
//! * СВОЙ `VkQueryPool` на каждый слот кадра (`points` запросов): метки кадра k не затираются, пока GPU
//!   пишет кадр k+1, и сброс с CPU слота k+1 не трогает пул, который ещё использует летящий буфер слота k
//!   (validation отслеживает «занятость» запросов по всему пулу — `VUID-vkResetQueryPool-firstQuery-02741`);
//! * точки помечены именами (`point_labels`), результат — отрезки между соседними записанными точками;
//! * `record_reset` → точки `record_point` → сабмит → когда-нибудь позже `try_collect`.
//!
//! # `try_collect` никогда не ждёт
//! Флаг `WAIT` не ставится: незавершённые метки → `TimestampCollect::NotReady` («ещё нет»), а не
//! блокировка потока рендера. Звать его удобно сразу после ожидания fence слота, перед новой записью
//! слота — тогда результат готов.
//!
//! # Защита от устаревших значений
//! Запрос, который уже содержит результат, остаётся «готовым», пока его не сбросят. Сброс командой
//! (`vkCmdResetQueryPool`) действует только когда сабмит дойдёт до GPU: `try_collect` между новой записью
//! слота и исполнением сброса прочитал бы ПРЕЖНИЕ значения как новые (спецификация,
//! `vkGetQueryPoolResults`: «a stale value could be returned from a previous use of the query»).
//! * **Основной путь — `hostQueryReset` (Vulkan 1.2, есть у RADV и Adreno 740).** `record_reset`
//!   сбрасывает запросы слота С CPU (`vkResetQueryPool`) сразу: с этого момента они «не готовы», пока
//!   GPU не запишет новые метки, — старые значения прочитать невозможно. Условие спецификации («команды,
//!   ссылающиеся на эти запросы, завершены») — это контракт слота: `record_reset` зовут, только когда
//!   fence прошлого сабмита слота сигнален.
//! * **Запасной путь — сброс командой.** Тогда при `record_reset` (fence прошлой записи слота уже
//!   сигнален по контракту) объект без ожидания читает метку начала прошлой записи и запоминает её;
//!   `try_collect` с той же первой меткой отвечает `NotReady`: метки строго растут, совпадение = старые
//!   данные. Так защита работает и когда прошлую запись так и не собрали.
//!
//! # Нет поддержки
//! `GpuCapabilities::timestamps == None` (очередь без меток) — объект создаётся в состоянии
//! [`TimestampAvailability::Unsupported`]: `record_*` ничего не пишут, `try_collect` честно отвечает
//! `Unsupported`. Паники и ошибки создания нет — видоискатель работает и без замера.

pub(crate) mod math;

use ash::vk;

use self::math::{elapsed_ticks, ticks_to_nanoseconds};
use crate::context::{GpuContext, TimestampSupport};
use crate::error::{GpuError, run_step};
use crate::live_objects::OwnedQueryPool;

/// На какой стадии конвейера записать метку: метка ставится, когда все предыдущие команды очереди
/// прошли эту стадию.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampStage {
    /// Как можно раньше (начало работы).
    TopOfPipe,
    /// После завершения compute-шейдеров предыдущих команд.
    ComputeShaderDone,
    /// После завершения фрагментных шейдеров предыдущих команд.
    FragmentShaderDone,
    /// После завершения всех предыдущих команд.
    AllCommandsDone,
}

impl TimestampStage {
    fn pipeline_stage(self) -> vk::PipelineStageFlags2 {
        match self {
            Self::TopOfPipe => vk::PipelineStageFlags2::TOP_OF_PIPE,
            Self::ComputeShaderDone => vk::PipelineStageFlags2::COMPUTE_SHADER,
            Self::FragmentShaderDone => vk::PipelineStageFlags2::FRAGMENT_SHADER,
            Self::AllCommandsDone => vk::PipelineStageFlags2::ALL_COMMANDS,
        }
    }
}

/// Поддерживает ли очередь метки времени.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimestampAvailability {
    /// Метки пишутся.
    Supported(TimestampSupport),
    /// Очередь не поддерживает метки (`timestampValidBits == 0`): замеров не будет.
    Unsupported,
}

/// Отрезок между двумя соседними записанными точками.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimestampSpan {
    /// Имя начальной точки.
    pub from: &'static str,
    /// Имя конечной точки.
    pub to: &'static str,
    /// Длительность, наносекунды.
    pub duration_ns: u64,
}

/// Результат одного слота: отрезки по порядку точек и полное время от первой до последней.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimestampSpans {
    /// Отрезки между соседними записанными точками.
    pub spans: Vec<TimestampSpan>,
    /// От первой записанной точки до последней, наносекунды.
    pub total_ns: u64,
}

/// Итог `try_collect`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimestampCollect {
    /// Метки готовы.
    Ready(TimestampSpans),
    /// GPU ещё не дошёл до меток (или сброс слота ещё не исполнен): спросить позже.
    NotReady,
    /// В слот после создания (или после сброса) не записано ни одной точки.
    NothingRecorded,
    /// Очередь не поддерживает метки.
    Unsupported,
}

/// Как сбрасываются запросы слота перед новой записью (см. «Защита от устаревших значений»).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryResetMode {
    /// `vkResetQueryPool` с CPU (`hostQueryReset` включена): старые значения недоступны сразу.
    Host,
    /// `vkCmdResetQueryPool` в командном буфере + запоминание первой метки прошлой записи.
    InCommandBuffer,
}

/// Что записано в слот с последнего `record_reset`.
#[derive(Debug, Clone, Copy, Default)]
struct SlotRecord {
    /// Битовая маска записанных точек.
    recorded_points: u64,
    /// Первая метка (в тиках) последнего успешного сбора — защита от устаревших значений.
    last_collected_first_tick: Option<u64>,
}

/// Метки времени GPU по слотам кадров.
pub struct GpuTimestamps {
    availability: TimestampAvailability,
    reset_mode: QueryResetMode,
    /// Пул на слот кадра (по `points` запросов); пусто — метки не поддерживаются.
    pools: Vec<OwnedQueryPool>,
    point_labels: Vec<&'static str>,
    slots: Vec<SlotRecord>,
    context: GpuContext,
}

impl std::fmt::Debug for GpuTimestamps {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "GpuTimestamps({:?}, {} слотов × {:?})",
            self.availability,
            self.slots.len(),
            self.point_labels
        )
    }
}

/// Больше точек на слот не бывает: маска записанных точек — `u64`.
const MAX_POINTS: usize = 64;

impl GpuTimestamps {
    /// Создаёт метки на `frame_slots` слотов по точке на каждое имя из `point_labels`.
    ///
    /// Пул сразу сбрасывается разовой отправкой (блокирующей, с таймаутом контекста): после этого
    /// любой запрос пула «сброшен, но не готов», и `try_collect` до первой записи не наткнётся на
    /// неинициализированные запросы.
    pub fn new(context: &GpuContext, frame_slots: usize, point_labels: &[&'static str]) -> Result<Self, GpuError> {
        let reset_mode = if context.host_query_reset_enabled() {
            QueryResetMode::Host
        } else {
            QueryResetMode::InCommandBuffer
        };
        Self::with_availability(
            context,
            frame_slots,
            point_labels,
            context.capabilities().timestamps,
            reset_mode,
        )
    }

    /// То же с явной поддержкой и способом сброса (тесты подставляют `None`, чтобы проверить путь без
    /// меток, и `InCommandBuffer`, чтобы проверить запасной путь на устройстве с `hostQueryReset`).
    pub(crate) fn with_availability(
        context: &GpuContext,
        frame_slots: usize,
        point_labels: &[&'static str],
        support: Option<TimestampSupport>,
        reset_mode: QueryResetMode,
    ) -> Result<Self, GpuError> {
        if reset_mode == QueryResetMode::Host && !context.host_query_reset_enabled() {
            return Err(GpuError::InvalidArgument {
                what: "сброс меток с CPU без включённой hostQueryReset".to_owned(),
            });
        }
        if frame_slots == 0 || point_labels.len() < 2 || point_labels.len() > MAX_POINTS {
            return Err(GpuError::InvalidArgument {
                what: format!(
                    "метки: слотов {frame_slots} (нужно ≥ 1), точек {} (нужно 2..={MAX_POINTS})",
                    point_labels.len()
                ),
            });
        }
        // Не больше MAX_POINTS (проверено выше) — помещается в u32.
        let points_per_slot = point_labels.len() as u32;
        let (availability, pools) = match support {
            Some(support) => (
                TimestampAvailability::Supported(support),
                (0..frame_slots)
                    .map(|_| create_reset_pool(context, points_per_slot))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            None => (TimestampAvailability::Unsupported, Vec::new()),
        };
        Ok(Self {
            availability,
            reset_mode,
            pools,
            point_labels: point_labels.to_vec(),
            slots: vec![SlotRecord::default(); frame_slots],
            context: context.clone(),
        })
    }

    /// Поддерживаются ли метки.
    pub fn availability(&self) -> TimestampAvailability {
        self.availability
    }

    /// Число слотов.
    pub fn frame_slots(&self) -> usize {
        self.slots.len()
    }

    /// Сбрасывает точки слота перед новой записью — в начале командного буфера кадра, ВНЕ динамического
    /// рендеринга. С `hostQueryReset` сброс делается сразу на CPU и в буфер ничего не пишется; без неё —
    /// `vkCmdResetQueryPool` в буфере (см. «Защита от устаревших значений»). Без поддержки меток — ничего.
    ///
    /// Контракт: fence последнего сабмита, записавшего точки этого слота, сигнален (как у всех ресурсов
    /// слота). Прежний результат слота, если его не собрали, теряется.
    pub fn record_reset(&mut self, command_buffer: vk::CommandBuffer, frame_slot: usize) -> Result<(), GpuError> {
        let first_query = self.first_query(frame_slot)?;
        let Some(pool) = self.pools.get(frame_slot) else {
            return Ok(());
        };
        match self.reset_mode {
            QueryResetMode::Host => {
                // SAFETY: hostQueryReset включена (режим выбран по возможностям контекста); диапазон слота
                // внутри пула (first_query); команды, ссылавшиеся на эти запросы, завершены — fence слота
                // сигнален (контракт метода); пул не используется другими потоками (`&mut self`).
                unsafe {
                    self.context
                        .device()
                        .reset_query_pool(pool.handle(), first_query, self.points_per_slot());
                }
            }
            QueryResetMode::InCommandBuffer => {
                // Прошлая запись слота завершена (контракт): её первая метка уже доступна. Запоминаем её,
                // чтобы `try_collect` не выдал эти значения за новые до исполнения сброса.
                let previous_first_tick = self.read_first_recorded_tick(frame_slot)?;
                if previous_first_tick.is_some() {
                    self.slots[frame_slot].last_collected_first_tick = previous_first_tick;
                }
                // SAFETY: командный буфер в состоянии записи вне рендеринга (контракт вызывающего); диапазон
                // запросов слота лежит внутри пула (проверено в first_query).
                unsafe {
                    self.context.device().cmd_reset_query_pool(
                        command_buffer,
                        pool.handle(),
                        first_query,
                        self.points_per_slot(),
                    );
                }
            }
        }
        self.slots[frame_slot].recorded_points = 0;
        Ok(())
    }

    /// Как сбрасываются запросы перед новой записью слота.
    pub fn reset_mode(&self) -> QueryResetMode {
        self.reset_mode
    }

    /// Записывает точку `point_index` слота на стадии `stage` (`vkCmdWriteTimestamp2`).
    /// Точку можно писать один раз между сбросами (повтор — `InvalidState`: так требует Vulkan).
    pub fn record_point(
        &mut self,
        command_buffer: vk::CommandBuffer,
        frame_slot: usize,
        point_index: usize,
        stage: TimestampStage,
    ) -> Result<(), GpuError> {
        let first_query = self.first_query(frame_slot)?;
        if point_index >= self.point_labels.len() {
            return Err(GpuError::InvalidArgument {
                what: format!("точка {point_index} вне 0..{}", self.point_labels.len()),
            });
        }
        let Some(pool) = self.pools.get(frame_slot) else {
            return Ok(());
        };
        let point_bit = 1_u64 << point_index;
        if self.slots[frame_slot].recorded_points & point_bit != 0 {
            return Err(GpuError::InvalidState {
                what: format!(
                    "точка {:?} слота {frame_slot} уже записана после сброса",
                    self.point_labels[point_index]
                ),
            });
        }
        let query = first_query + point_index as u32;
        // SAFETY: командный буфер в состоянии записи (контракт вызывающего); запрос сброшен
        // `record_reset` этого же слота раньше в порядке записи и не записан повторно (проверено).
        unsafe {
            self.context
                .device()
                .cmd_write_timestamp2(command_buffer, stage.pipeline_stage(), pool.handle(), query);
        }
        self.slots[frame_slot].recorded_points |= point_bit;
        Ok(())
    }

    /// Пробует забрать метки слота. НИКОГДА не ждёт GPU: не готово — `NotReady`.
    pub fn try_collect(&mut self, frame_slot: usize) -> Result<TimestampCollect, GpuError> {
        let first_query = self.first_query(frame_slot)?;
        let (Some(pool), TimestampAvailability::Supported(support)) = (self.pools.get(frame_slot), self.availability)
        else {
            return Ok(TimestampCollect::Unsupported);
        };
        let record = self.slots[frame_slot];
        if record.recorded_points == 0 {
            return Ok(TimestampCollect::NothingRecorded);
        }
        let recorded = match self.read_recorded_ticks(pool, first_query, record.recorded_points)? {
            Some(recorded) => recorded,
            None => return Ok(TimestampCollect::NotReady),
        };
        let first_tick = recorded.first().map(|(_, tick)| *tick);
        if first_tick.is_some() && first_tick == record.last_collected_first_tick {
            // Запасной путь: сброс новой записи ещё не исполнен — это значения прошлой записи.
            return Ok(TimestampCollect::NotReady);
        }
        self.slots[frame_slot].last_collected_first_tick = first_tick;
        Ok(TimestampCollect::Ready(spans_from_ticks(
            &self.point_labels,
            &recorded,
            support,
        )))
    }

    /// Читает без ожидания все записанные (по маске) точки слота. `None` — хотя бы одна ещё не готова.
    fn read_recorded_ticks(
        &self,
        pool: &OwnedQueryPool,
        first_query: u32,
        recorded_points: u64,
    ) -> Result<Option<Vec<(usize, u64)>>, GpuError> {
        let mut recorded: Vec<(usize, u64)> = Vec::new();
        for point_index in 0..self.point_labels.len() {
            if recorded_points & (1_u64 << point_index) == 0 {
                continue;
            }
            let mut ticks = [0_u64; 1];
            // SAFETY: запрос внутри пула; флаг WAIT не ставится — вызов не блокируется; буфер на один u64.
            let result = unsafe {
                self.context.device().get_query_pool_results(
                    pool.handle(),
                    first_query + point_index as u32,
                    &mut ticks,
                    vk::QueryResultFlags::TYPE_64,
                )
            };
            match result {
                Ok(()) => recorded.push((point_index, ticks[0])),
                Err(vk::Result::NOT_READY) => return Ok(None),
                Err(code) => return run_step(Err(code), "vkGetQueryPoolResults(метки)"),
            }
        }
        Ok(Some(recorded))
    }

    /// Первая записанная метка прошлой записи слота (запасной путь сброса); `None` — нечего читать.
    fn read_first_recorded_tick(&self, frame_slot: usize) -> Result<Option<u64>, GpuError> {
        let record = self.slots[frame_slot];
        let (Some(pool), Ok(first_query)) = (self.pools.get(frame_slot), self.first_query(frame_slot)) else {
            return Ok(None);
        };
        if record.recorded_points == 0 {
            return Ok(None);
        }
        let first_point = record.recorded_points.trailing_zeros();
        let recorded = self.read_recorded_ticks(pool, first_query, 1_u64 << first_point)?;
        Ok(recorded.and_then(|ticks| ticks.first().map(|(_, tick)| *tick)))
    }

    fn points_per_slot(&self) -> u32 {
        // Не больше MAX_POINTS (проверено при создании).
        self.point_labels.len() as u32
    }

    /// Номер первого запроса слота в ЕГО пуле (всегда 0: пул у слота свой); слот вне кольца — `InvalidArgument`.
    fn first_query(&self, frame_slot: usize) -> Result<u32, GpuError> {
        if frame_slot >= self.slots.len() {
            return Err(GpuError::InvalidArgument {
                what: format!("слот меток {frame_slot} вне 0..{}", self.slots.len()),
            });
        }
        Ok(0)
    }
}

/// Отрезки между соседними записанными точками (по возрастанию номера точки).
fn spans_from_ticks(labels: &[&'static str], recorded: &[(usize, u64)], support: TimestampSupport) -> TimestampSpans {
    let spans: Vec<TimestampSpan> = recorded
        .windows(2)
        .map(|pair| {
            let (from_index, from_tick) = pair[0];
            let (to_index, to_tick) = pair[1];
            TimestampSpan {
                from: labels[from_index],
                to: labels[to_index],
                duration_ns: ticks_to_nanoseconds(
                    elapsed_ticks(from_tick, to_tick, support.valid_bits),
                    support.period_ns,
                ),
            }
        })
        .collect();
    let total_ns = match (recorded.first(), recorded.last()) {
        (Some((_, first)), Some((_, last))) => {
            ticks_to_nanoseconds(elapsed_ticks(*first, *last, support.valid_bits), support.period_ns)
        }
        _ => 0,
    };
    TimestampSpans { spans, total_ns }
}

/// Создаёт пул слота и сразу сбрасывает все его запросы разовой отправкой.
fn create_reset_pool(context: &GpuContext, query_count: u32) -> Result<OwnedQueryPool, GpuError> {
    let info = vk::QueryPoolCreateInfo::default()
        .query_type(vk::QueryType::TIMESTAMP)
        .query_count(query_count);
    // SAFETY: устройство валидно; info — локальная структура.
    let pool = OwnedQueryPool::from_raw(
        context,
        run_step(
            unsafe { context.device().create_query_pool(&info, None) },
            "vkCreateQueryPool(метки)",
        )?,
    );
    let handle = pool.handle();
    context.run_one_shot(|command_buffer| {
        // SAFETY: командный буфер в состоянии записи (контракт run_one_shot); весь диапазон пула.
        unsafe {
            context
                .device()
                .cmd_reset_query_pool(command_buffer, handle, 0, query_count)
        };
        Ok(())
    })?;
    Ok(pool)
}

#[cfg(test)]
mod tests;
