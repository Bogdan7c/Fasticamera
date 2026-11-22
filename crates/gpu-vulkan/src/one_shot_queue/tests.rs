//! Тесты очереди одноразовой работы на настоящем GPU (RADV, validation FULL).
//!
//! «Ворота»: работа GPU ждёт импортированный fence, который сигналит timeline-семафор, управляемый тестом.
//! Пока ворота закрыты, работа обязана быть `Pending` и НЕ выполнить команды; после открытия — `Completed`.

use std::time::{Duration, Instant};

use ash::vk;

use super::*;
use crate::context::test_support::gpu_context_or_skip;
use crate::host_readback::{HostReadbackBuffer, SealedHostBytes};
use crate::live_objects::LiveObjectKind;
use crate::one_shot_queue::SemaphoreSync;
use crate::sync::{
    ExportableSemaphore, GpuWaitFence, QueueSubmission, TimelineSemaphore, WaitSemaphore, import_wait_fence,
};

const FILL_VALUE: u32 = 0xAB_AB_AB_AB;
const BUFFER_BYTES: u64 = 64;
/// Срок ожидания «должно случиться быстро»: на ПК GPU-работа из нескольких команд — миллисекунды.
const EVENTUALLY: Duration = Duration::from_secs(10);

/// Ворота: timeline-семафор, значение которого выставляет тест с CPU (`open`). Работа ждёт значение 1
/// ДО команд (wait-before-signal) — заведомо незавершённая работа, а отправка не блокируется.
///
/// Не годятся: `VkEvent` (host-`vkSetEvent` на событие, которого ждёт отправленный буфер, запрещён
/// VUID-vkSetEvent-event-09543), экспорт `sync_fd` и бинарный семафор, чей сигнал ждёт timeline (RADV
/// блокирует поток, пока сигнал не материализован; проверено: тесты зависали).
struct Gate {
    timeline: TimelineSemaphore,
    context: GpuContext,
}

impl Gate {
    fn new(context: &GpuContext) -> Self {
        Self {
            timeline: TimelineSemaphore::new(context, 0).unwrap(),
            context: context.clone(),
        }
    }

    fn open(&self) {
        self.timeline.signal_from_cpu(1).unwrap();
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        // Незакрытые ворота открываем (повторный сигнал — ошибка монотонности, её игнорируем), иначе
        // ожидание простоя устройства не вернулось бы.
        let _ = self.timeline.signal_from_cpu(1);
        let _ = self.context.wait_idle();
    }
}

/// Отправка, которая ждёт ворота до выполнения команд (`buffer` — что заполнить после; `None` — ничего).
fn submit_gated(
    context: &GpuContext,
    submitter: &mut OneShotSubmitter,
    gate: &Gate,
    buffer: Option<&HostReadbackBuffer>,
) -> SubmittedWork {
    let timeline_waits = [(gate.timeline.handle(), 1_u64)];
    submitter
        .submit_with_sync(
            Vec::new(),
            &SemaphoreSync {
                timeline_waits: &timeline_waits,
                ..SemaphoreSync::default()
            },
            |command_buffer| match buffer {
                Some(buffer) => fill_commands(context, buffer)(command_buffer),
                None => Ok::<(), GpuError>(()),
            },
        )
        .unwrap()
}

/// Запись: заполнить буфер значением и сделать запись видимой хосту.
fn fill_commands<'a>(
    context: &'a GpuContext,
    buffer: &'a HostReadbackBuffer,
) -> impl FnOnce(vk::CommandBuffer) -> Result<(), GpuError> + 'a {
    move |command_buffer| {
        // SAFETY: командный буфер в состоянии записи; буфер имеет usage TRANSFER_DST и размер BUFFER_BYTES.
        unsafe {
            context
                .device()
                .cmd_fill_buffer(command_buffer, buffer.buffer_handle(), 0, BUFFER_BYTES, FILL_VALUE);
        }
        buffer.record_transfer_write_to_host_read(command_buffer);
        Ok(())
    }
}

