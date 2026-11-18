//! Общее решение «считать ли статистику на этом кадре» для всех её потребителей (AWB и AE).
//!
//! Стадия статистики стоит заметного тока (≈ 55 мА из ≈ 119 мА ISP, 07a), а считается она **одна на двоих**:
//! плитки сумм до баланса белого нужны и AWB (цвет света), и AE (яркость сцены). Поэтому решение принимает
//! один владелец — эта функция, — а потребители лишь говорят, чего хотят:
//! * AWB отдаёт готовый [`StatisticsPlan`] своего расписания (каждый N-й кадр, на холодном старте — каждый);
//! * AE говорит [`StatisticsDemand`]: «на каждом кадре» (у него ещё нет первой оценки сцены) или «как придётся»
//!   (довольствуется кадрами по расписанию AWB).
//!
//! Итог — [`StatisticsDecision`]: считать ли статистику на GPU и кому из потребителей отдать результат. AWB
//! получает только те кадры, которые просил сам: так форсирование ради AE не меняет его поведение.

use super::white_balance::StatisticsPlan;

/// Чего хочет AE от статистики на этом кадре.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatisticsDemand {
    /// Нужна на каждом кадре (холодный старт замера): оценка сцены ещё не получена.
    EveryFrame,
    /// Довольствуется кадрами, на которых статистику считают по расписанию AWB.
    WhenScheduled,
}

/// Решение для одного кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatisticsDecision {
    /// Включать ли стадию статистики в проход ISP этого кадра (хоть один потребитель просит).
    pub gpu: StatisticsPlan,
    /// Что просил AWB (по его расписанию). Результат ему отдаётся только при `Compute`.
    awb: StatisticsPlan,
}

impl StatisticsDecision {
    /// Объединяет потребности. `ae` — `None`, если AE на потоке рендера выключен.
    pub fn combine(awb: StatisticsPlan, ae: Option<StatisticsDemand>) -> Self {
        let ae_forces = ae == Some(StatisticsDemand::EveryFrame);
        let gpu = if awb == StatisticsPlan::Compute || ae_forces {
            StatisticsPlan::Compute
        } else {
            StatisticsPlan::Skip
        };
        Self { gpu, awb }
    }

    /// Отдавать ли посчитанную статистику AWB (он просил этот кадр сам).
    pub fn awb_receives(self) -> bool {
        self.awb == StatisticsPlan::Compute
    }

    /// Отдавать ли посчитанную статистику AE: ему годится любой посчитанный кадр.
    pub fn ae_receives(self) -> bool {
        self.gpu == StatisticsPlan::Compute
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_ae_the_awb_plan_decides_alone() {
        for awb in [StatisticsPlan::Compute, StatisticsPlan::Skip] {
            let decision = StatisticsDecision::combine(awb, None);
            assert_eq!(decision.gpu, awb);
            assert_eq!(decision.awb_receives(), awb == StatisticsPlan::Compute);
        }
    }

    #[test]
    fn ae_cold_start_forces_statistics_but_awb_does_not_receive_the_extra_frame() {
        let decision = StatisticsDecision::combine(StatisticsPlan::Skip, Some(StatisticsDemand::EveryFrame));
        assert_eq!(decision.gpu, StatisticsPlan::Compute);
        assert!(decision.ae_receives());
        assert!(
            !decision.awb_receives(),
            "AWB не просил этот кадр: его расписание не меняется"
        );
    }

    #[test]
    fn a_relaxed_ae_rides_on_the_awb_schedule() {
        let skipped = StatisticsDecision::combine(StatisticsPlan::Skip, Some(StatisticsDemand::WhenScheduled));
        assert_eq!((skipped.gpu, skipped.ae_receives()), (StatisticsPlan::Skip, false));
        let scheduled = StatisticsDecision::combine(StatisticsPlan::Compute, Some(StatisticsDemand::WhenScheduled));
        assert_eq!(
            (scheduled.gpu, scheduled.ae_receives(), scheduled.awb_receives()),
            (StatisticsPlan::Compute, true, true)
        );
    }
}
