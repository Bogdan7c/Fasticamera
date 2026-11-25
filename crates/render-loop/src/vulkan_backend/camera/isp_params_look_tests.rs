//! Вид картинки в кэше параметров (решение 178): входит в ключ пересчёта по ЗНАЧЕНИЯМ, а не по указателю.

use std::sync::Arc;

use look_contract::{LookKnob, LookSettings, ToneBaseModel};

use super::tests::{DAYLIGHT, calibration, frame, plain, stream};
use super::*;

fn warm_look() -> Arc<LookSettings> {
    Arc::new(LookSettings {
        warmth: LookKnob::new(0.5).expect("ручка в диапазоне"),
        ..LookSettings::NEUTRAL
    })
}

fn resolved() -> ParamsDecision {
    ParamsDecision {
        hints: HintsSource::ThisFrame,
        outcome: ParamsOutcome::Resolved,
    }
}

fn unchanged() -> ParamsDecision {
    ParamsDecision {
        hints: HintsSource::ThisFrame,
        outcome: ParamsOutcome::Unchanged,
    }
}

#[test]
fn a_new_look_recomputes_params_on_the_next_frame_and_a_repeat_does_not() {
    let stream = stream(calibration());
    let frame = frame(&stream, plain());
    let mut cache = IspParamsCache::default();
    assert_eq!(cache.prepare(&frame, DAYLIGHT, DisplayGamut::Srgb), resolved());
    assert_eq!(
        cache.prepare(&frame, DAYLIGHT, DisplayGamut::Srgb),
        unchanged(),
        "без смены вида лишнего пересчёта нет"
    );

    assert_eq!(cache.set_look(warm_look()), LookChange::Changed);
    assert_eq!(
        cache.prepare(&frame, DAYLIGHT, DisplayGamut::Srgb),
        resolved(),
        "смена вида пересчитывает параметры на следующем кадре"
    );
    assert_eq!(cache.look(), warm_look().as_ref());

    // Те же значения в другом `Arc` (повторная команда экрана): вид не изменился, пересчёта нет.
    assert_eq!(cache.set_look(warm_look()), LookChange::Unchanged);
    assert_eq!(cache.prepare(&frame, DAYLIGHT, DisplayGamut::Srgb), unchanged());
}

#[test]
fn returning_to_the_previous_look_recomputes_again() {
    let stream = stream(calibration());
    let frame = frame(&stream, plain());
    let mut cache = IspParamsCache::default().with_look(warm_look());
    cache.prepare(&frame, DAYLIGHT, DisplayGamut::Srgb);

    assert_eq!(cache.set_look(Arc::new(LookSettings::NEUTRAL)), LookChange::Changed);
    assert_eq!(cache.prepare(&frame, DAYLIGHT, DisplayGamut::Srgb), resolved());
    assert!(cache.look().is_neutral());
}

#[test]
fn the_look_given_at_construction_acts_from_the_first_frame() {
    let stream = stream(calibration());
    let mut cache = IspParamsCache::default().with_look(warm_look());

    assert_eq!(cache.look(), warm_look().as_ref(), "вид действует ещё до первого кадра");
    assert_eq!(
        cache.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::Srgb),
        resolved()
    );
    assert_eq!(cache.look(), warm_look().as_ref(), "первый кадр не сбросил вид");
}

/// Параметры кадра `frame` под вид `look` (видоискатель, sRGB).
fn params_with(look: Arc<LookSettings>) -> IspParams {
    let stream = stream(calibration());
    let mut cache = IspParamsCache::default().with_look(look);
    assert_eq!(
        cache.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::Srgb),
        resolved()
    );
    cache.current().cloned().expect("параметры посчитаны")
}

#[test]
fn neutral_look_gives_exactly_the_params_of_the_honest_base() {
    // «Честный» (решение 98, инвариант 2): ручки в нуле — ровно параметры основы без вида, стадия цвета пропущена.
    let stream = stream(calibration());
    let mut base_only = IspParamsCache::default();
    base_only.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::Srgb);
    let neutral = params_with(Arc::new(LookSettings::NEUTRAL));

    assert_eq!(Some(&neutral), base_only.current());
    assert!(neutral.look.color.is_none(), "нейтральный вид не включает стадию цвета");
}

