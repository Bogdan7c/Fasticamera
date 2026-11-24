//! Вид картинки (сессия 14a, решения 98, 176, 178) на ПИКСЕЛЯХ через НАСТОЯЩИЙ бэкенд показа: RADV, headless-показ,
//! полная validation, подставная камера (стенд `viewfinder_rig`, тот же, что у `vulkan_viewfinder_content.rs`).
//!
//! Доказывается: команда `SetLook` меняет то, что нарисовал видоискатель, и новая картинка совпадает с эталоном
//! `isp-model` с ЭТИМ видом; нейтральный вид возвращает нейтральный эталон; вид не влияет на то, что видят AWB
//! (инвариант 1); основы тона `aces` и `filmic` из конфига доходят до кадра так же, как `agx`.
//! Снимок с видом момента нажатия — в `vulkan_still_look.rs`.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod support;
mod viewfinder_rig;
mod vulkan_support;

use std::sync::Arc;

use look_contract::{BandAdjustment, BandAdjustments, HueBandId, LookKnob, LookSettings, ToneBaseModel};
use render_loop::backend::DisplayBackend;
use render_loop::config::{
    DisplayColorConfig, RenderLoopConfig, ToneBaseChoice, ViewfinderConfig, ViewfinderToneConfig,
};
use render_loop::viewfinder::{ViewfinderCommand, ViewfinderCommandOutcome};
use viewfinder_rig::{
    CellValues, TOLERANCE, ViewfinderBench, assert_image_color, neutral_gray, reference_color_with_look,
};

/// Ручка в диапазоне (тест не проверяет границы, их проверяет `look-contract`).
fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).expect("ручка в диапазоне")
}

/// Конфиг с sRGB-выходом (эталон однозначен) и выбранной основой тона.
fn config_with_base(base: ToneBaseChoice) -> RenderLoopConfig {
    RenderLoopConfig {
        display_color: DisplayColorConfig::SrgbOnly,
        viewfinder: ViewfinderConfig {
            tone: ViewfinderToneConfig {
                base,
                ..ViewfinderToneConfig::default()
            },
            ..ViewfinderConfig::default()
        },
        ..RenderLoopConfig::default()
    }
}

/// Синеватый объект: насыщенный, чтобы полоса «blue» и насыщенность его заметно меняли.
fn bluish() -> CellValues {
    CellValues {
        red: 220,
        green: 300,
        blue: 620,
    }
}

/// Умеренно цветной тёплый объект: внутри охвата sRGB, поэтому вид и основа тона заметны без обрезки каналов.
fn muted_warm() -> CellValues {
    CellValues {
        red: 420,
        green: 300,
        blue: 200,
    }
}

/// Вид «насыщенность и полоса blue»: +насыщенность, оттенок полосы сдвинут, яркость полосы приподнята.
fn saturated_blue_band_look() -> LookSettings {
    let blue = HueBandId::new("blue").expect("идентификатор полосы");
    LookSettings {
        saturation: knob(0.6),
        bands: BandAdjustments::new(vec![BandAdjustment {
            band: blue,
            hue: knob(0.7),
            saturation: knob(0.4),
            luminance: knob(0.3),
        }])
        .expect("набор полос"),
        ..LookSettings::NEUTRAL
    }
}

/// Наибольшая разность каналов двух цветов.
fn color_distance(first: [f32; 3], second: [f32; 3]) -> f32 {
    (0..3)
        .map(|channel| (first[channel] - second[channel]).abs())
        .fold(0.0_f32, f32::max)
}

/// Команда вида доходит до рендера и применяется.
fn set_look(bench: &mut ViewfinderBench, look: &LookSettings) {
    assert_eq!(
        bench
            .backend
            .apply_viewfinder_command(ViewfinderCommand::SetLook(Arc::new(look.clone()))),
        ViewfinderCommandOutcome::Applied
    );
}

/// Эталон должен отличаться от нейтрального сильнее допуска сравнения, иначе тест не отличил бы «вид дошёл»
/// от «вид не дошёл».
fn assert_distinguishable(styled: [f32; 3], neutral: [f32; 3], what: &str) {
    let distance = color_distance(styled, neutral);
    assert!(
        distance > 8.0 * TOLERANCE,
        "{what}: эталон вида почти не отличается от нейтрального ({styled:?} против {neutral:?}): тест ничего не докажет"
    );
}