/// Опрашивает работу, пока она не завершится (не дольше `EVENTUALLY`), и возвращает доказательство.
fn wait_completed(submitter: &mut OneShotSubmitter, work: &SubmittedWork) -> CompletedWork {
    let deadline = Instant::now() + EVENTUALLY;
    loop {
        match submitter.poll(work) {
            WorkStatus::Completed(proof) => return proof,
            WorkStatus::Pending => {
                assert!(Instant::now() < deadline, "работа не завершилась за {EVENTUALLY:?}");
                std::thread::sleep(Duration::from_millis(1));
            }
            WorkStatus::Failed(failure) => panic!("опрос не удался: {failure:?}"),
        }
    }
}

fn assert_clean(context: &GpuContext) {
    context.wait_idle().unwrap();
    assert_eq!(
        context.validation_report().errors,
        0,
        "{:?}",
        context.validation_report()
    );
}

/// Отмечает работу как писавшую в буфер и запечатывает его доказательством (путь настоящего потребителя).
fn seal_after(mut buffer: HostReadbackBuffer, work: &SubmittedWork, proof: &CompletedWork) -> SealedHostBytes {
    buffer.mark_written_by(work);
    buffer.seal(proof).unwrap()
}

fn filled(bytes: &SealedHostBytes) -> bool {
    bytes.bytes() == [0xAB_u8; BUFFER_BYTES as usize].as_slice()
}

#[test]
fn zero_capacity_is_rejected_without_creating_objects() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let error = OneShotSubmitter::new(&context, 0).unwrap_err();
    assert!(matches!(error, GpuError::InvalidArgument { .. }), "{error:?}");
    assert_eq!(context.live_objects(), baseline);
}

#[test]
fn submitted_work_completes_and_gpu_bytes_are_visible_after_poll_without_blocking() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let mut submitter = OneShotSubmitter::new(&context, 2).unwrap();
        assert_eq!(submitter.capacity(), 2);
        let buffer = HostReadbackBuffer::new(&context, BUFFER_BYTES).unwrap();
        let before_submit = Instant::now();
        let work = submitter.submit(Vec::new(), fill_commands(&context, &buffer)).unwrap();
        assert!(
            before_submit.elapsed() < EVENTUALLY,
            "submit не должен ждать GPU (иначе это run_one_shot)"
        );
        assert_eq!(submitter.in_flight_count(), 1);
        let proof = wait_completed(&mut submitter, &work);
        assert_eq!(
            proof.id(),
            work.id(),
            "доказательство несёт идентификатор именно этой работы"
        );
        let sealed = seal_after(buffer, &work, &proof);
        assert!(filled(&sealed), "байты, записанные GPU, видны после poll → Completed");
        // Повторный опрос завершённой работы остаётся Completed (идемпотентно).
        assert!(matches!(submitter.poll(&work), WorkStatus::Completed(_)));
    }
    assert_eq!(context.live_objects(), baseline, "очередь, буфер и память освобождены");
    assert_clean(&context);
}

#[test]
fn work_stays_pending_and_does_not_run_commands_until_the_gpu_gate_opens() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let mut submitter = OneShotSubmitter::new(&context, 1).unwrap();
        let gate = Gate::new(&context);
        let buffer = HostReadbackBuffer::new(&context, BUFFER_BYTES).unwrap();
        let work = submit_gated(&context, &mut submitter, &gate, Some(&buffer));
        // Ворота закрыты: даём GPU заведомо достаточно времени, работа обязана висеть.
        for _ in 0..20 {
            assert!(matches!(submitter.poll(&work), WorkStatus::Pending));
            std::thread::sleep(Duration::from_millis(5));
        }
        gate.open();
        let proof = wait_completed(&mut submitter, &work);
        let sealed = seal_after(buffer, &work, &proof);
        assert!(
            filled(&sealed),
            "команды выполнились ПОСЛЕ открытия ворот (и не раньше)"
        );
    }
    assert_eq!(context.live_objects(), baseline);
    assert_clean(&context);
}

