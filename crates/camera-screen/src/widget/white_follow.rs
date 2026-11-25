//! Слежение шкалы за фактами о белом (WB21) и защита от запоздавших фактов.
//!
//! Правило из трёх частей:
//! 1. Шкала следует за ИЗМЕНЕНИЕМ факта, а не за каждым кадром: пока факт тот же, что уже разобран, ничего не делается
//!    (так же, как `last_followed` у ручек раздела калибровки).
//! 2. Пока шкалу ведёт палец, факты игнорируются целиком и «разобранными» НЕ считаются: после отпускания
//!    свежий факт будет разобран по правилу 3.
//! 3. Значения, которые отправил палец, запоминаются на короткий срок. Факт, равный запомненному, — это эхо
//!    нашего же жеста (факты отстают на кадр-два): шкала не двигается, а запись и все более ранние стираются.
//!    Факт, которого нет среди отправленных, — чужая правка (кнопка света, «Телефон», перезапуск): шкала
//!    встаёт на него, память стирается.
//!
//! Гонка из задания: палец отпущен на K2, приходит устаревший K1 (он среди отправленных — игнор, шкала не
//! дёрнулась назад), затем K2 (эхо — игнор), затем K3 (чужой — шкала догоняет).
//!
//! Модуль чистый: времени у него нет, `now` приходит снаружи; сцены и виджетов он не знает.

use std::collections::VecDeque;

use ui_core::time::MonotonicNanos;

use crate::white_layout::WhiteCardLayout;

/// Что делать шкале с очередным фактом.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum FollowVerdict {
    /// Не двигать: факт уже разобран, это эхо пальца или палец ещё ведёт шкалу.
    Stay,
    /// Встать на значение факта точно (без намерений).
    Follow(f64),
}

/// Значение, отправленное пальцем, и когда.
#[derive(Debug, Clone, Copy)]
struct SentValue {
    value: f64,
    at: MonotonicNanos,
}

/// Числа правила: допуск сравнения, срок и размер памяти.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct EchoRules {
    /// Допуск «то же значение», единицы положения шкалы.
    pub match_tolerance_units: f64,
    /// Срок жизни отправленного значения, нс.
    pub memory_nanos: i64,
    /// Сколько значений помнится.
    pub memory_len: usize,
}

impl EchoRules {
    pub(crate) fn from_layout(layout: &WhiteCardLayout) -> Self {
        Self {
            match_tolerance_units: layout.echo_match_tolerance_units,
            memory_nanos: (layout.echo_memory_ms * 1_000_000.0) as i64,
            memory_len: layout.echo_memory_len,
        }
    }
}

/// Слежение одной шкалы. Значения — в «единицах факта» шкалы (Кельвины; единицы оттенка).
#[derive(Debug, Default)]
pub(crate) struct AxisFollower {
    /// Последний разобранный факт.
    last_seen: Option<f64>,
    /// Отправленное пальцем и ещё не подтверждённое фактами, от старого к новому.
    sent: VecDeque<SentValue>,
    /// Палец что-то отправил, и итоговая сверка шкалы с фактом ещё не делалась.
    needs_reconcile: bool,
}

impl AxisFollower {
    /// Палец отправил значение (намерение ушло экрану).
    pub(crate) fn note_sent(&mut self, value: f64, now: MonotonicNanos, rules: &EchoRules) {
        if self
            .sent
            .back()
            .is_some_and(|last| same(last.value, value, rules.match_tolerance_units))
        {
            // Тот же номер подряд (палец стоит) не раздувает память.
            return;
        }
        self.needs_reconcile = true;
        self.sent.push_back(SentValue { value, at: now });
        while self.sent.len() > rules.memory_len {
            self.sent.pop_front();
        }
    }

    /// Разбор факта. `finger_driven` — шкалу сейчас ведёт палец (или её инерция).
    pub(crate) fn on_fact(
        &mut self,
        fact: f64,
        finger_driven: bool,
        now: MonotonicNanos,
        rules: &EchoRules,
    ) -> FollowVerdict {
        if finger_driven {
            return FollowVerdict::Stay;
        }
        self.forget_expired(now, rules);
        if self
            .last_seen
            .is_some_and(|seen| same(seen, fact, rules.match_tolerance_units))
        {
            return FollowVerdict::Stay;
        }
        self.last_seen = Some(fact);
        match self
            .sent
            .iter()
            .position(|sent| same(sent.value, fact, rules.match_tolerance_units))
        {
            Some(index) => {
                // Эхо нашего жеста: подтверждено всё до этого значения включительно.
                self.sent.drain(..=index);
                FollowVerdict::Stay
            }
            None => {
                // Чужая правка: прежние отправленные значения уже не про это белое.
                self.sent.clear();
                FollowVerdict::Follow(fact)
            }
        }
    }