#[test]
fn a_set_look_command_changes_the_shown_picture_to_the_reference_with_that_look_and_neutral_restores_it() {
    let config = config_with_base(ToneBaseChoice::Agx);
    let Some(mut bench) = ViewfinderBench::start(&config, bluish(), 8) else {
        return;
    };
    let tone = config.viewfinder.tone;
    let look = saturated_blue_band_look();
    let neutral_reference = reference_color_with_look(&bench.passport, bluish(), &tone, &LookSettings::NEUTRAL);
    let styled_reference = reference_color_with_look(&bench.passport, bluish(), &tone, &look);
    assert_distinguishable(styled_reference, neutral_reference, "насыщенность + полоса blue");

    // 1. Нейтральный вид: картинка совпадает с нейтральным эталоном.
    bench.show_next_frame();
    assert_image_color(&bench.backend, neutral_reference, "нейтральный вид");
    let resolved_neutral = bench
        .backend
        .diagnostics()
        .viewfinder
        .expect("видоискатель")
        .counters
        .params_resolved;

    // 2. SetLook: следующий кадр рисуется с видом и совпадает с эталоном ЭТОГО вида.
    set_look(&mut bench, &look);
    bench.show_next_frame();
    assert_image_color(&bench.backend, styled_reference, "вид: насыщенность и полоса blue");
    let resolved_styled = bench
        .backend
        .diagnostics()
        .viewfinder
        .expect("видоискатель")
        .counters
        .params_resolved;
    assert_eq!(
        resolved_styled,
        resolved_neutral + 1,
        "смена вида пересчитала параметры ровно один раз"
    );

    // 3. Тот же вид повторно: ничего не меняется, картинка та же.
    assert_eq!(
        bench
            .backend
            .apply_viewfinder_command(ViewfinderCommand::SetLook(Arc::new(look.clone()))),
        ViewfinderCommandOutcome::AlreadyInEffect
    );
    bench.show_next_frame();
    assert_image_color(&bench.backend, styled_reference, "вид без изменений");

    // 4. Возврат к нейтрали: снова нейтральный эталон.
    set_look(&mut bench, &LookSettings::NEUTRAL);
    bench.show_next_frame();
    assert_image_color(&bench.backend, neutral_reference, "возврат к нейтрали");
    bench.finish();
}

#[test]
fn a_warmth_look_warms_the_shown_picture_like_the_reference_and_does_not_touch_what_awb_sees() {
    let config = config_with_base(ToneBaseChoice::Agx);
    let warm = LookSettings {
        warmth: knob(0.7),
        ..LookSettings::NEUTRAL
    };
    let tone = config.viewfinder.tone;

    // Прогон без вида: эталон и то, что видит AWB.
    let Some(mut quiet) = ViewfinderBench::start(&config, neutral_gray(), 4) else {
        return;
    };
    let neutral_reference = reference_color_with_look(&quiet.passport, neutral_gray(), &tone, &LookSettings::NEUTRAL);
    quiet.show_next_frame();
    quiet.show_next_frame();
    assert_image_color(&quiet.backend, neutral_reference, "серый без вида");
    let quiet_white = quiet
        .backend
        .diagnostics()
        .viewfinder
        .expect("видоискатель")
        .white_balance;
    quiet.finish();

    // Прогон с видом «теплота»: картинка теплее и совпадает с эталоном, AWB видит ту же честную сцену.
    let Some(mut styled) = ViewfinderBench::start(&config, neutral_gray(), 4) else {
        return;
    };
    set_look(&mut styled, &warm);
    let warm_reference = reference_color_with_look(&styled.passport, neutral_gray(), &tone, &warm);
    assert_distinguishable(warm_reference, neutral_reference, "теплота");
    assert!(
        warm_reference[0] > warm_reference[2] + 0.02,
        "эталон «+теплота» теплее нейтрального серого: {warm_reference:?}"
    );
    styled.show_next_frame();
    styled.show_next_frame();
    assert_image_color(&styled.backend, warm_reference, "вид: теплота");
    let styled_white = styled
        .backend
        .diagnostics()
        .viewfinder
        .expect("видоискатель")
        .white_balance;
    styled.finish();

    assert!(quiet_white.estimates > 0, "AWB мерил кадры: {quiet_white:?}");
    assert_eq!(styled_white.estimates, quiet_white.estimates, "то же число оценок AWB");
    assert_eq!(
        styled_white.applied_white, quiet_white.applied_white,
        "белая сцены у AWB одна и та же с видом и без него: статистика видит честную базу"
    );
    assert_eq!(
        styled_white.last_estimate, quiet_white.last_estimate,
        "последняя оценка AWB та же"
    );
}