#[test]
fn exhausted_capacity_is_an_explicit_error_not_a_block_and_slot_returns_after_completion() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let mut submitter = OneShotSubmitter::new(&context, 2).unwrap();
        let gate_a = Gate::new(&context);
        let gate_b = Gate::new(&context);
        let buffer = HostReadbackBuffer::new(&context, BUFFER_BYTES).unwrap();
        let first = submit_gated(&context, &mut submitter, &gate_a, None);
        let second = submit_gated(&context, &mut submitter, &gate_b, None);
        assert_eq!(submitter.in_flight_count(), 2);

        let mut record_called = false;
        let started = Instant::now();
        let third = submitter.submit(Vec::new(), |_| {
            record_called = true;
            Ok::<(), GpuError>(())
        });
        assert!(
            matches!(third, Err(SubmitError::CapacityExhausted { capacity: 2 })),
            "{third:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(1), "исчерпание не блокирует");
        assert!(!record_called, "при исчерпании запись не вызывается");
        assert_eq!(submitter.in_flight_count(), 2, "отказ не трогает занятые слоты");

        gate_a.open();
        gate_b.open();
        let _ = wait_completed(&mut submitter, &first);
        let _ = wait_completed(&mut submitter, &second);
        // Слот вернулся: новая работа принимается, а токен старой остаётся «завершён».
        let fourth = submitter.submit(Vec::new(), fill_commands(&context, &buffer)).unwrap();
        let proof = wait_completed(&mut submitter, &fourth);
        assert!(filled(&seal_after(buffer, &fourth, &proof)));
        assert!(matches!(submitter.poll(&first), WorkStatus::Completed(_)));
    }
    assert_eq!(context.live_objects(), baseline);
    assert_clean(&context);
}

#[test]
fn drain_times_out_while_gate_is_closed_then_drains_after_opening() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let mut submitter = OneShotSubmitter::new(&context, 2).unwrap();
        // Пустая очередь сливается мгновенно.
        assert_eq!(
            submitter.drain(Duration::from_millis(10)).unwrap(),
            DrainOutcome::Drained
        );
        let gate = Gate::new(&context);
        let work = submit_gated(&context, &mut submitter, &gate, None);
        let started = Instant::now();
        let outcome = submitter.drain(Duration::from_millis(100)).unwrap();
        assert_eq!(outcome, DrainOutcome::TimedOut { still_in_flight: 1 });
        assert!(
            started.elapsed() >= Duration::from_millis(90) && started.elapsed() < Duration::from_secs(5),
            "ожидание ограничено таймаутом, а не бесконечно: {:?}",
            started.elapsed()
        );
        assert!(matches!(submitter.poll(&work), WorkStatus::Pending));
        gate.open();
        assert_eq!(submitter.drain(EVENTUALLY).unwrap(), DrainOutcome::Drained);
        assert_eq!(submitter.in_flight_count(), 0);
        assert!(matches!(submitter.poll(&work), WorkStatus::Completed(_)));
    }
    assert_eq!(context.live_objects(), baseline);
    assert_clean(&context);
}

#[test]
fn record_error_is_reported_without_occupying_a_slot_and_queue_stays_usable() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut submitter = OneShotSubmitter::new(&context, 1).unwrap();
    let failed = submitter.submit(Vec::new(), |_| Err::<(), &str>("запись сорвалась"));
    assert!(
        matches!(failed, Err(SubmitError::Record("запись сорвалась"))),
        "{failed:?}"
    );
    assert_eq!(submitter.in_flight_count(), 0, "сорванная запись не занимает слот");
    // Единственный слот свободен: следующая работа принимается и завершается.
    let buffer = HostReadbackBuffer::new(&context, BUFFER_BYTES).unwrap();
    let work = submitter.submit(Vec::new(), fill_commands(&context, &buffer)).unwrap();
    let proof = wait_completed(&mut submitter, &work);
    assert!(filled(&seal_after(buffer, &work, &proof)));
    drop(submitter);
    assert_clean(&context);
}

#[test]
fn token_of_another_queue_is_foreign_and_does_not_get_a_proof() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut first = OneShotSubmitter::new(&context, 1).unwrap();
    let mut second = OneShotSubmitter::new(&context, 1).unwrap();
    let work = first.submit(Vec::new(), |_| Ok::<(), GpuError>(())).unwrap();
    assert!(matches!(
        second.poll(&work),
        WorkStatus::Failed(WorkFailure::ForeignWork)
    ));
    // Свой опрос при этом работает: чужой опрос ничего не испортил.
    let _ = wait_completed(&mut first, &work);
    drop((first, second));
    assert_clean(&context);
}

