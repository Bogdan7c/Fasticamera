//! GPU-тесты меток времени: путь без поддержки, «не готово» без ожидания, защита от устаревших
//! значений, независимость слотов, учёт живых объектов.

use std::time::{Duration, Instant};

use ash::vk;

use super::*;
use crate::context::test_support::gpu_context_or_skip;
use crate::live_objects::{LiveObjectKind, OwnedCommandPool, OwnedFence};
use crate::sync::{QueueSubmission, TimelineSemaphore};

const LABELS: [&str; 3] = ["начало", "середина", "конец"];

/// Командный буфер, записанный замыканием, но ещё НЕ отправленный (для сабмита за «воротами»).
struct PendingCommands {
    command_buffer: vk::CommandBuffer,
    // Пул держит буфер живым; уничтожается после ожидания fence в тесте.
    _pool: OwnedCommandPool,
}

// SAFETY (для всех unsafe-блоков помощников ниже): устройство валидно, объекты создаются этим же
// устройством и живут до конца теста; командный буфер записывается в одном потоке, ожидание fence
// ограничено 5 секундами.
fn record_commands(
    context: &GpuContext,
    record: impl FnOnce(vk::CommandBuffer) -> Result<(), GpuError>,
) -> PendingCommands {
    let device = context.device();
    let pool_info = vk::CommandPoolCreateInfo::default().queue_family_index(context.queue_family_index());
    let pool = OwnedCommandPool::from_raw(
        context,
        unsafe { device.create_command_pool(&pool_info, None) }.unwrap(),
    );
    let allocate_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(pool.handle())
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    let command_buffer = unsafe { device.allocate_command_buffers(&allocate_info) }.unwrap()[0];
    unsafe { device.begin_command_buffer(command_buffer, &vk::CommandBufferBeginInfo::default()) }.unwrap();
    record(command_buffer).unwrap();
    unsafe { device.end_command_buffer(command_buffer) }.unwrap();
    PendingCommands {
        command_buffer,
        _pool: pool,
    }
}

fn new_fence(context: &GpuContext) -> OwnedFence {
    let info = vk::FenceCreateInfo::default();
    OwnedFence::from_raw(context, unsafe { context.device().create_fence(&info, None) }.unwrap())
}

fn wait_fence(context: &GpuContext, fence: &OwnedFence) {
    unsafe { context.device().wait_for_fences(&[fence.handle()], true, 5_000_000_000) }.unwrap();
}

fn record_all_points(timestamps: &mut GpuTimestamps, command_buffer: vk::CommandBuffer, slot: usize) {
    timestamps.record_reset(command_buffer, slot).unwrap();
    timestamps
        .record_point(command_buffer, slot, 0, TimestampStage::TopOfPipe)
        .unwrap();
    timestamps
        .record_point(command_buffer, slot, 1, TimestampStage::ComputeShaderDone)
        .unwrap();
    timestamps
        .record_point(command_buffer, slot, 2, TimestampStage::AllCommandsDone)
        .unwrap();
}

#[test]
fn unsupported_queue_gives_explicit_state_and_no_pool() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let mut timestamps =
        GpuTimestamps::with_availability(&context, 2, &LABELS, None, QueryResetMode::InCommandBuffer).unwrap();
    assert_eq!(timestamps.availability(), TimestampAvailability::Unsupported);
    assert_eq!(context.live_objects(), baseline, "без поддержки пул не создаётся");
    context
        .run_one_shot(|command_buffer| {
            // Запись без поддержки ничего не пишет и не падает.
            timestamps.record_reset(command_buffer, 0)?;
            timestamps.record_point(command_buffer, 0, 0, TimestampStage::TopOfPipe)
        })
        .unwrap();
    assert_eq!(timestamps.try_collect(0).unwrap(), TimestampCollect::Unsupported);
    // Ошибки аргументов остаются ошибками и без поддержки.
    assert!(timestamps.try_collect(2).is_err());
}

