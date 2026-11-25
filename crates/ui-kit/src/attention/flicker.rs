//! Мерцание ожидания: плавная прозрачность между 1 и нижним уровнем, пока ждём.

use std::f64::consts::TAU;

use ui_core::time::MonotonicNanos;

use super::{elapsed_seconds, smoothstep};
use crate::tokens::AttentionTokens;

/// Как началось мерцание: с чистого листа или поверх возвращения к 1 (чтобы не было скачка прозрачности).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Entry {
    /// Прозрачность была 1: волна идёт сразу.
    Fresh,
    /// Мерцание началось, пока прозрачность ещё не вернулась к 1: плавно вливаемся в волну с этого уровня.
    FromOpacity(f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    /// Не ждём, прозрачность 1.
    Idle,
    /// Ждём: волна от момента `started`.
    Waiting { started: MonotonicNanos, entry: Entry },
    /// Ожидание кончилось в `stopped` при прозрачности `from`: мягко возвращаемся к 1.
    Settling { stopped: MonotonicNanos, from: f64 },
}

/// Мерцание ожидания. Владелец — экран: он зовёт [`WaitingFlicker::start`] / [`WaitingFlicker::stop`] по
/// состоянию «ждём ответа» и рисует с [`WaitingFlicker::opacity`].
#[derive(Debug, Clone)]
pub struct WaitingFlicker {
    tokens: AttentionTokens,
    state: State,
}

impl WaitingFlicker {
    /// Покоящееся мерцание (прозрачность 1). Токены берутся копией при создании.
    pub fn new(tokens: &AttentionTokens) -> Self {
        Self {
            tokens: *tokens,
            state: State::Idle,
        }
    }

    /// Начать ожидание в `now`. Если оно уже идёт — ничего не меняет (волна не перезапускается). Если шло
    /// возвращение к 1, мерцание вливается с текущего уровня без скачка.
    pub fn start(&mut self, now: MonotonicNanos) {
        let entry = match self.state {
            State::Waiting { .. } => return,
            State::Idle => Entry::Fresh,
            State::Settling { .. } => Entry::FromOpacity(self.opacity(now)),
        };
        self.state = State::Waiting { started: now, entry };
    }

    /// Закончить ожидание в `now`: прозрачность мягко вернётся к 1. Если ожидания не было — ничего не меняет.
    pub fn stop(&mut self, now: MonotonicNanos) {
        if matches!(self.state, State::Waiting { .. }) {
            self.state = State::Settling {
                stopped: now,
                from: self.opacity(now),
            };
        }
    }

    /// Идёт ли ожидание (не возвращение).
    pub fn is_waiting(&self) -> bool {
        matches!(self.state, State::Waiting { .. })
    }

    /// Нужно ли ещё рисовать кадры: ждём или прозрачность ещё возвращается к 1.
    pub fn is_animating(&self, now: MonotonicNanos) -> bool {
        match self.state {
            State::Idle => false,
            State::Waiting { .. } => true,
            State::Settling { stopped, .. } => elapsed_seconds(stopped, now) < self.tokens.waiting_settle_s,
        }
    }

    /// Прозрачность в момент `now`, 0..=1.
    pub fn opacity(&self, now: MonotonicNanos) -> f64 {
        match self.state {
            State::Idle => 1.0,
            State::Waiting { started, entry } => {
                let elapsed = elapsed_seconds(started, now);
                let wave = self.wave_opacity(elapsed);
                match entry {
                    Entry::Fresh => wave,
                    Entry::FromOpacity(from) => {
                        let blend = smoothstep(elapsed / self.tokens.waiting_settle_s);
                        from + (wave - from) * blend
                    }
                }
            }
            State::Settling { stopped, from } => {
                let blend = smoothstep(elapsed_seconds(stopped, now) / self.tokens.waiting_settle_s);
                from + (1.0 - from) * blend
            }
        }
    }

    /// Волна ожидания: начинается с 1, за полпериода опускается до нижнего уровня и возвращается (косинус).
    fn wave_opacity(&self, elapsed_seconds: f64) -> f64 {
        let depth = 1.0 - self.tokens.waiting_min_opacity;
        let phase = TAU * elapsed_seconds / self.tokens.waiting_period_s;
        1.0 - depth * 0.5 * (1.0 - phase.cos())
    }
}
