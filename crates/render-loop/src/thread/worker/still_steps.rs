//! Шаги снимков, пока показа нет (сессия 14e, решение 150). Вынесено из `worker.rs` (размер модуля).
//!
//! С показом снимки двигаются после каждого показанного кадра (`render_frame`). Без показа (surface отпущена,
//! пауза Activity) кадров нет, и раньше поток спал на ящике без срока — захваченный снимок стоял до следующего
//! присоединения или до срока записи. Теперь, пока бэкенд отвечает «есть недоделанные снимки», поток ждёт ящик не
//! дольше `still.step_interval_without_display_ms` и после каждого пробуждения делает шаг снимков. Снимков нет —
//! сон без срока, как раньше: лишних пробуждений у простаивающего потока нет.
//!
//! Неисправный поток бэкенд не зовёт (`guarded`): его снимки получают итог сразу при неисправности
//! (`DisplayBackend::abandon_stills` в `enter_fault`), будить ради них поток незачем.

use std::time::Duration;

use super::super::frame_clock::FrameClock;
use super::RenderWorker;
use crate::backend::DisplayBackend;
use crate::still::StillWorkload;

impl<B: DisplayBackend, C: FrameClock> RenderWorker<B, C> {
    /// Сколько ждать ящик без показа: до следующего шага снимков, если они есть, иначе без срока (`None`).
    pub(super) fn wait_limit_without_presentation(&mut self) -> Option<Duration> {
        match self.unfinished_stills() {
            StillWorkload::Unfinished => Some(self.config.still.step_interval_without_display()),
            StillWorkload::Idle => None,
        }
    }

    /// Шаг снимков на итерации без показа: опрос GPU, следующие полосы, итоговые события. Ничего не ждёт.
    pub(super) fn advance_stills_without_presentation(&mut self) {
        if self.unfinished_stills() == StillWorkload::Idle {
            return;
        }
        self.guarded(DisplayBackend::advance_stills);
        // Счётчики снимков в статусе (smoke, диагностика) обновляются и без кадров — по обычному расписанию опроса.
        self.poll_diagnostics_if_due();
    }

    /// Ответ бэкенда о недоделанных снимках; неисправный поток бэкенд не спрашивает — снимков у него нет.
    fn unfinished_stills(&mut self) -> StillWorkload {
        self.guarded(|backend| backend.still_workload())
            .unwrap_or(StillWorkload::Idle)
    }
}
