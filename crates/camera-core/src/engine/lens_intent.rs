//! Объективы в движке: чтение намерений экрана, смена цели камеры и возврат при провале.
//!
//! # Кто владеет чем
//! * шкала зума ([`ZoomCatalog`]) — строится при загрузке каталога камер (`zoom`);
//! * выбор камеры по коэффициенту — чистая функция [`select_lens`] (гистерезис только у плавных
//!   жестов, явные выборы идут без него);
//! * учёт смены (`Steady/Switching`, блокировка, счётчики) — [`LensSwitchTracker`] (`lens_switch`);
//! * **цель камеры** по-прежнему одна — `Desired::target`, перестройкой потока владеет
//!   `reconcile` (флаг `reconfiguring`). Здесь второго владельца нет: намерение лишь меняет цель
//!   так же, как команда `SwitchTo`.
//!
//! # Приоритет команд и намерений
//! И команды (`Open`, `SwitchTo`), и намерения меняют `Desired::target`; **побеждает та, что
//! пришла позже**. Намерение читается только если его поколение новее уже прочитанного: старое
//! намерение никогда не перебьёт более позднюю команду (толчок без нового поколения, в том
//! числе досланный при привязке, ничего не делает).
//!
//! # Что намерение не делает
//! Смена коэффициента **внутри** одного объектива цель не меняет и бэкенд не трогает: кадрирование
//! считают экран и рендер. Намерение до первой `Open` (цели ещё нет) только запоминается: движок
//! сам камеру не открывает, это делает команда.

use std::sync::Arc;
use std::time::Instant;

use camera_contract::ids::{CameraId, CameraTarget, LensFacing, LensRole};
use camera_contract::zoom::{LensIntent, LensIntentChannel, ZoomCatalog, ZoomGesture};

use super::lens_switch::{LensSwitchTracker, RetargetCause, Revert};
use super::{CameraEngine, Halt, LOG_TARGET};
use crate::command::{CommandOutcome, InvalidStateReason};
use crate::lens_status::{ActiveLens, LensStatus, LensSwitchFailure};
use crate::zoom::{select_initial_lens, select_lens};
use camera_contract::three_a::DirectiveGeneration;

/// Всё, чем движок владеет в части объективов, кроме самой цели камеры.
pub(super) struct LensEngineState {
    /// Ящик намерений экрана; `None` — экран к движку не подключён.
    intents: Option<LensIntentChannel>,
    /// Поколение последнего прочитанного намерения.
    seen: Option<DirectiveGeneration>,
    last_intent: Option<LensIntent>,
    /// Последний задний объектив, на котором стояла цель: опора гистерезиса, когда экран
    /// возвращается с фронтальной камеры (иначе выбор считался бы «с нуля» и мигал на границе).
    last_back_role: Option<LensRole>,
    /// Последний задний объектив, который **реально отдал кадр**: на него откатывается
    /// `last_back_role`, если провалилась смена с фронтальной камеры на задний объектив.
    last_streamed_back_role: Option<LensRole>,
    zoom_catalog: Arc<ZoomCatalog>,
    tracker: LensSwitchTracker,
    /// После возврата цели нужно довести реальное состояние (делает хвост `reconcile`).
    revert_reconcile_pending: bool,
}

impl LensEngineState {
    pub(super) fn new() -> Self {
        Self {
            intents: None,
            seen: None,
            last_intent: None,
            last_back_role: None,
            last_streamed_back_role: None,
            zoom_catalog: Arc::new(ZoomCatalog::default()),
            tracker: LensSwitchTracker::new(),
            revert_reconcile_pending: false,
        }
    }

    pub(super) fn attach_intents(&mut self, intents: LensIntentChannel) {
        self.intents = Some(intents);
    }

    pub(super) fn set_zoom_catalog(&mut self, zoom_catalog: Arc<ZoomCatalog>) {
        self.zoom_catalog = zoom_catalog;
    }
}

