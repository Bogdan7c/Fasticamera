//! Ячейка видоискателя под замком хаба: очередь кадров и действующая политика
//! (`LatestWins` или буфер с запасом — семантика в `crate::viewfinder_pacing`).
//!
//! Владелец — `HubState`. Ячейка сама решает, какой кадр отдать, вытеснить или отпустить, и сама
//! ведёт свои счётчики; возвраты камере только складывает в [`Deferred`] (они идут после замка хаба).
//!
//! # Учёт (точный баланс, проверяется тестами)
//! Каждая доля кадра, попавшая в ячейку, кончается ровно одним из: выдана рендеру
//! (`viewfinder_delivered`), вытеснена более новым или пришла старше выданного (`dropped_superseded`),
//! переполнила буфер (`dropped_jitter_overflow`), сброшена концом потока или сменой эпохи
//! (`viewfinder_flushed`), или лежит в ячейке сейчас (`viewfinder_queued`). Значит
//! `frames_published = delivered + superseded + overflow + flushed + queued`.

use std::collections::VecDeque;

use camera_contract::frame_passport::FrameSequence;

use super::deferred::Deferred;
use crate::counters::StreamAccounting;
use crate::lease::RawFrameLease;
use crate::viewfinder_pacing::{JitterBufferConfig, ViewfinderPacing, ViewfinderPacingSwitch};

/// Что ячейка ответила на опрос рендера (без учёта активности потока — её знает хаб).
pub(super) enum CellTake {
    /// Кадр для показа.
    Frame(RawFrameLease),
    /// Отдать нечего (пусто, ещё заполняется или запас исчерпан).
    Nothing,
}

pub(super) struct ViewfinderCell {
    pacing: ViewfinderPacing,
    jitter: JitterBufferConfig,
    /// Кадры по возрастанию `sequence` (старейший спереди). При `LatestWins` — не больше одного.
    queue: VecDeque<RawFrameLease>,
    /// Буфер набрал `reserve + 1` кадр и отдаёт кадры (для `LatestWins` не используется).
    primed: bool,
    /// Номер последнего выданного кадра: старше него не выдаётся ничего (показ не идёт назад).
    last_handed: Option<FrameSequence>,
}

impl ViewfinderCell {
    pub(super) fn new(pacing: ViewfinderPacing, jitter: JitterBufferConfig) -> Self {
        Self {
            pacing,
            jitter,
            queue: VecDeque::new(),
            primed: false,
            last_handed: None,
        }
    }

    /// Сколько кадров лежит в ячейке сейчас.
    pub(super) fn queued(&self) -> usize {
        self.queue.len()
    }

    pub(super) fn pacing(&self) -> ViewfinderPacing {
        self.pacing
    }

    /// Принимает долю нового кадра по действующей политике.
    pub(super) fn accept(&mut self, incoming: RawFrameLease, accounting: &StreamAccounting, deferred: &mut Deferred) {
        let sequence = incoming.passport().sequence;
        let older_than_shown = self.last_handed.is_some_and(|handed| sequence <= handed);
        let older_than_newest_in_latest_wins = self.pacing == ViewfinderPacing::LatestWins
            && self
                .queue
                .back()
                .is_some_and(|newest| newest.passport().sequence > sequence);
        if older_than_shown || older_than_newest_in_latest_wins {
            // Опоздавшая публикация: показ назад во времени запрещён, кадр «вытеснен» более новым.
            StreamAccounting::bump(&accounting.dropped_superseded);
            deferred.release_unused_later(incoming);
            self.publish_gauge(accounting);
            return;
        }
        self.insert_in_order(incoming);
        match self.pacing {
            ViewfinderPacing::LatestWins => self.keep_only_newest(accounting, deferred),
            ViewfinderPacing::JitterBuffer => self.trim_overflow(accounting, deferred),
        }
        self.publish_gauge(accounting);
    }

