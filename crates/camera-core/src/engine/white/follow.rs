//! Слежение за белым источника: режим, темп вопросов, мёртвая зона и заморозка блокировкой AE/AF.
//!
//! Чистая часть владельца: сама белую не меняет и заявок не заводит, только отвечает [`super::owner::WhiteOwner`]
//! на вопросы «пора ли спросить», «можно ли сейчас двигать белую» и «стоит ли этот ответ переезда».
//!
//! Инварианты:
//! * темп считается по меткам кадров сенсора, часов здесь нет;
//! * во время снимка со вспышкой и ещё `settle_after_flash_ms` после него источника не спрашивают;
//! * метка раньше прошлой — другой поток: темп начинается заново, а не ждёт, пока часы догонят;
//! * блокировка действует только на слежение: выключенному слежению она безразлична;
//! * пауза (экран открыл CALIBRATE) — второй замок, независимый от блокировки: слежение заморожено, пока висит
//!   хотя бы один; режим «Auto» и память паузой не трогаются, и сама пауза не запоминается.

use std::time::Duration;

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::three_a::AeAfLock;
use camera_contract::white::{HeldWhite, WhiteOrigin, WhiteSourceId};

use crate::white::{
    WhiteFollowConfig, WhiteFollowPause, WhiteFollowRecord, WhiteFollowSpeed, WhiteFollowState, WhiteFollowStatus,
    WhiteKeeping, WhiteRequestOutcome,
};

/// Можно ли этим кадром спрашивать источник ради слежения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::engine) enum FollowTick {
    /// Обычный кадр с меткой сенсора.
    Frame(SensorTimestamp),
    /// Кадр без метки сенсора: темп не двигается.
    Unstamped,
    /// Идёт снимок со вспышкой: свет сцены подмешан нашим светодиодом.
    FlashInProgress,
}

/// Пауза из-за вспышки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlashPause {
    None,
    /// Снимок идёт или кончился, но обычного кадра после него ещё не было.
    AwaitingFirstFrame,
    /// Первый обычный кадр после снимка: от него считается время успокоения автоматики.
    Settling {
        since: SensorTimestamp,
    },
}

/// Изменилось ли слежение от просьбы.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::engine) enum FollowChange {
    Changed,
    Unchanged,
}

/// Стоит ли ответ источника переезда белой.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FollowVerdict {
    Move,
    /// Разница в пределах дрожи оценки.
    InsideDeadZone,
}

/// Слежение владельца белого.
#[derive(Debug)]
pub(super) struct WhiteFollow {
    config: WhiteFollowConfig,
    /// За кем следим; `None` — слежение выключено.
    source: Option<WhiteSourceId>,
    /// Скорость, выбранная человеком.
    chosen_speed: Option<WhiteFollowSpeed>,
    lock: AeAfLock,
    /// Пауза от экрана (CALIBRATE); не зависит от `lock` и не запоминается.
    pause: WhiteFollowPause,
    /// Метка кадра, на котором спросили в последний раз на этом потоке.
    last_ask: Option<SensorTimestamp>,
    flash: FlashPause,
    asked: u64,
    moved: u64,
    last_refusal: Option<WhiteRequestOutcome>,
}

impl WhiteFollow {
    /// Слежение, каким его запомнила память: «следить» возобновляется за источником открытия (он один).
    pub(super) fn restored(
        config: WhiteFollowConfig,
        remembered: WhiteFollowRecord,
        opening_source: WhiteSourceId,
    ) -> Self {
        Self {
            config,
            source: match remembered.keeping {
                WhiteKeeping::FollowSource => Some(opening_source),
                WhiteKeeping::Hold => None,
            },
            chosen_speed: remembered.speed,
            lock: AeAfLock::Unlocked,
            pause: WhiteFollowPause::Running,
            last_ask: None,
            flash: FlashPause::None,
            asked: 0,
            moved: 0,
            last_refusal: None,
        }
    }