impl CameraEngine {
    /// Прочитать ящик намерений и, если выбранный объектив изменился, сменить цель камеры.
    ///
    /// Зовёт поток камеры по толчку экрана ([`crate::LensIntentWaker`]) и после загрузки каталога.
    /// Намерения нет, оно старое или каталог не прочитан — ничего не делает.
    pub fn refresh_lens_intent(&mut self, now: Instant) {
        if self.poisoned || !self.catalog_loaded || self.is_fatal() {
            return;
        }
        let Some((generation, intent)) = self
            .lens
            .intents
            .as_ref()
            .and_then(|channel| channel.newer_than(self.lens.seen))
        else {
            return;
        };
        self.lens.seen = Some(generation);
        self.lens.last_intent = Some(intent);
        let Some(role) = self.role_for_intent(intent) else {
            tracing::warn!(target: LOG_TARGET, ?intent, "для намерения нет камеры направления");
            self.lens.tracker.note_intent_ignored();
            return;
        };
        let role_is_blocked = self.lens.tracker.blocked().is_some_and(|block| block.role == role);
        if role_is_blocked && intent.gesture == ZoomGesture::Continuous {
            // Смена на эту роль только что провалилась, а колесо продолжает публиковать
            // коэффициенты в её диапазоне: не пробуем снова, пока жест не уведёт к другой роли.
            return;
        }
        // Явный выбор (`Discrete`) заблокированной роли — прямая просьба пользователя: блок
        // снимается и пробуется одна попытка. Провал вернёт блок и снова даст `LensSwitchFailed`,
        // по которому экран откатится. Намерение другой роли блок тоже снимает.
        self.lens.tracker.clear_block();
        match self.decide_switch_to(role, RetargetCause::Intent, now) {
            CommandOutcome::Accepted => {
                // Как после `SwitchTo`: новая камера отменяет ожидание восстановления прежней.
                self.recovery.clear();
                self.reconcile(now);
                self.three_a.publish_white_balance(&self.desired.three_a);
                self.three_a.publish_exposure_intent(&self.desired.three_a);
            }
            // Объектив тот же: меняется только кадрирование, бэкенд не трогаем.
            CommandOutcome::AlreadyInState => {}
            // Камера ещё не запрошена командой: намерение запомнено, открывать нечего.
            CommandOutcome::InvalidState(InvalidStateReason::NoCameraRequested) => {}
            rejected => {
                tracing::warn!(target: LOG_TARGET, ?intent, ?rejected, "намерение объектива не выполнено");
                self.lens.tracker.note_intent_ignored();
            }
        }
    }

    /// Роль, которую выбирает намерение, или `None`, если для направления нет камер.
    fn role_for_intent(&self, intent: LensIntent) -> Option<LensRole> {
        let group = self.lens.zoom_catalog.group(intent.facing)?;
        match (intent.facing, self.current_back_role()) {
            (LensFacing::Back, Some(current)) => {
                select_lens(group, intent.ratio, current, intent.gesture, &self.config.zoom)
            }
            _ => select_initial_lens(group, intent.ratio),
        }
    }

    /// Задний объектив, от которого считается гистерезис: желаемая камера, если она задняя,
    /// иначе последний задний объектив.
    fn current_back_role(&self) -> Option<LensRole> {
        let from_target = self.desired_camera_id().and_then(|camera| {
            self.lens
                .zoom_catalog
                .back
                .as_ref()?
                .lens_for_camera(&camera)
                .map(|lens| lens.role)
        });
        from_target.or(self.lens.last_back_role)
    }

    /// Цель камеры поменялась на `camera`: учесть смену объектива. Зовут `Open`, `SwitchTo` и
    /// намерение сразу после записи цели в `Desired`.
    pub(super) fn note_lens_target_changed(&mut self, camera: &CameraId, cause: RetargetCause, now: Instant) {
        let Some(role) = self.catalog.role_of(camera) else {
            self.lens.tracker.on_target_without_role();
            return;
        };
        if role != LensRole::Front {
            self.lens.last_back_role = Some(role);
        }
        if let Some(event) = self.lens.tracker.on_target_changed(role, cause, now) {
            self.events.push(event);
        }
    }