#[test]
fn invalid_shapes_are_rejected_without_creating_objects() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    assert!(GpuTimestamps::new(&context, 0, &LABELS).is_err());
    assert!(GpuTimestamps::new(&context, 2, &["одна"]).is_err());
    assert_eq!(context.live_objects(), baseline);
}

#[test]
fn not_ready_never_blocks_and_ready_results_follow_after_gpu_finishes() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let mut timestamps = GpuTimestamps::new(&context, 2, &LABELS).unwrap();
        assert!(matches!(timestamps.availability(), TimestampAvailability::Supported(_)));
        assert_eq!(
            context.live_objects().count(LiveObjectKind::QueryPool),
            2,
            "свой пул на каждый слот"
        );
        assert_eq!(timestamps.try_collect(0).unwrap(), TimestampCollect::NothingRecorded);

        let gate = TimelineSemaphore::new(&context, 0).unwrap();
        let fence = new_fence(&context);
        let commands = record_commands(&context, |command_buffer| {
            record_all_points(&mut timestamps, command_buffer, 0);
            Ok(())
        });
        // Сабмит ждёт «ворота»: GPU физически не может записать метки, пока CPU их не откроет.
        let _submitted = QueueSubmission::new()
            .wait_for_timeline(&gate, 1, vk::PipelineStageFlags2::TOP_OF_PIPE)
            .run_command_buffer(commands.command_buffer)
            .signal_fence(&fence)
            .submit(&context)
            .unwrap();

        // Ворота открываются из другого потока через 2 с: если бы try_collect ждал GPU, он вернулся
        // бы только после этого и с готовым результатом — тест бы упал, а не завис.
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_secs(2));
                gate.signal_from_cpu(1).unwrap();
            });
            let started = Instant::now();
            let collected = timestamps.try_collect(0).unwrap();
            assert!(started.elapsed() < Duration::from_secs(1), "try_collect ждал GPU");
            assert_eq!(collected, TimestampCollect::NotReady);
            // Слот 1 не тронут записью слота 0.
            assert_eq!(timestamps.try_collect(1).unwrap(), TimestampCollect::NothingRecorded);
        });
        wait_fence(&context, &fence);
        let TimestampCollect::Ready(spans) = timestamps.try_collect(0).unwrap() else {
            panic!("после fence метки обязаны быть готовы");
        };
        assert_eq!(spans.spans.len(), 2);
        assert_eq!((spans.spans[0].from, spans.spans[0].to), ("начало", "середина"));
        assert_eq!((spans.spans[1].from, spans.spans[1].to), ("середина", "конец"));
        // Каждый отрезок и итог переводятся из тиков в нс с округлением независимо, поэтому
        // сумма отрезков может отличаться от итога на ≤ 1 нс на отрезок (тест падал так ~1 раз
        // из 3 на RADV — свойство «точное равенство» было неверным, а не код).
        let sum_of_spans = spans.spans.iter().map(|span| span.duration_ns).sum::<u64>();
        assert!(
            spans.total_ns.abs_diff(sum_of_spans) <= spans.spans.len() as u64,
            "отрезки складываются в полное время с точностью округления: итог {}, сумма {sum_of_spans}",
            spans.total_ns
        );
    }
    assert_eq!(context.live_objects(), baseline);
    assert_eq!(
        context.validation_report().errors,
        0,
        "{:?}",
        context.validation_report()
    );
}

