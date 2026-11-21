//! Замкнутая симуляция AE со всегда-белым окном в кадре (13c, решения 133, 134): равновесие при любой доле
//! пересвета, компенсация EV ±1 и тёмный старт. Стенд [`Rig`] — общий с `simulation_tests`.

#![allow(clippy::unwrap_used, reason = "в тестах падение — сигнал провала")]

use super::*;

/// Доля веса матрицы у столбцов `columns` (доля пересвета, которую увидит замер).
fn matrix_share_of_columns(columns: &std::ops::Range<u32>) -> f32 {
    let weights = MeteringWeights::build(
        GRID,
        TilePlacement::full_frame(4.0 / 3.0).unwrap(),
        MeteringArea::Matrix,
        &MeteringConfig::default(),
    )
    .unwrap();
    weights
        .weights()
        .iter()
        .enumerate()
        .filter(|(index, _)| columns.contains(&(u32::try_from(*index).unwrap() % GRID.columns)))
        .map(|(_, &weight)| weight)
        .sum::<f32>()
        / weights.weights().iter().sum::<f32>()
}

#[test]
fn an_always_white_window_of_any_share_below_severe_clipping_settles_with_both_averages() {
    // Решение 134: обычное среднее без предохранителя уводило экспозицию в ноль, как только всегда-белое окно
    // давало больше цели. Перебор: окно от ≈ 3 % веса до почти «сильного пересвета», оба способа усреднения.
    let severe = MeteringConfig::default().severe_clipped_fraction;
    let tolerance = f64::from(ExposureControllerConfig::default().settled_tolerance_ev);
    let mut checked_shares = Vec::new();
    for half_width in 1..=13_u32 {
        let window = 16 - half_width..16 + half_width;
        let share = matrix_share_of_columns(&window);
        if share >= severe {
            // От «сильного пересвета» точной оценки нет вовсе, там ограниченный шаг вниз (другое правило).
            continue;
        }
        checked_shares.push(share);
        for average in [SceneAverage::LinearLight, SceneAverage::LogLuminance] {
            let config = MeteringConfig {
                scene_average: average,
                ..MeteringConfig::default()
            };
            let mut rig = Rig::start(ExposureDirective::DEFAULT, 0.01, 0.01).with_metering(config);
            rig.window_columns = Some(window.clone());
            rig.run(600);
            let tail = &rig.log[420..];
            let (low, high) = tail
                .iter()
                .map(|entry| entry.applied.log2())
                .fold((f64::MAX, f64::MIN), |(low, high), value| {
                    (low.min(value), high.max(value))
                });
            assert!(
                high - low <= tolerance,
                "{average:?}, окно {share:.2} веса: экспозиция гуляет на {} EV",
                high - low
            );
            // У обычного среднего «устоялась» — не значит «ушла в ноль»: предохранитель оставляет стену видимой
            // (оценка решения 134: до −1,8 EV при окне 30–40 %). Лог-среднее здесь темнит стену сильнее (до −5 EV
            // при 40 %) — это его прежнее поведение, проверяем у него только отсутствие качелей.
            let wall_ev = (rig.scene_gain * tail[tail.len() - 1].applied / MIDDLE_GRAY).log2();
            if average == SceneAverage::LinearLight {
                assert!(
                    wall_ev > -2.5,
                    "окно {share:.2} веса: стена ушла в темноту на {wall_ev:.2} EV"
                );
            }
        }
    }
    // Перебор действительно прошёл через опасную зону 24–35 % (защита светов уже глубокая, полоса ещё «норма»).
    assert!(
        checked_shares.iter().any(|share| (0.24..0.35).contains(share)),
        "{checked_shares:?}"
    );
    assert!(checked_shares.len() >= 6, "{checked_shares:?}");
}

#[test]
fn ev_compensation_doubles_and_halves_the_exposure_with_an_always_white_window_in_frame() {
    // 13c на телефоне: с пересветом в кадре EV +1 давал ×3,3, EV −1 — ×0,2, потому что замер зависел от выдержки
    // кадра. Здесь движок честно умножает цель на 2^EV; контур обязан встать ровно на ×2 и ×½ к опорной точке.
    // Окно — только в полосе «норма» (< `clipped_fraction_limit`): в полосе частичного пересвета правило «вверх
    // нельзя» сравнивает цель для EV 0 с кадром, снятым при EV −1, и экспозиция уходит в ноль — это старый дефект
    // 08a, он есть и у лог-среднего (найден этим тестом в 13c, см. results/13c.md).
    let limit = MeteringConfig::default().clipped_fraction_limit;
    for half_width in [1_u32, 2, 3, 4] {
        let window = 16 - half_width..16 + half_width;
        let share = matrix_share_of_columns(&window);
        assert!(share < limit, "окно {share:.2} веса вне полосы «норма»");
        let mut rig = Rig::start(ExposureDirective::DEFAULT, 0.01, 0.01);
        rig.window_columns = Some(window);
        let settled_at = |rig: &mut Rig, bias_ev: f64| {
            rig.set_bias_ev(bias_ev);
            rig.run(300);
            rig.log[rig.log.len() - 1].applied
        };
        let baseline = settled_at(&mut rig, 0.0);
        let brighter = settled_at(&mut rig, 1.0) / baseline;
        let darker = settled_at(&mut rig, -1.0) / baseline;
        assert!(
            (brighter - 2.0).abs() < 0.1 && (darker - 0.5).abs() < 0.03,
            "окно {share:.2} веса: EV +1 → ×{brighter:.2}, EV −1 → ×{darker:.2}"
        );
    }
}

#[test]
fn a_dark_start_with_an_always_white_window_converges_to_the_same_exposure_without_overshoot() {
    // Новая формула обычного среднего делит на измеримую часть кадра: у тёмного кадра она у пола, и шаг вверх
    // может быть крупным. Старт на 6 EV темнее с окном в четверть кадра обязан прийти туда же, куда приходит
    // старт от нужной экспозиции, и не перелететь её больше допуска «устоялось».
    let window = 13..19;
    let settle = |initial: f64| {
        let mut rig = Rig::start(ExposureDirective::DEFAULT, initial, 0.01);
        rig.window_columns = Some(window.clone());
        rig.run(600);
        rig
    };
    let reference = settle(0.01);
    let from_dark = settle(0.01 / 64.0);
    let tolerance = f64::from(ExposureControllerConfig::default().settled_tolerance_ev);
    let final_of = |rig: &Rig| rig.log[rig.log.len() - 1].applied;
    let gap_ev = (final_of(&from_dark) / final_of(&reference)).log2();
    assert!(gap_ev.abs() <= tolerance, "пришли не туда: {gap_ev:.2} EV");
    let overshoot_ev = from_dark
        .log
        .iter()
        .map(|entry| (entry.applied / final_of(&reference)).log2())
        .fold(f64::MIN, f64::max);
    assert!(overshoot_ev <= tolerance + 0.1, "перелёт на {overshoot_ev:.2} EV");
}