    /// Пришёл кадр повторяющегося запроса текущей сессии.
    pub(super) fn note_lens_frame(&mut self, now: Instant) {
        let Some(session) = &self.session else {
            return;
        };
        let Some(role) = self.catalog.role_of(&session.configured.stream.camera) else {
            return;
        };
        if role != LensRole::Front {
            self.lens.last_streamed_back_role = Some(role);
        }
        if let Some(event) = self.lens.tracker.on_frame(role, now) {
            self.events.push(event);
        }
    }

    /// Показывать нечего (камера закрыта, поток остановлен, окончательная ошибка).
    pub(super) fn reset_lens_switch(&mut self) {
        self.lens.tracker.reset();
    }

    /// Новая камера отказала во время смены: вернуть цель на прежний объектив.
    ///
    /// `None` — возвращаться не к чему (смены нет, либо уже идёт возврат): отказ идёт обычным
    /// путём. `Some(Halt)` — цель возвращена, устройство отпущено, доведение продолжит хвост
    /// `reconcile` ([`CameraEngine::run_pending_lens_revert`]). Само восстановление (решение 39)
    /// при этом не запускается: попытки не тратятся, `Unavailable` не объявляется — камера `from`
    /// исправна, а причина отказа уже в событии `LensSwitchFailed`.
    pub(super) fn try_revert_lens_switch(&mut self, reason: LensSwitchFailure) -> Option<Halt> {
        let revert = self.lens.tracker.revert_candidate()?;
        Some(self.revert_lens_switch(revert, reason))
    }

    fn revert_lens_switch(&mut self, revert: Revert, reason: LensSwitchFailure) -> Halt {
        tracing::warn!(
            target: LOG_TARGET,
            from = revert.from.name(),
            to = revert.to.name(),
            ?reason,
            "смена объектива не удалась: возврат на прежний"
        );
        self.release_platform();
        let event = self.lens.tracker.on_switch_failed(revert, reason);
        self.events.push(event);
        self.desired.target = Some(CameraTarget::Role(revert.from));
        // Опора гистерезиса откатывается вместе с целью: провалившийся `to` не должен остаться
        // «последним задним объективом». С фронтальной `from` берём последний задний, что
        // действительно стримил.
        self.lens.last_back_role = if revert.from == LensRole::Front {
            self.lens.last_streamed_back_role
        } else {
            Some(revert.from)
        };
        self.reset_point_for_camera_change();
        self.retarget_reconfiguring_state();
        self.lens.revert_reconcile_pending = true;
        Halt
    }

    /// Хвост `reconcile` и разбора событий бэкенда: довести состояние до возвращённой цели.
    /// Флаг снимается до вызова, поэтому провал самого возврата не зацикливает.
    pub(super) fn run_pending_lens_revert(&mut self, now: Instant) {
        if std::mem::take(&mut self.lens.revert_reconcile_pending) {
            self.reconcile(now);
        }
    }

    /// Шкала зума для снимка и диагностика.
    pub(super) fn lens_status(&self) -> LensStatus {
        let active = self.session.as_ref().and_then(|session| {
            let camera = &session.configured.stream.camera;
            Some(ActiveLens {
                role: self.catalog.role_of(camera)?,
                camera: camera.clone(),
                facing: self.catalog.camera(camera)?.capabilities.facing,
            })
        });
        LensStatus {
            zoom_catalog: Arc::clone(&self.lens.zoom_catalog),
            active,
            switch: self.lens.tracker.state(),
            last_intent: self.lens.last_intent,
            blocked: self.lens.tracker.blocked().cloned(),
            counters: self.lens.tracker.counters(),
        }
    }
}