#[test]
fn stale_results_of_previous_recording_are_not_reported_as_new() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut timestamps = GpuTimestamps::new(&context, 1, &LABELS).unwrap();
    context
        .run_one_shot(|command_buffer| {
            record_all_points(&mut timestamps, command_buffer, 0);
            Ok(())
        })
        .unwrap();
    let TimestampCollect::Ready(first) = timestamps.try_collect(0).unwrap() else {
        panic!("первый сбор обязан быть готов");
    };

    // Новая запись слота за закрытыми воротами: сброс ещё не исполнен, в пуле — старые значения.
    let gate = TimelineSemaphore::new(&context, 0).unwrap();
    let fence = new_fence(&context);
    let commands = record_commands(&context, |command_buffer| {
        record_all_points(&mut timestamps, command_buffer, 0);
        Ok(())
    });
    let _submitted = QueueSubmission::new()
        .wait_for_timeline(&gate, 1, vk::PipelineStageFlags2::TOP_OF_PIPE)
        .run_command_buffer(commands.command_buffer)
        .signal_fence(&fence)
        .submit(&context)
        .unwrap();
    assert_eq!(
        timestamps.try_collect(0).unwrap(),
        TimestampCollect::NotReady,
        "старые значения нельзя выдавать за новый кадр"
    );
    gate.signal_from_cpu(1).unwrap();
    wait_fence(&context, &fence);
    let TimestampCollect::Ready(second) = timestamps.try_collect(0).unwrap() else {
        panic!("после fence новая запись готова");
    };
    assert_eq!(second.spans.len(), first.spans.len());
    assert_eq!(
        context.validation_report().errors,
        0,
        "{:?}",
        context.validation_report()
    );
}

/// Метки обоими способами сброса: основной (`Host`, если устройство умеет) и запасной (командой).
fn timestamps_in_every_reset_mode(context: &GpuContext) -> Vec<GpuTimestamps> {
    let support = context.capabilities().timestamps;
    let mut modes = vec![QueryResetMode::InCommandBuffer];
    if context.host_query_reset_enabled() {
        modes.push(QueryResetMode::Host);
    }
    modes
        .into_iter()
        .map(|mode| GpuTimestamps::with_availability(context, 1, &LABELS, support, mode).unwrap())
        .collect()
}

#[test]
fn uncollected_previous_recording_is_never_reported_as_the_new_one() {
    let Some(context) = gpu_context_or_skip() else { return };
    assert!(
        context.capabilities().timestamps.is_some(),
        "тест требует меток времени на очереди"
    );
    for mut timestamps in timestamps_in_every_reset_mode(&context) {
        let mode = timestamps.reset_mode();
        // Запись 1 исполнена (fence дождались), но её результат НИКТО не собрал.
        context
            .run_one_shot(|command_buffer| {
                record_all_points(&mut timestamps, command_buffer, 0);
                Ok(())
            })
            .unwrap();

        // Запись 2 того же слота за закрытыми воротами: её сброс и метки ещё не исполнены.
        let gate = TimelineSemaphore::new(&context, 0).unwrap();
        let fence = new_fence(&context);
        let commands = record_commands(&context, |command_buffer| {
            record_all_points(&mut timestamps, command_buffer, 0);
            Ok(())
        });
        let _submitted = QueueSubmission::new()
            .wait_for_timeline(&gate, 1, vk::PipelineStageFlags2::TOP_OF_PIPE)
            .run_command_buffer(commands.command_buffer)
            .signal_fence(&fence)
            .submit(&context)
            .unwrap();
        let before_gate = timestamps.try_collect(0).unwrap();
        // Сначала открываем ворота и ждём fence: упавшая проверка не должна оставить сабмит висеть.
        gate.signal_from_cpu(1).unwrap();
        wait_fence(&context, &fence);
        assert_eq!(
            before_gate,
            TimestampCollect::NotReady,
            "{mode:?}: значения несобранной записи 1 нельзя выдавать за запись 2"
        );
        assert!(
            matches!(timestamps.try_collect(0).unwrap(), TimestampCollect::Ready(_)),
            "{mode:?}: после fence запись 2 готова"
        );
    }
    assert_eq!(
        context.validation_report().errors,
        0,
        "{:?}",
        context.validation_report()
    );
}