    /// Опрос рендера раз в vsync: самый свежий (`LatestWins`) или старейший непоказанный (буфер).
    /// `stream_active` — идёт ли поток: счётчики заполнения и повторов ведутся только для живого потока.
    pub(super) fn take(&mut self, stream_active: bool, accounting: &StreamAccounting) -> CellTake {
        let taken = match self.pacing {
            ViewfinderPacing::LatestWins => self.queue.pop_back(),
            ViewfinderPacing::JitterBuffer => self.take_from_jitter_buffer(stream_active, accounting),
        };
        self.publish_gauge(accounting);
        match taken {
            Some(lease) => {
                self.last_handed = Some(lease.passport().sequence);
                StreamAccounting::bump(&accounting.viewfinder_delivered);
                CellTake::Frame(lease)
            }
            None => CellTake::Nothing,
        }
    }

    /// Конец потока или новая эпоха: всё отпускается нетронутым, буфер заполняется заново.
    pub(super) fn flush(&mut self, accounting: &StreamAccounting, deferred: &mut Deferred) {
        for lease in self.queue.drain(..) {
            StreamAccounting::bump(&accounting.viewfinder_flushed);
            deferred.release_unused_later(lease);
        }
        self.primed = false;
        self.last_handed = None;
        self.publish_gauge(accounting);
    }

    /// Переключение политики на ходу (A/B на телефоне). Лизинги не теряются и не выдаются дважды:
    /// в `LatestWins` остаётся самый новый (прочие — вытеснены), в буфер переходит то, что было в ячейке.
    pub(super) fn switch_to(
        &mut self,
        pacing: ViewfinderPacing,
        accounting: &StreamAccounting,
        deferred: &mut Deferred,
    ) -> ViewfinderPacingSwitch {
        if pacing == self.pacing {
            return ViewfinderPacingSwitch::AlreadyActive;
        }
        self.pacing = pacing;
        // Новая политика начинает с заполнения (для `LatestWins` флаг не используется).
        self.primed = false;
        if pacing == ViewfinderPacing::LatestWins {
            self.keep_only_newest(accounting, deferred);
        }
        StreamAccounting::bump(&accounting.viewfinder_pacing_switches);
        self.publish_gauge(accounting);
        ViewfinderPacingSwitch::Switched
    }

    fn take_from_jitter_buffer(&mut self, stream_active: bool, accounting: &StreamAccounting) -> Option<RawFrameLease> {
        if !self.primed {
            let needed = usize::try_from(self.jitter.reserve)
                .unwrap_or(usize::MAX)
                .saturating_add(1);
            if self.queue.len() < needed {
                if stream_active {
                    StreamAccounting::bump(&accounting.viewfinder_polls_while_priming);
                }
                return None;
            }
            self.primed = true;
        }
        let taken = self.queue.pop_front();
        if taken.is_none() && stream_active {
            // Запас исчерпан: этот тик повторит прежний кадр. Пропущенный опрос сам поднимет
            // уровень очереди на кадр, поэтому заново ждать заполнения не нужно (см. модуль политики).
            StreamAccounting::bump(&accounting.viewfinder_underruns);
        }
        taken
    }

    /// Вставка по возрастанию `sequence` (публикация может идти не по порядку прихода).
    fn insert_in_order(&mut self, incoming: RawFrameLease) {
        let sequence = incoming.passport().sequence;
        let position = self
            .queue
            .iter()
            .rposition(|held| held.passport().sequence < sequence)
            .map_or(0, |index| index + 1);
        self.queue.insert(position, incoming);
    }

    /// `LatestWins`: всё, кроме самого нового, вытеснено.
    fn keep_only_newest(&mut self, accounting: &StreamAccounting, deferred: &mut Deferred) {
        while self.queue.len() > 1 {
            let Some(oldest) = self.queue.pop_front() else { break };
            StreamAccounting::bump(&accounting.dropped_superseded);
            deferred.release_unused_later(oldest);
        }
    }

    /// Буфер: сверх `max_queued` отпускается старейший (очередь не копит задержку).
    fn trim_overflow(&mut self, accounting: &StreamAccounting, deferred: &mut Deferred) {
        let capacity = usize::try_from(self.jitter.max_queued).unwrap_or(usize::MAX).max(1);
        while self.queue.len() > capacity {
            let Some(oldest) = self.queue.pop_front() else { break };
            StreamAccounting::bump(&accounting.dropped_jitter_overflow);
            deferred.release_unused_later(oldest);
        }
    }

    fn publish_gauge(&self, accounting: &StreamAccounting) {
        accounting.set_viewfinder_queued(self.queue.len());
    }
}
