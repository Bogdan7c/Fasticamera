//! Арена жестов: арбитраж распознавателей по модели UIKit.
//!
//! # Модель
//! * **Последовательность касания** — от первого пальца вниз до последнего вверх. Её кандидаты —
//!   распознаватели элементов на пути хит-теста **первого** пальца (путь передаёт вызывающий на
//!   `Began`). Приоритет: глубина (глубже раньше), затем порядок добавления.
//! * **Следующие пальцы** идут тем же кандидатам; их путь хит-теста не используется. Однопальцевые
//!   распознаватели на втором пальце проваливаются/отменяются (параметр [`ExtraFingers`]).
//! * **Эксклюзивность**: когда эксклюзивный распознаватель начался (непрерывный — `Began`,
//!   дискретный — `Ended`), остальные кандидаты в `Possible` проваливаются, идущие отменяются —
//!   кроме пар, которым разрешено `allow_simultaneous`. Если начать хотят несколько на одном
//!   входе, побеждает первый по приоритету (более глубокий). `Press` не эксклюзивен: он только
//!   отменяется чужой победой.
//! * **`require_to_fail(A, B)`**: A, будучи готов, ждёт в `Possible`, пока B не провалится; B начался —
//!   A проваливается (даже если разрешено одновременно); B провалился или исчез — A срабатывает
//!   сразу, хоть спустя время (таймер идёт от кадров).
//! * **Сброс**: после отрыва всех пальцев терминальные распознаватели молча возвращаются в
//!   `Possible`. Остаются кандидатами только те, кто живёт между касаниями: ждёт второго тапа или
//!   готов и ждёт чужого провала. Они же участвуют в следующем касании вместе с новыми кандидатами
//!   пути — так двойной тап собирается из двух касаний.
//! * **Время** приходит снаружи: метки касаний и [`GestureArena::advance_time`] (кадры). Таймеры
//!   (долгий тап, интервал двойного) проверяются и по кадру, и по метке касания; метка касания раньше
//!   уже пришедшего кадра не отбрасывается.
//!
//! # Порядок событий в одном входе
//! Сначала каждый кандидат (по приоритету) обрабатывает вход сам (`Changed`, `Ended` активных,
//! `Failed` по собственным условиям), затем разрешаются готовые к началу. Поэтому `Press`,
//! завершившийся отрывом, остаётся `Ended`, даже если на том же отрыве победил `Tap` соседа.
//!
//! # Известные ограничения
//! * Путь хит-теста второго и следующих пальцев не учитывается.
//! * Тап не имеет максимальной длительности удержания (как `UITapGestureRecognizer`).

mod arbitration;
mod recognizer;

use shell_contract::touch::TouchPhase;

use self::recognizer::Recognizer;
use super::error::GestureError;
use super::kinds::Kind;
use super::relations::Relations;
use super::spec::RecognizerSpec;
use super::state::{GestureEvent, GestureState, RecognizerId};
use super::touches::Fingers;
use crate::id::NodeId;
use crate::input::PointerSample;
use crate::time::{AnimationClock, ClockAdvance, MonotonicNanos, MotionPreference};
use crate::velocity::{ScalarVelocityTracker, VelocityConfig, VelocityTracker};

/// Настройки арены.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct GestureConfig {
    /// Оценка скорости пальца (пан, свайп, щипок).
    pub velocity: VelocityConfig,
}

/// Как арена приняла касание. Игнорирование — не ошибка, но и не молчаливое: оболочка видит причину.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerDisposition {
    Applied,
    /// `Moved`/`Ended` для пальца, о котором арена не знает (например, после дыры в потоке).
    IgnoredUnknownPointer,
    /// `Began` для пальца, который уже лежит.
    IgnoredDuplicateBegan,
    /// Координаты не конечны (NaN/inf).
    IgnoredInvalidPosition,
}

/// Кандидат последовательности: распознаватель и глубина его элемента на пути хит-теста.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    id: RecognizerId,
    depth: usize,
}

/// Арена жестов экрана. `I` — тип намерения экрана, который арена возвращает в событиях.
pub struct GestureArena<I> {
    /// Распознаватели по возрастанию `id` (порядок добавления).
    recognizers: Vec<Recognizer<I>>,
    next_id: u32,
    relations: Relations,
    fingers: Fingers,
    /// Кандидаты в порядке приоритета.
    candidates: Vec<Candidate>,
    events: Vec<GestureEvent<I>>,
    /// Время, до которого уже проверены таймеры.
    timer_clock: AnimationClock,
    /// Самое позднее время любого входа: метка для событий вне входа (удаление распознавателя).
    latest_input_time: MonotonicNanos,
    scale_velocity_template: ScalarVelocityTracker,
}