/// Регрессия (P7): сброс слота 1, пока буфер слота 0 ещё летит. С общим пулом validation сообщала
/// `VUID-vkResetQueryPool-firstQuery-02741` («запросы пула используются буфером») — она следит за
/// занятостью всего пула, а не участка слота. Пул на слот снимает конфликт в любом режиме сброса.
#[test]
fn resetting_one_slot_while_another_slot_is_in_flight_is_valid() {
    let Some(context) = gpu_context_or_skip() else { return };
    let support = context.capabilities().timestamps;
    let mut modes = vec![QueryResetMode::InCommandBuffer];
    if context.host_query_reset_enabled() {
        modes.push(QueryResetMode::Host);
    }
    for mode in modes {
        let mut timestamps = GpuTimestamps::with_availability(&context, 2, &LABELS, support, mode).unwrap();
        // Слот 0: буфер за закрытыми воротами — «летит» (ожидает исполнения).
        let gate = TimelineSemaphore::new(&context, 0).unwrap();
        let fence = new_fence(&context);
        let commands = record_commands(&context, |command_buffer| {
            record_all_points(&mut timestamps, command_buffer, 0);
            Ok(())
        });
        let _submitted = QueueSubmission::new()
            .wait_for_timeline(&gate, 1, vk::PipelineStageFlags2::TOP_OF_PIPE)
            .run_command_buffer(commands.command_buffer)
            .signal_fence(&fence)
            .submit(&context)
            .unwrap();
        // Слот 1 сбрасывается и пишется, пока слот 0 в полёте (fence слота 1 свободен — он не использовался).
        let slot_one = record_commands(&context, |command_buffer| {
            record_all_points(&mut timestamps, command_buffer, 1);
            Ok(())
        });
        gate.signal_from_cpu(1).unwrap();
        wait_fence(&context, &fence);
        let slot_one_fence = new_fence(&context);
        let _slot_one_submitted = QueueSubmission::new()
            .run_command_buffer(slot_one.command_buffer)
            .signal_fence(&slot_one_fence)
            .submit(&context)
            .unwrap();
        wait_fence(&context, &slot_one_fence);
        assert!(
            matches!(timestamps.try_collect(0).unwrap(), TimestampCollect::Ready(_)),
            "{mode:?}"
        );
        assert!(
            matches!(timestamps.try_collect(1).unwrap(), TimestampCollect::Ready(_)),
            "{mode:?}"
        );
        assert_eq!(
            context.validation_report().errors,
            0,
            "{mode:?}: {:?}",
            context.validation_report()
        );
    }
}

#[test]
fn point_errors_do_not_change_slot_state() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut timestamps = GpuTimestamps::new(&context, 1, &LABELS).unwrap();
    context
        .run_one_shot(|command_buffer| {
            timestamps.record_reset(command_buffer, 0)?;
            timestamps.record_point(command_buffer, 0, 0, TimestampStage::TopOfPipe)?;
            // Точка вне списка, слот вне кольца и повтор точки — ошибки до записи команды.
            assert!(
                timestamps
                    .record_point(command_buffer, 0, 3, TimestampStage::TopOfPipe)
                    .is_err()
            );
            assert!(
                timestamps
                    .record_point(command_buffer, 1, 0, TimestampStage::TopOfPipe)
                    .is_err()
            );
            let repeat = timestamps.record_point(command_buffer, 0, 0, TimestampStage::TopOfPipe);
            assert!(matches!(repeat, Err(GpuError::InvalidState { .. })), "{repeat:?}");
            timestamps.record_point(command_buffer, 0, 2, TimestampStage::AllCommandsDone)
        })
        .unwrap();
    // Записаны две точки (0 и 2) — один отрезок «начало → конец».
    let TimestampCollect::Ready(spans) = timestamps.try_collect(0).unwrap() else {
        panic!("метки готовы после разовой отправки");
    };
    assert_eq!(spans.spans.len(), 1);
    assert_eq!((spans.spans[0].from, spans.spans[0].to), ("начало", "конец"));
    assert_eq!(
        context.validation_report().errors,
        0,
        "{:?}",
        context.validation_report()
    );
}

#[test]
fn spans_are_wrap_safe_and_scaled_by_period() {
    let support = TimestampSupport {
        valid_bits: 8,
        period_ns: 2.0,
    };
    // 250 → 4 (переход через ноль) → 10: 10 и 6 тиков = 20 и 12 нс.
    let spans = spans_from_ticks(&LABELS, &[(0, 250), (1, 4), (2, 10)], support);
    assert_eq!(spans.spans[0].duration_ns, 20);
    assert_eq!(spans.spans[1].duration_ns, 12);
    assert_eq!(spans.total_ns, 32);
}