#[test]
fn the_look_really_reaches_the_frame_params_and_spares_metering() {
    let neutral = params_with(Arc::new(LookSettings::NEUTRAL));

    // Теплота — сдвиг белой точки для рисования: другие усиления баланса, но замер AE на честной нейтрали.
    let warm = params_with(warm_look());
    assert_ne!(
        warm.white_balance, neutral.white_balance,
        "теплота меняет усиления рисования"
    );
    assert_eq!(
        warm.scene_luminance_weights(),
        neutral.scene_luminance_weights(),
        "AE не видит вид (инвариант 1)"
    );

    // Насыщенность — стадия цвета вида включена и несёт множитель.
    let saturated = params_with(Arc::new(LookSettings {
        saturation: LookKnob::new(0.8).expect("ручка в диапазоне"),
        ..LookSettings::NEUTRAL
    }));
    let color = saturated
        .look
        .color
        .as_ref()
        .expect("насыщенность включает стадию цвета");
    assert!(
        color.saturation_multiplier > 1.0,
        "+насыщенность — множитель цветности больше 1"
    );

    // Контраст — стадия тона вида: включает её и несёт наклон, а основу и стадию цвета не меняет.
    let contrasty = params_with(Arc::new(LookSettings {
        contrast: LookKnob::new(1.0).expect("ручка в диапазоне"),
        ..LookSettings::NEUTRAL
    }));
    assert_eq!(
        contrasty.tone_base, neutral.tone_base,
        "контраст — стадия тона вида, основу не меняет"
    );
    assert!(neutral.look.tone.is_none(), "нейтральные ручки — стадии тона нет");
    let tone = contrasty
        .look
        .tone
        .as_ref()
        .expect("контраст включает стадию тона вида");
    assert!(tone.contrast_slope > 1.0, "+контраст — наклон на сером больше 1");
    assert_eq!(
        (tone.shadow_slope, tone.highlight_slope, tone.midtone_slope),
        (1.0, 1.0, 1.0),
        "остальные три ручки тона не тронуты"
    );
    assert!(contrasty.look.color.is_none(), "контраст не включает стадию цвета");
    assert_eq!(
        contrasty.scene_luminance_weights(),
        neutral.scene_luminance_weights(),
        "AE не видит тон вида"
    );
}

/// Каждая модель основы даёт свои разрешённые параметры: смена модели пересчитывает их ровно один раз, повтор той же
/// модели — нет, возврат — снова пересчёт; подсказки кадров остаются (кадр без метаданных не теряет параметры).
#[test]
fn a_new_tone_base_model_recomputes_params_once_and_a_repeat_does_not() {
    use isp_model::tone::base::ResolvedToneBase;
    let stream = stream(calibration());
    let frame = frame(&stream, plain());
    let mut cache = IspParamsCache::default();
    assert_eq!(cache.tone_base_model(), ToneBaseModel::Agx, "умолчание конфига");
    assert_eq!(cache.prepare(&frame, DAYLIGHT, DisplayGamut::Srgb), resolved());
    assert!(matches!(
        cache.current().expect("параметры").tone_base,
        ResolvedToneBase::Agx(_)
    ));

    assert_eq!(cache.set_tone_base(ToneBaseModel::Aces), LookChange::Changed);
    assert_eq!(cache.prepare(&frame, DAYLIGHT, DisplayGamut::Srgb), resolved());
    assert!(matches!(
        cache.current().expect("параметры").tone_base,
        ResolvedToneBase::Aces(_)
    ));
    assert_eq!(cache.set_tone_base(ToneBaseModel::Aces), LookChange::Unchanged);
    assert_eq!(cache.prepare(&frame, DAYLIGHT, DisplayGamut::Srgb), unchanged());

    assert_eq!(cache.set_tone_base(ToneBaseModel::Legacy), LookChange::Changed);
    assert_eq!(cache.prepare(&frame, DAYLIGHT, DisplayGamut::Srgb), resolved());
    assert!(matches!(
        cache.current().expect("параметры").tone_base,
        ResolvedToneBase::Legacy
    ));
}

/// Смена основы не трогает вид, а смена вида — основу; стартовая основа действует с первого кадра.
#[test]
fn the_tone_base_and_the_look_are_independent_inputs() {
    use isp_model::tone::base::ResolvedToneBase;
    let stream = stream(calibration());
    let mut cache = IspParamsCache::default()
        .with_look(warm_look())
        .with_tone_base_model(ToneBaseModel::Filmic);
    assert_eq!(
        cache.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::Srgb),
        resolved()
    );
    assert!(matches!(
        cache.current().expect("параметры").tone_base,
        ResolvedToneBase::Filmic(_)
    ));
    assert_eq!(cache.look(), warm_look().as_ref());

    cache.set_tone_base(ToneBaseModel::Agx);
    assert_eq!(cache.look(), warm_look().as_ref(), "смена основы не сбросила вид");
    cache.set_look(Arc::new(LookSettings::NEUTRAL));
    assert_eq!(
        cache.tone_base_model(),
        ToneBaseModel::Agx,
        "смена вида не сбросила основу"
    );
}

/// Основа любой модели берётся из секций конфига, а не из выбранной `base`: настройки ACES из конфига доходят до
/// параметров, даже когда конфиг выбрал AgX.
#[test]
fn the_switched_base_uses_its_own_section_of_the_tone_config() {
    use isp_model::tone::base::ResolvedToneBase;
    let stream = stream(calibration());
    let defaults = ViewfinderToneConfig::default();
    let custom = ViewfinderToneConfig {
        aces: isp_model::tone::aces::AcesSettings {
            glow_gain: 0.0,
            ..defaults.aces
        },
        ..defaults
    };
    let resolve_aces = |tone: ViewfinderToneConfig| {
        let mut cache = IspParamsCache::default().with_tone(tone);
        cache.set_tone_base(ToneBaseModel::Aces);
        cache.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::Srgb);
        match &cache.current().expect("параметры").tone_base {
            ResolvedToneBase::Aces(resolved) => resolved.clone(),
            other => panic!("ожидалась ACES: {other:?}"),
        }
    };
    assert_ne!(
        resolve_aces(custom),
        resolve_aces(defaults),
        "секция ACES конфига не дошла"
    );
}