impl<I: Clone> GestureArena<I> {
    pub fn new(config: GestureConfig) -> Result<Self, GestureError> {
        let finger_velocity = VelocityTracker::new(config.velocity.clone())?;
        let scale_velocity_template = ScalarVelocityTracker::new(config.velocity)?;
        Ok(Self {
            recognizers: Vec::new(),
            next_id: 0,
            relations: Relations::default(),
            fingers: Fingers::new(finger_velocity),
            candidates: Vec::new(),
            events: Vec::new(),
            timer_clock: AnimationClock::new(MotionPreference::Full),
            latest_input_time: MonotonicNanos::from_nanos(0),
            scale_velocity_template,
        })
    }

    /// Добавить распознаватель элемента `node`. Кандидатом он становится со следующего касания.
    pub fn add_recognizer(
        &mut self,
        node: NodeId,
        spec: RecognizerSpec,
        intent: I,
    ) -> Result<RecognizerId, GestureError> {
        spec.validate()?;
        // Идентификаторы не переиспользуются (порядок id = порядок добавления = приоритет), поэтому
        // исчерпание — явная ошибка, а не тихое «насыщение» с повтором последнего id.
        let id = RecognizerId(self.next_id);
        let next_id = self
            .next_id
            .checked_add(1)
            .ok_or(GestureError::RecognizerIdsExhausted)?;
        self.next_id = next_id;
        let kind = Kind::new(&spec, &self.scale_velocity_template);
        self.recognizers.push(Recognizer {
            id,
            node,
            intent,
            spec,
            kind,
            state: GestureState::Possible,
            ready: None,
            last_data: None,
            recognition_origin: None,
        });
        Ok(id)
    }

    /// Удалить распознаватель. Идущий жест получает `Cancelled`; ждавшие его провала освобождаются.
    pub fn remove_recognizer(&mut self, id: RecognizerId) -> Result<(), GestureError> {
        let index = self.index_of(id).ok_or(GestureError::UnknownRecognizer(id))?;
        let time = self.latest_input_time;
        if self.recognizers[index].state.is_active() {
            self.cancel(index, time);
        }
        self.candidates.retain(|candidate| candidate.id != id);
        self.relations.forget(id);
        self.recognizers.remove(index);
        self.resolve(time);
        self.finish_sequence_if_idle(time);
        Ok(())
    }

    /// Удалить все распознаватели элемента; возвращает, сколько удалено. Ошибка — только если
    /// инвариант арены нарушен (распознаватель из списка вдруг неизвестен): она видна, а не
    /// пропускается.
    pub fn remove_node_recognizers(&mut self, node: NodeId) -> Result<usize, GestureError> {
        let doomed: Vec<RecognizerId> = self
            .recognizers
            .iter()
            .filter(|recognizer| recognizer.node == node)
            .map(|recognizer| recognizer.id)
            .collect();
        for id in &doomed {
            self.remove_recognizer(*id)?;
        }
        Ok(doomed.len())
    }

    /// `waiting` не сработает, пока `must_fail` не провалится.
    pub fn require_to_fail(&mut self, waiting: RecognizerId, must_fail: RecognizerId) -> Result<(), GestureError> {
        self.ensure_known(waiting)?;
        self.ensure_known(must_fail)?;
        self.relations.require_to_fail(waiting, must_fail)?;
        Ok(())
    }

    /// Разрешить двум распознавателям быть распознанными одновременно (по умолчанию нельзя).
    pub fn allow_simultaneous(&mut self, first: RecognizerId, second: RecognizerId) -> Result<(), GestureError> {
        self.ensure_known(first)?;
        self.ensure_known(second)?;
        self.relations.allow_simultaneous(first, second)?;
        Ok(())
    }

    pub fn recognizer_state(&self, id: RecognizerId) -> Result<GestureState, GestureError> {
        self.recognizer(id)
            .map(|recognizer| recognizer.state)
            .ok_or(GestureError::UnknownRecognizer(id))
    }

    /// Нет ни пальцев, ни кандидатов: арена покоится и ничего не ждёт.
    pub fn is_idle(&self) -> bool {
        self.fingers.is_empty() && self.candidates.is_empty()
    }

    /// Забрать накопленные события в порядке возникновения.
    pub fn drain_events(&mut self) -> Vec<GestureEvent<I>> {
        std::mem::take(&mut self.events)
    }