    // ---- Просьбы ----

    /// Следить за `source`. Счётчики и последний отказ прежнего слежения к новому не относятся.
    pub(super) fn start(&mut self, source: WhiteSourceId) -> FollowChange {
        if self.source == Some(source) {
            return FollowChange::Unchanged;
        }
        self.source = Some(source);
        self.last_refusal = None;
        self.forget_pace();
        FollowChange::Changed
    }

    pub(super) fn stop(&mut self) -> FollowChange {
        if self.source.take().is_none() {
            return FollowChange::Unchanged;
        }
        self.last_refusal = None;
        FollowChange::Changed
    }

    /// Выбор скорости. Совпавший с конфигом выбор — всё равно выбор: его запоминаем.
    pub(super) fn choose_speed(&mut self, speed: WhiteFollowSpeed) -> FollowChange {
        if self.chosen_speed == Some(speed) {
            return FollowChange::Unchanged;
        }
        self.chosen_speed = Some(speed);
        FollowChange::Changed
    }

    /// Блокировка AE/AF сейчас. Снятие блокировки спрашивает источник с ближайшего кадра.
    pub(super) fn note_lock(&mut self, lock: AeAfLock) {
        if self.lock != lock {
            self.lock = lock;
            self.forget_pace();
        }
    }

    /// Пауза слежения от экрана. Снятие паузы спрашивает источник с ближайшего кадра, как и снятие блокировки.
    /// Пережила смену линзы, поток и фон: её ставит и снимает только экран.
    pub(super) fn set_pause(&mut self, pause: WhiteFollowPause) -> FollowChange {
        if self.pause == pause {
            return FollowChange::Unchanged;
        }
        self.pause = pause;
        self.forget_pace();
        FollowChange::Changed
    }

    /// Поток кончился: метки нового к прежним не относятся, автоматика нового вспышки не видела.
    pub(super) fn note_stream_ended(&mut self) {
        self.forget_pace();
        self.flash = FlashPause::None;
    }

    /// Пришёл кадр: ведёт паузу вспышки.
    pub(super) fn note_tick(&mut self, tick: FollowTick) {
        self.flash = match (tick, self.flash) {
            (FollowTick::FlashInProgress, _) => FlashPause::AwaitingFirstFrame,
            (FollowTick::Frame(frame), FlashPause::AwaitingFirstFrame) => FlashPause::Settling { since: frame },
            (FollowTick::Frame(frame), FlashPause::Settling { since }) if self.flash_settled(since, frame) => {
                FlashPause::None
            }
            (_, unchanged) => unchanged,
        };
    }

    /// Темп начинается заново: спросить с первого же кадра.
    fn forget_pace(&mut self) {
        self.last_ask = None;
    }

    pub(super) fn note_asked(&mut self, frame: SensorTimestamp) {
        self.last_ask = Some(frame);
        self.asked += 1;
    }

    pub(super) fn note_moved(&mut self) {
        self.last_refusal = None;
        self.moved += 1;
    }

    /// Ответ есть, но двигаться незачем.
    pub(super) fn note_stayed(&mut self) {
        self.last_refusal = None;
    }

    pub(super) fn note_refused(&mut self, outcome: WhiteRequestOutcome) {
        self.last_refusal = Some(outcome);
    }

    // ---- Вопросы ----

    /// За кем следим (в том числе под блокировкой).
    pub(super) fn source(&self) -> Option<WhiteSourceId> {
        self.source
    }

    /// Слежение включено и заморожено блокировкой.
    fn is_held_by_lock(&self) -> bool {
        self.source.is_some() && self.lock == AeAfLock::Locked
    }

    /// Слежение включено и поставлено на паузу.
    fn is_paused(&self) -> bool {
        self.source.is_some() && self.pause == WhiteFollowPause::Paused
    }

    /// Слежение включено и заморожено хотя бы одним из замков: блокировкой AE/AF или паузой.
    pub(super) fn is_frozen(&self) -> bool {
        self.is_held_by_lock() || self.is_paused()
    }

