//! Учёт лизингов кадров камеры: какой кадр сейчас на экране, какие ждут возврата и когда их
//! можно вернуть камере (инвариант 3 плана: буфер возвращается только по fence сабмита, который
//! перестал его читать).
//!
//! Модуль нейтральный и не знает про Vulkan: лизинг абстрагирован трейтом [`CameraLease`], поэтому
//! все правила «что и когда отпускать» проверяются на ПК с поддельными лизингами.
//!
//! # Правила
//! * кадр, который читал ПОСЛЕДНИЙ принятый сабмит, называется *показанным* (`displayed`); его
//!   нельзя возвращать: GPU может читать его снова (перерисовка) или ещё дочитывать;
//! * когда сабмит показал другой кадр (или чёрный), прежний показанный кадр переходит в *уходящие*
//!   (`retiring`). Его последний читавший сабмит — один из ПРЕДЫДУЩИХ, поэтому fence ТЕКУЩЕГО
//!   сабмита (он покрывает все более ранние сабмиты очереди, см. `PresentSync`) — безопасный сигнал;
//! * кадр, который ISP видоискателя читает ОДИН раз (сессия 07), *поглощается* сабмитом
//!   ([`FrameChange::Consume`]): этот же сабмит его принимает, обрабатывает и возвращает камере, на
//!   экране остаётся изображение видоискателя, а не буфер камеры. Поэтому лизинг сразу уходящий, и
//!   fence ЭТОГО сабмита (сигнал после всех его команд, включая барьер возврата) — безопасный сигнал;
//! * без fence лизинг возвращается только когда GPU подтверждённо простаивает
//!   ([`LeaseLedger::release_all_after_gpu_idle`]); иначе его можно лишь выбросить — это учтённая
//!   утечка (решение 43), а не тихий возврат.

use std::io;

use gpu_vulkan::hardware_buffer_cache::SubmissionSerial;
use raw_frame_stream::RawFrameLease;
use shell_contract::sync_fence::GpuReleaseFence;

/// Лизинг кадра камеры с точки зрения учёта: вернуть по fence или как нетронутый.
pub trait CameraLease {
    /// Вернуть после работы GPU: буфер уйдёт камере по сигналу `fence`.
    fn release_after_gpu_fence(self, fence: GpuReleaseFence);
    /// Вернуть кадр, которого GPU не касался.
    fn release_unused(self);
}

impl CameraLease for RawFrameLease {
    fn release_after_gpu_fence(self, fence: GpuReleaseFence) {
        RawFrameLease::release_after_gpu_fence(self, fence);
    }

    fn release_unused(self) {
        RawFrameLease::release_unused(self);
    }
}

/// Что сабмит делает с показанным кадром.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameChange {
    /// Тот же кадр рисуется снова (нового кадра нет) — возвращать нечего.
    Keep,
    /// Сабмит показывает другой кадр камеры.
    Replace,
    /// Сабмит показывает чёрный (потока нет) или изображение видоискателя без буфера камеры:
    /// показанного кадра больше нет.
    Clear,
    /// Сабмит читает новый кадр ОДИН раз и в нём же возвращает его камере (ISP видоискателя):
    /// новый кадр сразу уходящий, прежний показанный (если был) — тоже.
    Consume,
}

/// Лизинг, ушедший с экрана.
struct Retiring<Lease> {
    lease: Lease,
    /// Серия сабмита, который заменил кадр: в нём записан возврат владения камере, и он же — последний
    /// сабмит, после которого кадр не читается. Когда завершены все сабмиты до этой серии включительно,
    /// GPU точно закончил с буфером (без всякого fence).
    released_by: SubmissionSerial,
}

/// Учёт лизингов одной сессии показа.
pub struct LeaseLedger<Lease: CameraLease> {
    /// Кадр, который читал последний принятый сабмит.
    displayed: Option<Lease>,
    /// Кадры, ушедшие с экрана и ожидающие fence более позднего сабмита (или завершения своей серии).
    retiring: Vec<Retiring<Lease>>,
}

impl<Lease: CameraLease> Default for LeaseLedger<Lease> {
    fn default() -> Self {
        Self {
            displayed: None,
            retiring: Vec::new(),
        }
    }
}

