//! Арбитраж арены: обработка входа кандидатами, разрешение готовых, сброс последовательности.

use super::super::kinds::{Input, Phase, StepContext, Verdict};
use super::super::spec::ExtraFingers;
use super::super::state::{GestureData, GestureEvent, GestureState, RecognizerId};
use super::{Candidate, GestureArena};
use crate::id::NodeId;
use crate::time::MonotonicNanos;

impl<I: Clone> GestureArena<I> {
    // ---------------------------------------------------------------- вход кандидатам

    /// Отдать вход каждому кандидату по приоритету, затем разрешить готовых.
    pub(super) fn dispatch(&mut self, input: Input, time: MonotonicNanos) {
        let ids: Vec<RecognizerId> = self.candidates.iter().map(|candidate| candidate.id).collect();
        let extra_finger_arrived = matches!(input, Input::Down(_)) && self.fingers.len() >= 2;
        for id in ids {
            let Some(index) = self.index_of(id) else { continue };
            let recognizer = &mut self.recognizers[index];
            if recognizer.state.is_terminal() {
                continue;
            }
            // Второй палец: однопальцевые провалены/отменены. Уже готовый (ждущий чужого провала)
            // жест своё условие выполнил и не трогается.
            let single_finger_only = recognizer.kind.extra_fingers() == Some(ExtraFingers::FailOrCancel);
            if extra_finger_arrived && single_finger_only && recognizer.ready.is_none() {
                if recognizer.state.is_active() {
                    self.cancel(index, time);
                } else {
                    self.fail(index, time);
                }
                continue;
            }
            let phase = if recognizer.state.is_active() {
                Phase::Active
            } else {
                Phase::Possible
            };
            let context = StepContext {
                fingers: &self.fingers,
                now: time,
                phase,
            };
            let verdict = recognizer.kind.step(input, &context);
            self.apply_verdict(index, verdict, time);
        }
        self.resolve(time);
    }

    fn apply_verdict(&mut self, index: usize, verdict: Verdict, time: MonotonicNanos) {
        let state = self.recognizers[index].state;
        match verdict {
            Verdict::Idle => {}
            Verdict::Ready(data) => {
                if state == GestureState::Possible {
                    self.recognizers[index].ready = Some(data);
                }
            }
            Verdict::Update(data) => {
                if state.is_active() {
                    let data = self.with_origin(index, data);
                    self.recognizers[index].last_data = Some(data);
                    self.transition(index, GestureState::Changed, data, time);
                }
            }
            Verdict::Finish(data) => {
                if state.is_active() {
                    let data = self.with_origin(index, data);
                    self.recognizers[index].last_data = Some(data);
                    self.transition(index, GestureState::Ended, data, time);
                }
            }
            Verdict::Fail => self.fail(index, time),
        }
    }

    /// Данные со смещением от точки распознавания этого распознавателя.
    fn with_origin(&self, index: usize, data: GestureData) -> GestureData {
        match self.recognizers[index].recognition_origin {
            Some(origin) => data.with_recognition_origin(origin),
            None => data,
        }
    }

    // ---------------------------------------------------------------- переходы

    /// Сменить состояние и опубликовать событие.
    fn transition(&mut self, index: usize, state: GestureState, data: GestureData, time: MonotonicNanos) {
        let recognizer = &mut self.recognizers[index];
        recognizer.state = state;
        self.events.push(GestureEvent {
            recognizer: recognizer.id,
            node: recognizer.node,
            intent: recognizer.intent.clone(),
            state,
            data,
            time,
        });
    }

    /// `Possible → Failed`. Для остальных состояний — no-op.
    pub(super) fn fail(&mut self, index: usize, time: MonotonicNanos) {
        if self.recognizers[index].state != GestureState::Possible {
            return;
        }
        self.recognizers[index].ready = None;
        self.transition(index, GestureState::Failed, GestureData::None, time);
    }

    /// `Began/Changed → Cancelled` с последними данными жеста. Для остальных состояний — no-op.
    pub(super) fn cancel(&mut self, index: usize, time: MonotonicNanos) {
        if !self.recognizers[index].state.is_active() {
            return;
        }
        let last_data = self.recognizers[index].last_data.unwrap_or(GestureData::None);
        self.transition(index, GestureState::Cancelled, last_data, time);
    }

    /// Готовый распознаватель начинается: непрерывный — `Began`, дискретный — сразу `Ended`.
    fn start(&mut self, index: usize, time: MonotonicNanos) {
        let recognizer = &mut self.recognizers[index];
        let Some(ready_data) = recognizer.ready.take() else {
            return;
        };
        let origin = ready_data.position();
        recognizer.recognition_origin = origin;
        let data = origin.map_or(ready_data, |point| ready_data.with_recognition_origin(point));
        recognizer.last_data = Some(data);
        let next_state = if recognizer.spec.is_discrete() {
            GestureState::Ended
        } else {
            GestureState::Began
        };
        self.transition(index, next_state, data, time);
    }

    /// Системная отмена или дыра: идущее отменяется, возможное (в том числе ждущее) проваливается.
    pub(super) fn cancel_all(&mut self, time: MonotonicNanos) {
        let ids: Vec<RecognizerId> = self.candidates.iter().map(|candidate| candidate.id).collect();
        for id in ids {
            let Some(index) = self.index_of(id) else { continue };
            if self.recognizers[index].state.is_active() {
                self.cancel(index, time);
            } else {
                self.fail(index, time);
            }
        }
    }