    /// Слежение включено и стоит именно на паузе (для выбора причины отмены заявки).
    pub(super) fn is_held_by_pause(&self) -> bool {
        self.is_paused()
    }

    /// За кем спрашивать на кадре `frame`, если пора; `None` — не следим, заморожено или рано.
    pub(super) fn due_source(&self, frame: SensorTimestamp) -> Option<WhiteSourceId> {
        if self.is_frozen() || self.flash != FlashPause::None {
            return None;
        }
        let interval = Duration::from_millis(self.config.ask_interval_ms);
        self.elapsed_since_ask(frame)
            .is_none_or(|elapsed| elapsed >= interval)
            .then_some(self.source)
            .flatten()
    }

    /// Молчит ли источник на вопрос слежения дольше `timeout`.
    pub(super) fn ask_expired(&self, frame: SensorTimestamp, timeout: Duration) -> bool {
        self.elapsed_since_ask(frame).is_some_and(|elapsed| elapsed >= timeout)
    }

    /// Стоит ли белая источника `candidate` переезда от действующей. Мёртвая зона сравнивает с белой ТОГО ЖЕ
    /// источника: белая руки или запасного света уступает первому же ответу, иначе происхождение осталось бы
    /// «ручным» при включённом слежении.
    pub(super) fn verdict(&self, held: HeldWhite, candidate: HeldWhite) -> FollowVerdict {
        if held.origin != candidate.origin || !matches!(held.origin, WhiteOrigin::Source(_)) {
            return FollowVerdict::Move;
        }
        let mired = |white: HeldWhite| 1.0e6 / white.setting.temperature.kelvin();
        let mired_gap = (mired(held) - mired(candidate)).abs();
        let duv_gap = (held.setting.tint.duv() - candidate.setting.tint.duv()).abs();
        if mired_gap > self.config.dead_zone_mired || duv_gap > self.config.dead_zone_duv {
            FollowVerdict::Move
        } else {
            FollowVerdict::InsideDeadZone
        }
    }

    /// Действующая скорость: выбор человека, а если его нет — конфиг.
    pub(super) fn speed(&self) -> WhiteFollowSpeed {
        self.chosen_speed.unwrap_or_else(|| self.config.default_speed())
    }

    /// Что помнить между запусками.
    pub(super) fn record(&self) -> WhiteFollowRecord {
        WhiteFollowRecord {
            keeping: match self.source {
                Some(_) => WhiteKeeping::FollowSource,
                None => WhiteKeeping::Hold,
            },
            speed: self.chosen_speed,
        }
    }

    pub(super) fn status(&self) -> WhiteFollowStatus {
        WhiteFollowStatus {
            state: match self.source {
                None => WhiteFollowState::Off,
                // Пауза показывается раньше блокировки: она явнее и снимается только экраном.
                Some(source) if self.pause == WhiteFollowPause::Paused => WhiteFollowState::Paused { source },
                Some(source) if self.lock == AeAfLock::Locked => WhiteFollowState::HeldByLock { source },
                Some(source) => WhiteFollowState::Following { source },
            },
            speed: self.speed(),
            asked: self.asked,
            moved: self.moved,
            last_refusal: self.last_refusal,
        }
    }

    fn flash_settled(&self, since: SensorTimestamp, frame: SensorTimestamp) -> bool {
        let settle = Duration::from_millis(self.config.settle_after_flash_ms);
        frame.duration_since(since).is_some_and(|elapsed| elapsed >= settle)
    }

    /// Сколько прошло с прошлого вопроса; `None` — на этом потоке ещё не спрашивали.
    fn elapsed_since_ask(&self, frame: SensorTimestamp) -> Option<Duration> {
        frame.duration_since(self.last_ask?)
    }
}

#[cfg(test)]
#[path = "follow_tests.rs"]
mod tests;