impl<Lease: CameraLease> LeaseLedger<Lease> {
    /// Есть ли показанный кадр (его можно перерисовать).
    pub fn displayed(&self) -> Option<&Lease> {
        self.displayed.as_ref()
    }

    /// Нужен ли сабмиту экспорт release-fence: есть ли кто-то, кого этот сабмит делает «уходящим»
    /// (прежний показанный кадр или поглощаемый новый) или кто уже ждёт fence.
    pub fn needs_release_fence(&self, change: FrameChange) -> bool {
        !self.retiring.is_empty()
            || change == FrameChange::Consume
            || (self.displayed.is_some() && change != FrameChange::Keep)
    }

    /// Сколько лизингов ждут fence.
    #[cfg(test)]
    pub fn retiring_count(&self) -> usize {
        self.retiring.len()
    }

    /// Нет ни показанного, ни уходящих лизингов.
    pub fn is_empty(&self) -> bool {
        self.displayed.is_none() && self.retiring.is_empty()
    }

    /// Сабмит серии `serial` ПРИНЯТ очередью: фиксируем смену показанного кадра. `new_frame` — кадр, который
    /// он читал (`Some` только при [`FrameChange::Replace`]).
    pub fn commit_submitted(&mut self, change: FrameChange, new_frame: Option<Lease>, serial: SubmissionSerial) {
        match change {
            FrameChange::Keep => {
                // Нового кадра быть не может: если он пришёл, его судьба — забота вызывающего.
                debug_assert!(new_frame.is_none(), "Keep не принимает новый кадр");
                if let Some(unexpected) = new_frame {
                    // Сабмит нового кадра не читал, GPU его не касался.
                    unexpected.release_unused();
                }
            }
            FrameChange::Replace | FrameChange::Clear => {
                self.retire_displayed(serial);
                self.displayed = new_frame;
            }
            FrameChange::Consume => {
                // Возврат и прежнего, и нового кадра записан в ЭТОМ сабмите: оба ждут его fence.
                self.retire_displayed(serial);
                if let Some(consumed) = new_frame {
                    self.retiring.push(Retiring {
                        lease: consumed,
                        released_by: serial,
                    });
                }
            }
        }
    }

    /// Показанный кадр уходит с экрана: его возврат записан в сабмите `serial`.
    fn retire_displayed(&mut self, serial: SubmissionSerial) {
        if let Some(previous) = self.displayed.take() {
            self.retiring.push(Retiring {
                lease: previous,
                released_by: serial,
            });
        }
    }

    /// Возвращает уходящие кадры по fence принятого сабмита. Каждому достаётся своя копия дескриптора.
    /// Кадры, для которых копию сделать не удалось, остаются ждать следующего случая; возвращается их число.
    pub fn release_retiring_after_fence(&mut self, fence: GpuReleaseFence) -> usize {
        let mut waiting = std::mem::take(&mut self.retiring);
        // Последнему кадру отдаём оригинал, остальным — дубликаты.
        let Some(last) = waiting.pop() else { return 0 };
        let mut not_released = Vec::new();
        for entry in waiting {
            match duplicate_fence(&fence) {
                Ok(copy) => entry.lease.release_after_gpu_fence(copy),
                Err(error) => {
                    tracing::warn!(
                        target: "fasticamera::render::camera",
                        %error,
                        "не удалось продублировать release-fence: лизинг ждёт следующего случая"
                    );
                    not_released.push(entry);
                }
            }
        }
        last.lease.release_after_gpu_fence(fence);
        let waiting_count = not_released.len();
        self.retiring = not_released;
        waiting_count
    }

    /// Завершены все сабмиты до серии `completed` включительно: уходящие лизинги, чья серия возврата уже
    /// завершена, возвращаются с сигнальным fence — GPU с ними точно закончил (барьер возврата
    /// владения был в сабмите `released_by`). Не ждёт GPU и не трогает показанный кадр.
    /// Возвращает сколько лизингов отдано.
    pub fn release_retiring_completed(&mut self, completed: SubmissionSerial) -> usize {
        let (done, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.retiring)
            .into_iter()
            .partition(|entry| entry.released_by.0 <= completed.0);
        self.retiring = waiting;
        let count = done.len();
        for entry in done {
            entry.lease.release_after_gpu_fence(GpuReleaseFence::already_signaled());
        }
        count
    }