    // ---------------------------------------------------------------- разрешение готовых

    /// Пока есть готовый и не заблокированный распознаватель — запускать его и применять победу.
    /// Возвращает, менялось ли что-нибудь. Конечно: каждый шаг выводит один распознаватель из `Possible`.
    pub(super) fn resolve(&mut self, time: MonotonicNanos) -> bool {
        let mut changed = false;
        while let Some(id) = self.next_startable() {
            let Some(index) = self.index_of(id) else { break };
            changed = true;
            if self.is_preempted(id) {
                self.fail(index, time);
                continue;
            }
            self.start(index, time);
            if self.recognizers[index].spec.is_exclusive() {
                self.apply_win(id, time);
            }
        }
        changed
    }

    /// Первый по приоритету кандидат, который готов и ничем не заблокирован.
    fn next_startable(&self) -> Option<RecognizerId> {
        self.candidates.iter().map(|candidate| candidate.id).find(|&id| {
            self.recognizer(id)
                .is_some_and(|recognizer| recognizer.state == GestureState::Possible && recognizer.ready.is_some())
                && !self.is_blocked(id)
        })
    }

    /// Ждёт ли `id` провала кандидата, который ещё в `Possible`. Блокер вне кандидатов или
    /// удалённый не блокирует.
    fn is_blocked(&self, id: RecognizerId) -> bool {
        self.relations.blockers_of(id).any(|blocker| {
            self.is_candidate(blocker)
                && self
                    .recognizer(blocker)
                    .is_some_and(|recognizer| recognizer.state == GestureState::Possible)
        })
    }

    fn is_candidate(&self, id: RecognizerId) -> bool {
        self.candidates.iter().any(|candidate| candidate.id == id)
    }

    /// Уже начался или закончился эксклюзивный распознаватель, с которым `id` нельзя одновременно.
    fn is_preempted(&self, id: RecognizerId) -> bool {
        self.candidates.iter().any(|candidate| {
            candidate.id != id
                && self.recognizer(candidate.id).is_some_and(|other| {
                    let has_won = other.state.is_active() || other.state == GestureState::Ended;
                    has_won && other.spec.is_exclusive() && !self.relations.are_simultaneous(other.id, id)
                })
        })
    }

    /// Победа эксклюзивного `winner`: провалить/отменить остальных.
    fn apply_win(&mut self, winner: RecognizerId, time: MonotonicNanos) {
        let ids: Vec<RecognizerId> = self.candidates.iter().map(|candidate| candidate.id).collect();
        for id in ids {
            if id == winner {
                continue;
            }
            let Some(index) = self.index_of(id) else { continue };
            let state = self.recognizers[index].state;
            // «Требует провала победителя» проваливается всегда — даже при разрешённой одновременности.
            let waits_for_winner = self.relations.is_waiting_for(id, winner);
            if !waits_for_winner && self.relations.are_simultaneous(id, winner) {
                continue;
            }
            match state {
                GestureState::Possible => self.fail(index, time),
                GestureState::Began | GestureState::Changed => self.cancel(index, time),
                GestureState::Ended | GestureState::Cancelled | GestureState::Failed => {}
            }
        }
    }

    // ---------------------------------------------------------------- последовательность

    /// Первый палец ложится на пустой экран: кандидаты = пережившие прошлое касание + элементы пути.
    pub(super) fn start_sequence(&mut self, hit_path: &[NodeId]) {
        for (depth, node) in hit_path.iter().enumerate() {
            let ids_of_node: Vec<RecognizerId> = self
                .recognizers
                .iter()
                .filter(|recognizer| recognizer.node == *node)
                .map(|recognizer| recognizer.id)
                .collect();
            for id in ids_of_node {
                if !self.is_candidate(id) {
                    self.candidates.push(Candidate { id, depth });
                }
            }
        }
        self.candidates.sort_by_key(|candidate| (candidate.depth, candidate.id));
    }

    /// Если пальцев нет: терминальные и «пустые» кандидаты молча возвращаются в `Possible` и
    /// выбывают, остаются только живущие между касаниями. Инвариант: после этого каждый оставшийся
    /// кандидат либо ждёт тапа (таймер по кадрам), либо уже готов и ждёт провала того, у кого такой
    /// таймер есть — арена не залипает.
    pub(super) fn finish_sequence_if_idle(&mut self, time: MonotonicNanos) {
        if !self.fingers.is_empty() {
            return;
        }
        loop {
            self.prune_candidates();
            // Выжившие могли разблокироваться (блокер выбыл) и начаться: тогда снова сбросить.
            if !self.resolve(time) {
                break;
            }
        }
    }

    fn prune_candidates(&mut self) {
        let mut survivors = Vec::with_capacity(self.candidates.len());
        for candidate in &self.candidates {
            let Some(index) = self.index_of(candidate.id) else {
                continue;
            };
            if self.recognizers[index].survives_all_fingers_up() {
                survivors.push(*candidate);
            } else {
                self.recognizers[index].reset_to_possible();
            }
        }
        self.candidates = survivors;
    }
}