#[test]
fn imported_fence_semaphore_is_owned_by_the_queue_until_the_slot_is_reused() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        // Настоящий sync_fd: сигнал без ожиданий (экспорт у RADV неблокирующий, пока сигнал материализован).
        let producer = ExportableSemaphore::new(&context).unwrap();
        let mut producing = QueueSubmission::new()
            .signal_for_export(&producer, vk::PipelineStageFlags2::ALL_COMMANDS)
            .submit(&context)
            .unwrap();
        let release = producing.take_release_signal().unwrap().export().unwrap();
        let fence = match release.into_owned_fd() {
            Some(fd) => GpuWaitFence::from_sync_fd(fd),
            None => GpuWaitFence::already_signaled(),
        };
        let imported = import_wait_fence(&context, fence).unwrap();
        let imported_is_real = matches!(imported, WaitSemaphore::Imported(_));

        let mut submitter = OneShotSubmitter::new(&context, 1).unwrap();
        let buffer = HostReadbackBuffer::new(&context, BUFFER_BYTES).unwrap();
        let with_import = context.live_objects().count(LiveObjectKind::Semaphore);
        let work = submitter
            .submit(vec![imported], fill_commands(&context, &buffer))
            .unwrap();
        let proof = wait_completed(&mut submitter, &work);
        assert!(
            filled(&seal_after(buffer, &work, &proof)),
            "работа с импортированным ожиданием выполнилась"
        );
        let after_completion = context.live_objects().count(LiveObjectKind::Semaphore);
        if imported_is_real {
            assert_eq!(
                after_completion, with_import,
                "очередь держит импортированный семафор до переиспользования слота"
            );
        }
        // Новая работа в тот же слот: прежние ожидания отпущены.
        let second = submitter.submit(Vec::new(), |_| Ok::<(), GpuError>(())).unwrap();
        let _ = wait_completed(&mut submitter, &second);
        let after_reuse = context.live_objects().count(LiveObjectKind::Semaphore);
        if imported_is_real {
            assert_eq!(
                after_reuse + 1,
                after_completion,
                "семафор ожидания освобождён при переиспользовании слота"
            );
        }
    }
    assert_eq!(context.live_objects(), baseline);
    assert_clean(&context);
}

#[test]
fn semaphore_wait_orders_dependent_work_after_its_producer() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let mut producer_queue = OneShotSubmitter::new(&context, 1).unwrap();
        let mut consumer_queue = OneShotSubmitter::new(&context, 1).unwrap();
        let buffer = HostReadbackBuffer::new(&context, BUFFER_BYTES).unwrap();
        let semaphore = ExportableSemaphore::new(&context).unwrap();
        let waits = [semaphore.handle()];
        let signals = [semaphore.handle()];
        // Производитель пишет 0x11 БЕЗ барьера к последующим командам и сигналит семафор.
        let produced = producer_queue
            .submit_with_sync(
                Vec::new(),
                &SemaphoreSync {
                    signals: &signals,
                    ..SemaphoreSync::default()
                },
                |command_buffer| {
                    // SAFETY: командный буфер в состоянии записи; буфер имеет usage TRANSFER_DST.
                    unsafe {
                        context.device().cmd_fill_buffer(
                            command_buffer,
                            buffer.buffer_handle(),
                            0,
                            BUFFER_BYTES,
                            0x1111_1111,
                        );
                    }
                    Ok::<(), GpuError>(())
                },
            )
            .unwrap();
        // Потребитель ждёт семафор и перезаписывает буфер: без зависимости между сабмитами синхро-validation
        // сообщила бы WAW-гонку, а итоговое значение было бы не гарантировано.
        let consumed = consumer_queue
            .submit_with_sync(
                Vec::new(),
                &SemaphoreSync {
                    waits: &waits,
                    ..SemaphoreSync::default()
                },
                fill_commands(&context, &buffer),
            )
            .unwrap();
        let _ = wait_completed(&mut producer_queue, &produced);
        let proof = wait_completed(&mut consumer_queue, &consumed);
        assert!(
            filled(&seal_after(buffer, &consumed, &proof)),
            "последней записала работа, ждавшая семафор"
        );
    }
    assert_eq!(context.live_objects(), baseline);
    assert_clean(&context);
}