    /// GPU подтверждённо простаивает: все лизинги (показанный и уходящие) возвращаются с сигнальным
    /// fence. Возвращает сколько лизингов отдано.
    pub fn release_all_after_gpu_idle(&mut self) -> usize {
        let mut count = 0;
        let retiring = self.retiring.drain(..).map(|entry| entry.lease);
        for lease in self.displayed.take().into_iter().chain(retiring) {
            lease.release_after_gpu_fence(GpuReleaseFence::already_signaled());
            count += 1;
        }
        count
    }

    /// Состояние GPU неизвестно (завис), fence нет: лизинги выбрасываются — НАМЕРЕННАЯ учтённая утечка
    /// (`leaked_leases` у потока, решение 43). Возвращает сколько лизингов выброшено.
    pub fn leak_all(&mut self) -> usize {
        let count = self.retiring.len() + usize::from(self.displayed.is_some());
        drop(self.displayed.take());
        self.retiring.clear();
        count
    }
}

/// Копия дескриптора fence: sync_fd — обычный файловый дескриптор, `dup` даёт независимого «владельца»
/// того же сигнала.
fn duplicate_fence(fence: &GpuReleaseFence) -> io::Result<GpuReleaseFence> {
    match fence.as_sync_fd() {
        None => Ok(GpuReleaseFence::already_signaled()),
        Some(descriptor) => Ok(GpuReleaseFence::from_sync_fd(descriptor.try_clone_to_owned()?)),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    /// Как лизинг был возвращён.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Returned {
        AfterRealFence,
        AfterSignaledFence,
        Unused,
    }

    type Journal = Rc<RefCell<Vec<(u32, Returned)>>>;

    struct FakeLease {
        id: u32,
        journal: Journal,
    }

    impl CameraLease for FakeLease {
        fn release_after_gpu_fence(self, fence: GpuReleaseFence) {
            let how = if fence.is_already_signaled() {
                Returned::AfterSignaledFence
            } else {
                Returned::AfterRealFence
            };
            self.journal.borrow_mut().push((self.id, how));
        }

        fn release_unused(self) {
            self.journal.borrow_mut().push((self.id, Returned::Unused));
        }
    }

    fn lease(journal: &Journal, id: u32) -> FakeLease {
        FakeLease {
            id,
            journal: Rc::clone(journal),
        }
    }

    /// Настоящий (несигнальный) fence: один конец сокета.
    fn real_fence() -> GpuReleaseFence {
        let (fence_end, _signal_end) = std::os::unix::net::UnixStream::pair().expect("пара сокетов");
        GpuReleaseFence::from_sync_fd(fence_end.into())
    }

    #[test]
    fn replaced_frame_is_returned_with_the_fence_of_the_submit_that_replaced_it() {
        let journal = Journal::default();
        let mut ledger = LeaseLedger::default();
        ledger.commit_submitted(FrameChange::Replace, Some(lease(&journal, 1)), SubmissionSerial(1));
        assert!(journal.borrow().is_empty(), "показанный кадр не возвращается");

        assert!(ledger.needs_release_fence(FrameChange::Replace));
        ledger.commit_submitted(FrameChange::Replace, Some(lease(&journal, 2)), SubmissionSerial(2));
        assert!(journal.borrow().is_empty(), "до экспорта fence кадр 1 ещё ждёт");
        assert_eq!(ledger.retiring_count(), 1);

        assert_eq!(ledger.release_retiring_after_fence(real_fence()), 0);
        assert_eq!(*journal.borrow(), vec![(1, Returned::AfterRealFence)]);
        assert_eq!(
            ledger.displayed().map(|current| current.id),
            Some(2),
            "кадр 2 всё ещё на экране"
        );
    }

    #[test]
    fn redraw_of_the_same_frame_exports_nothing_and_returns_nothing() {
        let journal = Journal::default();
        let mut ledger = LeaseLedger::default();
        ledger.commit_submitted(FrameChange::Replace, Some(lease(&journal, 1)), SubmissionSerial(1));
        assert!(!ledger.needs_release_fence(FrameChange::Keep));
        ledger.commit_submitted(FrameChange::Keep, None, SubmissionSerial(9));
        assert!(journal.borrow().is_empty());
        assert_eq!(ledger.displayed().map(|current| current.id), Some(1));
    }

    #[test]
    fn first_frame_needs_no_fence_but_clearing_the_screen_does() {
        let journal = Journal::default();
        let mut ledger: LeaseLedger<FakeLease> = LeaseLedger::default();
        assert!(!ledger.needs_release_fence(FrameChange::Replace), "возвращать некого");
        assert!(!ledger.needs_release_fence(FrameChange::Clear));
        ledger.commit_submitted(FrameChange::Replace, Some(lease(&journal, 1)), SubmissionSerial(1));
        assert!(ledger.needs_release_fence(FrameChange::Clear));
        ledger.commit_submitted(FrameChange::Clear, None, SubmissionSerial(2));
        assert!(ledger.displayed().is_none(), "чёрный кадр: показанного нет");
        assert_eq!(ledger.release_retiring_after_fence(real_fence()), 0);
        assert_eq!(*journal.borrow(), vec![(1, Returned::AfterRealFence)]);
        assert!(ledger.is_empty());
    }

    #[test]
    fn leases_waiting_for_a_failed_export_all_get_a_fence_from_the_next_one() {
        let journal = Journal::default();
        let mut ledger = LeaseLedger::default();
        for id in 1..=3 {
            ledger.commit_submitted(
                FrameChange::Replace,
                Some(lease(&journal, id)),
                SubmissionSerial(u64::from(id)),
            );
        }
        // Экспорт после смен 1->2 и 2->3 не состоялся: кадры 1 и 2 ждут.
        assert_eq!(ledger.retiring_count(), 2);
        assert!(
            ledger.needs_release_fence(FrameChange::Keep),
            "ждущие кадры требуют fence и без смены"
        );
        assert_eq!(ledger.release_retiring_after_fence(real_fence()), 0);
        let mut returned = journal.borrow().clone();
        returned.sort_by_key(|(id, _)| *id);
        assert_eq!(
            returned,
            vec![(1, Returned::AfterRealFence), (2, Returned::AfterRealFence)],
            "оба получили настоящий fence; кадр 3 на экране"
        );
        assert_eq!(ledger.displayed().map(|current| current.id), Some(3));
    }

    #[test]
    fn gpu_idle_returns_every_lease_with_a_signaled_fence_and_never_as_unused() {
        let journal = Journal::default();
        let mut ledger = LeaseLedger::default();
        for id in 1..=3 {
            ledger.commit_submitted(
                FrameChange::Replace,
                Some(lease(&journal, id)),
                SubmissionSerial(u64::from(id)),
            );
        }
        assert_eq!(ledger.release_all_after_gpu_idle(), 3);
        let mut returned = journal.borrow().clone();
        returned.sort_by_key(|(id, _)| *id);
        assert_eq!(
            returned,
            vec![
                (1, Returned::AfterSignaledFence),
                (2, Returned::AfterSignaledFence),
                (3, Returned::AfterSignaledFence),
            ]
        );
        assert!(ledger.is_empty());
    }

    #[test]
    fn completed_release_of_retiring_keeps_the_displayed_frame() {
        let journal = Journal::default();
        let mut ledger = LeaseLedger::default();
        for id in 1..=2 {
            ledger.commit_submitted(
                FrameChange::Replace,
                Some(lease(&journal, id)),
                SubmissionSerial(u64::from(id)),
            );
        }
        assert_eq!(ledger.release_retiring_completed(SubmissionSerial(2)), 1);
        assert_eq!(*journal.borrow(), vec![(1, Returned::AfterSignaledFence)]);
        assert_eq!(
            ledger.displayed().map(|current| current.id),
            Some(2),
            "показанный кадр остался"
        );
    }

    #[test]
    fn retiring_lease_is_released_only_once_its_replacing_submit_is_completed() {
        let journal = Journal::default();
        let mut ledger = LeaseLedger::default();
        // Кадр 1 заменён сабмитом серии 5 (кадр 2), кадр 2 — сабмитом серии 8 (кадр 3).
        ledger.commit_submitted(FrameChange::Replace, Some(lease(&journal, 1)), SubmissionSerial(4));
        ledger.commit_submitted(FrameChange::Replace, Some(lease(&journal, 2)), SubmissionSerial(5));
        ledger.commit_submitted(FrameChange::Replace, Some(lease(&journal, 3)), SubmissionSerial(8));
        assert_eq!(
            ledger.release_retiring_completed(SubmissionSerial(4)),
            0,
            "серия 5 ещё не завершена"
        );
        assert!(journal.borrow().is_empty());
        assert_eq!(ledger.release_retiring_completed(SubmissionSerial(5)), 1);
        assert_eq!(*journal.borrow(), vec![(1, Returned::AfterSignaledFence)]);
        assert_eq!(
            ledger.release_retiring_completed(SubmissionSerial(7)),
            0,
            "кадр 2 ждёт серию 8"
        );
        assert_eq!(ledger.release_retiring_completed(SubmissionSerial(8)), 1);
        assert_eq!(journal.borrow().len(), 2);
        assert_eq!(ledger.displayed().map(|current| current.id), Some(3));
    }

    #[test]
    fn leak_drops_leases_without_returning_them() {
        let journal = Journal::default();
        let mut ledger = LeaseLedger::default();
        ledger.commit_submitted(FrameChange::Replace, Some(lease(&journal, 1)), SubmissionSerial(1));
        ledger.commit_submitted(FrameChange::Replace, Some(lease(&journal, 2)), SubmissionSerial(2));
        assert_eq!(ledger.leak_all(), 2);
        assert!(
            journal.borrow().is_empty(),
            "утечка — не возврат: журнал возвратов пуст"
        );
        assert!(ledger.is_empty());
    }

    #[test]
    fn consumed_frame_is_returned_with_the_fence_of_its_own_submit() {
        let journal = Journal::default();
        let mut ledger = LeaseLedger::default();
        assert!(
            ledger.needs_release_fence(FrameChange::Consume),
            "поглощаемый кадр требует fence даже самым первым"
        );
        ledger.commit_submitted(FrameChange::Consume, Some(lease(&journal, 1)), SubmissionSerial(1));
        assert!(
            ledger.displayed().is_none(),
            "на экране изображение видоискателя, а не буфер камеры"
        );
        assert_eq!(ledger.retiring_count(), 1);
        assert_eq!(ledger.release_retiring_after_fence(real_fence()), 0);
        assert_eq!(
            *journal.borrow(),
            vec![(1, Returned::AfterRealFence)],
            "кадр отдан fence-ом своего же сабмита, следующего сабмита не ждём"
        );
        assert!(ledger.is_empty());
    }

    #[test]
    fn consume_after_debug_mosaic_retires_the_displayed_frame_in_the_same_submit() {
        let journal = Journal::default();
        let mut ledger = LeaseLedger::default();
        // Отладочная мозаика показывала кадр 1, затем переключились на ISP: кадр 2 поглощён.
        ledger.commit_submitted(FrameChange::Replace, Some(lease(&journal, 1)), SubmissionSerial(1));
        ledger.commit_submitted(FrameChange::Consume, Some(lease(&journal, 2)), SubmissionSerial(2));
        assert_eq!(ledger.retiring_count(), 2);
        assert_eq!(
            ledger.release_retiring_completed(SubmissionSerial(1)),
            0,
            "серия 2 ещё идёт"
        );
        assert_eq!(ledger.release_retiring_completed(SubmissionSerial(2)), 2);
        let mut returned = journal.borrow().clone();
        returned.sort_by_key(|(id, _)| *id);
        assert_eq!(
            returned,
            vec![(1, Returned::AfterSignaledFence), (2, Returned::AfterSignaledFence)]
        );
        assert!(ledger.is_empty());
    }
}