#[test]
fn tone_bases_aces_and_filmic_reach_the_shown_picture_with_and_without_a_look() {
    let look = LookSettings {
        saturation: knob(0.5),
        contrast: knob(0.3),
        ..LookSettings::NEUTRAL
    };
    let mut seen_neutral = Vec::new();
    for base in [ToneBaseChoice::Agx, ToneBaseChoice::Aces, ToneBaseChoice::Filmic] {
        let config = config_with_base(base);
        let Some(mut bench) = ViewfinderBench::start(&config, muted_warm(), 4) else {
            return;
        };
        let tone = config.viewfinder.tone;
        let neutral_reference = reference_color_with_look(&bench.passport, muted_warm(), &tone, &LookSettings::NEUTRAL);
        let styled_reference = reference_color_with_look(&bench.passport, muted_warm(), &tone, &look);
        assert_distinguishable(styled_reference, neutral_reference, &format!("{base:?} + вид"));

        bench.show_next_frame();
        assert_image_color(&bench.backend, neutral_reference, &format!("основа {base:?}"));
        set_look(&mut bench, &look);
        bench.show_next_frame();
        assert_image_color(&bench.backend, styled_reference, &format!("основа {base:?} с видом"));
        seen_neutral.push((base, neutral_reference));
        bench.finish();
    }
    // Основы действительно разные: иначе «aces дошёл» нельзя отличить от «остался agx».
    for window in seen_neutral.windows(2) {
        assert_distinguishable(
            window[0].1,
            window[1].1,
            &format!("{:?} против {:?}", window[0].0, window[1].0),
        );
    }
}

/// Смена основы на ходу (`SetToneBase`) меняет картинку на эталон НОВОЙ модели при любой основе конфига: конфиг —
/// лишь стартовое умолчание. Параметры пересчитываются ровно один раз на смену, повтор — `AlreadyInEffect`.
#[test]
fn set_tone_base_switches_the_shown_picture_to_the_reference_of_the_new_model() {
    let config = config_with_base(ToneBaseChoice::Agx);
    let Some(mut bench) = ViewfinderBench::start(&config, muted_warm(), 8) else {
        return;
    };
    let tone_for = |base| ViewfinderToneConfig {
        base,
        ..config.viewfinder.tone
    };
    let agx_reference = reference_color_with_look(
        &bench.passport,
        muted_warm(),
        &tone_for(ToneBaseChoice::Agx),
        &LookSettings::NEUTRAL,
    );
    let aces_reference = reference_color_with_look(
        &bench.passport,
        muted_warm(),
        &tone_for(ToneBaseChoice::Aces),
        &LookSettings::NEUTRAL,
    );
    let legacy_reference = reference_color_with_look(
        &bench.passport,
        muted_warm(),
        &tone_for(ToneBaseChoice::Legacy),
        &LookSettings::NEUTRAL,
    );
    assert_distinguishable(aces_reference, agx_reference, "ACES против AgX");
    assert_distinguishable(legacy_reference, aces_reference, "прежняя против ACES");

    bench.show_next_frame();
    assert_image_color(&bench.backend, agx_reference, "стартовая основа конфига — AgX");
    let resolved_before = bench
        .backend
        .diagnostics()
        .viewfinder
        .expect("видоискатель")
        .counters
        .params_resolved;

    for (model, reference, name) in [
        (ToneBaseModel::Aces, aces_reference, "ACES"),
        (ToneBaseModel::Legacy, legacy_reference, "прежняя"),
        (ToneBaseModel::Agx, agx_reference, "возврат к AgX"),
    ] {
        assert_eq!(
            bench
                .backend
                .apply_viewfinder_command(ViewfinderCommand::SetToneBase(model)),
            ViewfinderCommandOutcome::Applied,
            "{name}"
        );
        bench.show_next_frame();
        assert_image_color(&bench.backend, reference, name);
        assert_eq!(
            bench.backend.diagnostics().viewfinder.expect("видоискатель").tone_base,
            model
        );
    }
    let resolved_after = bench
        .backend
        .diagnostics()
        .viewfinder
        .expect("видоискатель")
        .counters
        .params_resolved;
    assert_eq!(
        resolved_after,
        resolved_before + 3,
        "каждая смена основы пересчитала параметры ровно один раз"
    );

    // Повтор той же основы: команда ничего не меняет, параметры не пересчитываются.
    assert_eq!(
        bench
            .backend
            .apply_viewfinder_command(ViewfinderCommand::SetToneBase(ToneBaseModel::Agx)),
        ViewfinderCommandOutcome::AlreadyInEffect
    );
    bench.show_next_frame();
    assert_eq!(
        bench
            .backend
            .diagnostics()
            .viewfinder
            .expect("видоискатель")
            .counters
            .params_resolved,
        resolved_after
    );
    bench.finish();
}