    /// Касание. `hit_path` — элементы под пальцем от самого глубокого к корню; используется только
    /// на `Began` первого пальца последовательности.
    pub fn handle_pointer(&mut self, sample: PointerSample, hit_path: &[NodeId]) -> PointerDisposition {
        if !sample.position.x.is_finite() || !sample.position.y.is_finite() {
            return PointerDisposition::IgnoredInvalidPosition;
        }
        let time = sample.time;
        self.latest_input_time = self.latest_input_time.max(time);
        match sample.phase {
            TouchPhase::Began => {
                if self.fingers.contains(sample.pointer) {
                    return PointerDisposition::IgnoredDuplicateBegan;
                }
                self.run_timers(time);
                if self.fingers.is_empty() {
                    self.start_sequence(hit_path);
                }
                self.fingers.begin(sample);
                self.dispatch(super::kinds::Input::Down(sample.pointer), time);
            }
            TouchPhase::Moved => {
                if !self.fingers.contains(sample.pointer) {
                    return PointerDisposition::IgnoredUnknownPointer;
                }
                self.run_timers(time);
                self.fingers.moved(sample);
                self.dispatch(super::kinds::Input::Move(sample.pointer), time);
            }
            TouchPhase::Ended => {
                if !self.fingers.contains(sample.pointer) {
                    return PointerDisposition::IgnoredUnknownPointer;
                }
                self.run_timers(time);
                self.fingers.lifted(sample);
                self.dispatch(super::kinds::Input::Up(sample.pointer), time);
                self.fingers.remove(sample.pointer);
                self.finish_sequence_if_idle(time);
            }
            TouchPhase::Cancelled => self.abort_everything(time),
        }
        PointerDisposition::Applied
    }

    /// Дыра в потоке касаний: всё активное отменяется, возможное проваливается.
    pub fn handle_stream_gap(&mut self) {
        self.abort_everything(self.latest_input_time);
    }

    /// Новый кадр: время vsync. Двигает таймеры, даже когда палец стоит и касаний нет.
    pub fn advance_time(&mut self, now: MonotonicNanos) -> ClockAdvance {
        self.latest_input_time = self.latest_input_time.max(now);
        let advance = self.timer_clock.advance_to(now);
        if matches!(advance, ClockAdvance::Advanced { .. }) {
            self.dispatch(super::kinds::Input::Tick, now);
            self.finish_sequence_if_idle(now);
        }
        advance
    }

    /// Проверить таймеры по метке касания, если она позже уже проверенного времени.
    fn run_timers(&mut self, time: MonotonicNanos) {
        if self.timer_clock.now().is_none_or(|checked| time > checked) {
            self.timer_clock.advance_to(time);
            self.dispatch(super::kinds::Input::Tick, time);
            self.finish_sequence_if_idle(time);
        }
    }

    fn abort_everything(&mut self, time: MonotonicNanos) {
        self.cancel_all(time);
        self.fingers.clear();
        self.finish_sequence_if_idle(time);
    }

    fn index_of(&self, id: RecognizerId) -> Option<usize> {
        self.recognizers
            .binary_search_by_key(&id, |recognizer| recognizer.id)
            .ok()
    }

    fn recognizer(&self, id: RecognizerId) -> Option<&Recognizer<I>> {
        self.index_of(id).map(|index| &self.recognizers[index])
    }

    fn ensure_known(&self, id: RecognizerId) -> Result<(), GestureError> {
        self.index_of(id).map(|_| ()).ok_or(GestureError::UnknownRecognizer(id))
    }
}

#[cfg(test)]
mod id_exhaustion_tests {
    use super::*;
    use crate::gesture::spec::TapSpec;

    /// Исчерпание идентификаторов — явная ошибка; арена при этом не меняется, а последний выданный
    /// id остаётся уникальным (раньше `saturating_add` выдал бы повтор).
    #[test]
    fn exhausted_ids_are_an_error_and_do_not_change_the_arena() {
        let mut arena: GestureArena<&'static str> = GestureArena::new(GestureConfig::default()).unwrap();
        let node = NodeId::new(0, 0);
        // Перед последним допустимым значением счётчика.
        arena.next_id = u32::MAX - 1;
        let last = arena
            .add_recognizer(node, RecognizerSpec::Tap(TapSpec::default()), "last")
            .unwrap();
        assert_eq!(last.ordinal(), u32::MAX - 1);

        let error = arena
            .add_recognizer(node, RecognizerSpec::Tap(TapSpec::default()), "overflow")
            .unwrap_err();
        assert_eq!(error, GestureError::RecognizerIdsExhausted);
        assert_eq!(arena.recognizers.len(), 1, "неудачное добавление ничего не добавило");
        assert_eq!(arena.next_id, u32::MAX, "счётчик не сдвинут ошибкой");
    }
}