    /// Итоговая сверка: шкалу вели пальцем, теперь она не ведётся, а память отправленного пуста (подтверждена
    /// или истекла). Тогда правду говорит текущий факт `fact`: шкала обязана встать на него, если разошлась
    /// (намерение отвергнуто редьюсером, движок зажал значение, кнопка света поставила то же число, что мы
    /// уже отправляли). Срабатывает один раз на серию ведения.
    pub(crate) fn reconcile(
        &mut self,
        fact: f64,
        finger_driven: bool,
        now: MonotonicNanos,
        rules: &EchoRules,
    ) -> Option<f64> {
        if finger_driven || !self.needs_reconcile {
            return None;
        }
        self.forget_expired(now, rules);
        if !self.sent.is_empty() {
            return None;
        }
        self.needs_reconcile = false;
        self.last_seen = Some(fact);
        Some(fact)
    }

    /// Сколько отправленных значений ещё ждут подтверждения (для тестов и диагностики).
    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.sent.len()
    }

    fn forget_expired(&mut self, now: MonotonicNanos, rules: &EchoRules) {
        while self
            .sent
            .front()
            .is_some_and(|front| now.as_nanos().saturating_sub(front.at.as_nanos()) > rules.memory_nanos)
        {
            self.sent.pop_front();
        }
    }
}

fn same(left: f64, right: f64, tolerance: f64) -> bool {
    (left - right).abs() <= tolerance
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: EchoRules = EchoRules {
        match_tolerance_units: 0.01,
        memory_nanos: 600_000_000,
        memory_len: 4,
    };

    fn at(ms: i64) -> MonotonicNanos {
        MonotonicNanos::from_nanos(ms * 1_000_000)
    }

    #[test]
    fn the_first_fact_is_followed_and_the_same_fact_again_is_not() {
        let mut follower = AxisFollower::default();
        assert_eq!(
            follower.on_fact(5600.0, false, at(0), &RULES),
            FollowVerdict::Follow(5600.0)
        );
        // Тот же факт на следующем кадре: слежение привязано к изменению, а не к кадру.
        assert_eq!(follower.on_fact(5600.0, false, at(16), &RULES), FollowVerdict::Stay);
    }

    #[test]
    fn a_stale_then_the_final_echo_of_the_finger_never_move_the_scale_but_a_foreign_value_does() {
        let mut follower = AxisFollower::default();
        follower.on_fact(5600.0, false, at(0), &RULES);
        // Палец довёл до K2 = 4000, по пути отправил 5000 (K1) и 4000.
        follower.note_sent(5000.0, at(100), &RULES);
        follower.note_sent(4000.0, at(116), &RULES);
        // Пока ведёт палец — факты игнорируются, даже чужие.
        assert_eq!(follower.on_fact(5000.0, true, at(120), &RULES), FollowVerdict::Stay);
        // Палец отпущен; приходит устаревший K1, затем K2: шкала не двигается.
        assert_eq!(follower.on_fact(5000.0, false, at(140), &RULES), FollowVerdict::Stay);
        assert_eq!(follower.on_fact(4000.0, false, at(156), &RULES), FollowVerdict::Stay);
        assert_eq!(follower.pending_len(), 0);
        // Чужая правка (кнопка света): шкала догоняет.
        assert_eq!(
            follower.on_fact(3200.0, false, at(300), &RULES),
            FollowVerdict::Follow(3200.0)
        );
    }

    #[test]
    fn a_foreign_fact_ignored_during_the_drag_is_followed_after_release() {
        let mut follower = AxisFollower::default();
        follower.on_fact(5600.0, false, at(0), &RULES);
        follower.note_sent(5000.0, at(50), &RULES);
        // Во время ведения пришёл чужой 2700 — игнор и НЕ «разобран».
        assert_eq!(follower.on_fact(2700.0, true, at(60), &RULES), FollowVerdict::Stay);
        // После отпускания тот же факт всё ещё свежий: шкала догоняет.
        assert_eq!(
            follower.on_fact(2700.0, false, at(80), &RULES),
            FollowVerdict::Follow(2700.0)
        );
    }

    #[test]
    fn an_old_sent_value_expires_and_then_counts_as_a_foreign_edit() {
        let mut follower = AxisFollower::default();
        follower.on_fact(5600.0, false, at(0), &RULES);
        follower.note_sent(4000.0, at(10), &RULES);
        // Факты так и не подтвердили 4000, прошло больше срока; позже кнопка света ставит ровно 4000.
        assert_eq!(
            follower.on_fact(4000.0, false, at(1000), &RULES),
            FollowVerdict::Follow(4000.0)
        );
    }

    #[test]
    fn memory_is_bounded_and_equal_values_in_a_row_are_not_stored_twice() {
        let mut follower = AxisFollower::default();
        for step in 0..10 {
            follower.note_sent(f64::from(step), at(i64::from(step)), &RULES);
        }
        assert_eq!(follower.pending_len(), 4);
        follower.note_sent(9.0, at(11), &RULES);
        assert_eq!(follower.pending_len(), 4);
    }
}
