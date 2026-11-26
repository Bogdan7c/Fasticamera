//! Тесты раздела калибровки: ручки, полосы, основа тона, сброс, приборы, «чистяк», выбор полосы и плашка проблемы.
//! Проверяются наблюдаемые итоги: точные намерения наружу, итог действия и то, что НЕ должно было измениться.

use look_contract::{
    BandAdjustment, BandAdjustments, BandAxis, CleanView, GlobalKnob, LookControl, LookFacts, LookIntent, LookProblem,
    LookProblemKind, LookSettings, PresetError, ToneBaseModel,
};
use scope_contract::{ScopeIntent, ScopeKind, ScopeRequest, VectorscopeAxes};

use crate::action::ScreenAction;
use crate::calibration::CleanViewState;
use crate::effect::Effect;
use crate::look_test_support::{band, knob, look_facts, look_facts_edited, look_intents};
use crate::look_view::ProblemKind;
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::test_support::Screen;

fn settings_with_contrast(value: f32) -> LookSettings {
    LookSettings::NEUTRAL.with_knob(GlobalKnob::Contrast, knob(value))
}

fn scope_request(screen: &Screen, kind: ScopeKind, axes: VectorscopeAxes) -> ScopeRequest {
    let layout = screen.state.calibration().layout().expect("раскладка раздела");
    ScopeRequest {
        kind,
        vectorscope_axes: axes,
        scan: layout.scope_scan(),
        image_size: layout.scope_image_size_for(kind),
    }
}

// ---------------------------------------------------------------- глобальные ручки

#[test]
fn dragging_a_knob_sends_a_point_intent_carrying_only_that_knob() {
    // Факты уже несут правку теплоты: намерение про контраст её не упоминает (применит библиотека поверх своих значений).
    let edited = LookSettings::NEUTRAL.with_knob(GlobalKnob::Warmth, knob(0.2));
    let mut screen = Screen::calibrating(look_facts_edited(edited.clone()));

    let transition = screen.step(ScreenAction::KnobDragged {
        knob: GlobalKnob::Contrast,
        value: knob(0.4),
    });

    assert_eq!(transition.outcome, ActionOutcome::Applied);
    assert_eq!(
        transition.effects,
        [Effect::Look(LookIntent::SetKnob {
            knob: GlobalKnob::Contrast,
            value: knob(0.4)
        })]
    );
}

#[test]
fn two_knob_drags_in_one_batch_before_facts_update_send_two_independent_point_intents() {
    // Два пальца на разных ручках в одном кадре: факты ещё прошлого кадра. Каждое намерение несёт только свою ручку,
    // поэтому второе не затирает первое.
    let mut screen = Screen::calibrating(look_facts());

    let first = screen.step(ScreenAction::KnobDragged {
        knob: GlobalKnob::Contrast,
        value: knob(0.4),
    });
    let second = screen.step(ScreenAction::KnobDragged {
        knob: GlobalKnob::Warmth,
        value: knob(-0.3),
    });

    assert_eq!(
        first.effects,
        [Effect::Look(LookIntent::SetKnob {
            knob: GlobalKnob::Contrast,
            value: knob(0.4)
        })]
    );
    assert_eq!(
        second.effects,
        [Effect::Look(LookIntent::SetKnob {
            knob: GlobalKnob::Warmth,
            value: knob(-0.3)
        })],
        "второе намерение не тащит за собой старое значение контраста"
    );
}

#[test]
fn two_band_axis_drags_in_one_batch_send_independent_point_intents() {
    let mut screen = Screen::calibrating(look_facts());

    let first = screen.step(ScreenAction::BandKnobDragged {
        band: band("sky"),
        axis: BandAxis::Hue,
        value: knob(0.3),
    });
    let second = screen.step(ScreenAction::BandKnobDragged {
        band: band("red"),
        axis: BandAxis::Saturation,
        value: knob(-0.2),
    });

    assert_eq!(
        first.effects,
        [Effect::Look(LookIntent::SetBandAxis {
            band: band("sky"),
            axis: BandAxis::Hue,
            value: knob(0.3)
        })]
    );
    assert_eq!(
        second.effects,
        [Effect::Look(LookIntent::SetBandAxis {
            band: band("red"),
            axis: BandAxis::Saturation,
            value: knob(-0.2)
        })]
    );
}

#[test]
fn a_knob_drag_changes_neither_the_calibration_choices_nor_the_photo_settings() {
    let mut screen = Screen::calibrating(look_facts());
    let before = screen.state.clone();

    screen.step(ScreenAction::KnobDragged {
        knob: GlobalKnob::Shadows,
        value: knob(-0.5),
    });

    assert_eq!(screen.state.calibration(), before.calibration());
    assert_eq!(screen.state.settings(), before.settings());
    assert_eq!(
        screen.state.look(),
        before.look(),
        "значений ручек экран у себя не копит"
    );
}

#[test]
fn dragging_a_knob_to_its_current_value_changes_nothing() {
    let mut screen = Screen::calibrating(look_facts_edited(settings_with_contrast(0.4)));

    let transition = screen.step(ScreenAction::KnobDragged {
        knob: GlobalKnob::Contrast,
        value: knob(0.4),
    });

    assert_eq!(
        transition.outcome,
        ActionOutcome::NoChange(NoChangeReason::ValueUnchanged)
    );
    assert!(transition.effects.is_empty());
}

#[test]
fn a_knob_drag_without_look_facts_is_rejected_and_counted() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ModeSelected(crate::modes::CaptureMode::Calibrate));

    let transition = screen.step(ScreenAction::KnobDragged {
        knob: GlobalKnob::Contrast,
        value: knob(0.4),
    });

    assert_eq!(transition.outcome, ActionOutcome::Rejected(Rejection::LookFactsMissing));
    assert!(transition.effects.is_empty());
    assert_eq!(screen.state.rejections().get(Rejection::LookFactsMissing), 1);
}

#[test]
fn every_knob_is_accepted_on_every_tone_base() {
    for base in ToneBaseModel::ALL {
        let mut facts = look_facts();
        facts.current.base = base;
        facts.preset = facts.current.clone();
        let mut screen = Screen::calibrating(facts);
        for knob_id in GlobalKnob::ALL {
            let transition = screen.step(ScreenAction::KnobDragged {
                knob: knob_id,
                value: knob(0.3),
            });
            assert_eq!(transition.outcome, ActionOutcome::Applied, "{base:?} / {knob_id:?}");
            assert_eq!(
                transition.effects.len(),
                1,
                "{base:?} / {knob_id:?}: ровно один Effect::Look"
            );
        }
    }
}

#[test]
fn calibration_actions_are_rejected_outside_the_calibration_mode() {
    let mut screen = Screen::with_look(look_facts());
    let actions = [
        ScreenAction::KnobDragged {
            knob: GlobalKnob::Contrast,
            value: knob(0.4),
        },
        ScreenAction::ToneBaseSelected(ToneBaseModel::Aces),
        ScreenAction::ScopeKindSelected(ScopeKind::Vectorscope),
        ScreenAction::VectorscopeAxesSelected(VectorscopeAxes::OklabAb),
        ScreenAction::CleanViewPressed,
        ScreenAction::CleanViewReleased,
        ScreenAction::BandSelected(band("sky")),
        ScreenAction::ResetControlTapped(LookControl::ToneBase),
    ];

    for action in actions {
        let transition = screen.step(action.clone());
        assert_eq!(
            transition.outcome,
            ActionOutcome::Rejected(Rejection::NotInCalibrationMode),
            "{action:?}"
        );
        assert!(transition.effects.is_empty(), "{action:?}");
    }
    assert_eq!(screen.state.rejections().get(Rejection::NotInCalibrationMode), 8);
}

// ---------------------------------------------------------------- полосы

#[test]
fn dragging_a_band_axis_replaces_only_that_axis_of_that_band() {
    // В фактах уже есть правки: red.saturation и sky.luminance. Тянем sky.hue — остальное обязано остаться.
    let bands = BandAdjustments::new(vec![
        BandAdjustment::neutral(band("red")).with_axis(BandAxis::Saturation, knob(0.5)),
        BandAdjustment::neutral(band("sky")).with_axis(BandAxis::Luminance, knob(-0.2)),
    ])
    .expect("полосы");
    let edited = LookSettings {
        bands: bands.clone(),
        ..LookSettings::NEUTRAL
    };
    let mut screen = Screen::calibrating(look_facts_edited(edited));

    let transition = screen.step(ScreenAction::BandKnobDragged {
        band: band("sky"),
        axis: BandAxis::Hue,
        value: knob(0.3),
    });

    assert_eq!(
        transition.effects,
        [Effect::Look(LookIntent::SetBandAxis {
            band: band("sky"),
            axis: BandAxis::Hue,
            value: knob(0.3)
        })]
    );
}

#[test]
fn a_band_drag_to_the_same_value_or_for_an_unknown_band_is_distinguished() {
    let bands = BandAdjustments::new(vec![
        BandAdjustment::neutral(band("sky")).with_axis(BandAxis::Hue, knob(0.3)),
    ])
    .expect("полосы");
    let mut screen = Screen::calibrating(look_facts_edited(LookSettings {
        bands,
        ..LookSettings::NEUTRAL
    }));

    let same = screen.step(ScreenAction::BandKnobDragged {
        band: band("sky"),
        axis: BandAxis::Hue,
        value: knob(0.3),
    });
    let unknown = screen.step(ScreenAction::BandKnobDragged {
        band: band("magenta"),
        axis: BandAxis::Hue,
        value: knob(0.3),
    });

    assert_eq!(same.outcome, ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
    assert_eq!(unknown.outcome, ActionOutcome::Rejected(Rejection::UnknownBand));
    assert!(same.effects.is_empty() && unknown.effects.is_empty());
}

#[test]
fn the_first_band_is_selected_by_default_and_follows_the_band_list() {
    let mut screen = Screen::calibrating(look_facts());
    assert_eq!(screen.state.calibration().selected_band(), Some(&band("red")));

    let select = screen.step(ScreenAction::BandSelected(band("sky")));
    assert_eq!(select.outcome, ActionOutcome::Applied);
    assert!(select.effects.is_empty(), "выбор полосы — дело экрана, наружу ничего");
    assert_eq!(screen.state.calibration().selected_band(), Some(&band("sky")));

    let again = screen.step(ScreenAction::BandSelected(band("sky")));
    assert_eq!(again.outcome, ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
    let unknown = screen.step(ScreenAction::BandSelected(band("magenta")));
    assert_eq!(unknown.outcome, ActionOutcome::Rejected(Rejection::UnknownBand));
    assert_eq!(screen.state.calibration().selected_band(), Some(&band("sky")));

    // Список полос не потерял выбранную — выбор остаётся; потерял — снова первая; полос нет — выбора нет.
    let mut still_there = look_facts();
    still_there.bands.reverse();
    screen.push_look(still_there);
    assert_eq!(screen.state.calibration().selected_band(), Some(&band("sky")));

    let mut sky_gone = look_facts();
    sky_gone.bands.retain(|facts| facts.id != band("sky"));
    screen.push_look(sky_gone);
    assert_eq!(screen.state.calibration().selected_band(), Some(&band("red")));

    let mut no_bands = look_facts();
    no_bands.bands.clear();
    screen.push_look(no_bands);
    assert_eq!(screen.state.calibration().selected_band(), None);
}

// ---------------------------------------------------------------- основа и сброс

#[test]
fn choosing_a_tone_base_sends_it_and_the_same_base_changes_nothing() {
    let mut screen = Screen::calibrating(look_facts());

    let same = screen.step(ScreenAction::ToneBaseSelected(ToneBaseModel::Agx));
    let other = screen.step(ScreenAction::ToneBaseSelected(ToneBaseModel::Aces));

    assert_eq!(same.outcome, ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
    assert_eq!(
        other.effects,
        [Effect::Look(LookIntent::SetToneBase(ToneBaseModel::Aces))]
    );
}

#[test]
fn reset_goes_out_only_for_a_control_that_differs_from_the_preset() {
    let mut facts = look_facts_edited(settings_with_contrast(0.4));
    facts.current.base = ToneBaseModel::Aces;
    let mut screen = Screen::calibrating(facts);

    let knob_reset = screen.step(ScreenAction::ResetControlTapped(LookControl::Knob(
        GlobalKnob::Contrast,
    )));
    let base_reset = screen.step(ScreenAction::ResetControlTapped(LookControl::ToneBase));
    let dim = screen.step(ScreenAction::ResetControlTapped(LookControl::Knob(GlobalKnob::Warmth)));

    assert_eq!(
        knob_reset.effects,
        [Effect::Look(LookIntent::ResetControl(LookControl::Knob(
            GlobalKnob::Contrast
        )))]
    );
    assert_eq!(
        base_reset.effects,
        [Effect::Look(LookIntent::ResetControl(LookControl::ToneBase))]
    );
    assert_eq!(
        dim.outcome,
        ActionOutcome::NoChange(NoChangeReason::ControlMatchesPreset)
    );
    assert!(dim.effects.is_empty());
}

// ---------------------------------------------------------------- приборы

#[test]
fn swiping_to_another_scope_asks_the_renderer_for_exactly_that_scope() {
    let mut screen = Screen::calibrating(look_facts());

    let transition = screen.step(ScreenAction::ScopeKindSelected(ScopeKind::Vectorscope));

    assert_eq!(screen.state.calibration().scope_kind(), ScopeKind::Vectorscope);
    assert_eq!(
        transition.effects,
        [Effect::Scopes(ScopeIntent::Show(scope_request(
            &screen,
            ScopeKind::Vectorscope,
            VectorscopeAxes::YCbCrBt709
        )))]
    );
    // Вектороскоп просит КВАДРАТ, вписанный в зону 540×743 px (меньшая сторона), а не всю зону.
    let Effect::Scopes(ScopeIntent::Show(request)) = &transition.effects[0] else {
        panic!("ожидалась просьба показать прибор: {:?}", transition.effects);
    };
    assert_eq!((request.image_size.width(), request.image_size.height()), (540, 540));
    let same = screen.step(ScreenAction::ScopeKindSelected(ScopeKind::Vectorscope));
    assert_eq!(same.outcome, ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
}

#[test]
fn choosing_the_vectorscope_axes_keeps_the_scope_kind_and_resends_the_request() {
    let mut screen = Screen::calibrating(look_facts());
    screen.step(ScreenAction::ScopeKindSelected(ScopeKind::RgbParade));

    let transition = screen.step(ScreenAction::VectorscopeAxesSelected(VectorscopeAxes::OklabAb));

    assert_eq!(screen.state.calibration().vectorscope_axes(), VectorscopeAxes::OklabAb);
    assert_eq!(screen.state.calibration().scope_kind(), ScopeKind::RgbParade);
    assert_eq!(
        transition.effects,
        [Effect::Scopes(ScopeIntent::Show(scope_request(
            &screen,
            ScopeKind::RgbParade,
            VectorscopeAxes::OklabAb
        )))],
        "оси хранятся и при другом типе прибора (смена свайпом не теряет стандарт)"
    );
}

#[test]
fn the_config_sets_the_default_vectorscope_axes() {
    let mut config = crate::config::ScreenConfig::default();
    config.calibration.vectorscope_axes = VectorscopeAxes::OklabAb;
    let screen = Screen::unprepared(config);

    assert_eq!(screen.state.calibration().vectorscope_axes(), VectorscopeAxes::OklabAb);
    assert_eq!(screen.state.calibration().scope_kind(), ScopeKind::Waveform);
}

// ---------------------------------------------------------------- «чистяк»

#[test]
fn the_clean_view_button_is_idempotent_and_pairs_press_with_release() {
    let mut screen = Screen::calibrating(look_facts());

    let press = screen.step(ScreenAction::CleanViewPressed);
    let press_again = screen.step(ScreenAction::CleanViewPressed);
    let release = screen.step(ScreenAction::CleanViewReleased);
    let release_again = screen.step(ScreenAction::CleanViewReleased);

    assert_eq!(press.effects, [Effect::CleanView(CleanView::ShowBase)]);
    assert_eq!(
        press_again.outcome,
        ActionOutcome::NoChange(NoChangeReason::ValueUnchanged)
    );
    assert!(press_again.effects.is_empty());
    assert_eq!(release.effects, [Effect::CleanView(CleanView::ShowEdited)]);
    assert_eq!(
        release_again.outcome,
        ActionOutcome::NoChange(NoChangeReason::ValueUnchanged)
    );
    assert_eq!(screen.state.calibration().clean_view(), CleanViewState::ShowingEdited);
}

#[test]
fn holding_the_clean_view_does_not_touch_the_look_values() {
    let mut screen = Screen::calibrating(look_facts_edited(settings_with_contrast(0.4)));
    let facts_before = screen.state.look().clone();

    let press = screen.step(ScreenAction::CleanViewPressed);

    assert!(
        look_intents(&press.effects).is_empty(),
        "кнопка не шлёт SetLook сама: это дело исполнителя"
    );
    assert_eq!(screen.state.look(), &facts_before);
}

// ---------------------------------------------------------------- факты и плашка

fn facts_with_problem(serial: u64) -> LookFacts {
    LookFacts {
        problem: Some(LookProblem {
            serial,
            kind: LookProblemKind::Rejected(PresetError::NameTaken),
        }),
        ..look_facts()
    }
}

#[test]
fn a_library_problem_raises_a_banner_once_per_serial_number() {
    let mut screen = Screen::calibrating(look_facts());
    assert!(screen.state.look().banner().is_none());

    screen.push_look(facts_with_problem(1));
    let first = screen.state.look().banner().cloned().expect("плашка");
    assert_eq!(
        first.kind,
        ProblemKind::Look(LookProblemKind::Rejected(PresetError::NameTaken))
    );

    // Те же факты с тем же номером проблемы плашку не воскрешают, даже после закрытия.
    assert_eq!(
        screen
            .step(ScreenAction::ProblemDismissed { number: first.number })
            .outcome,
        ActionOutcome::Applied
    );
    screen.push_look(facts_with_problem(1));
    assert!(screen.state.look().banner().is_none());

    // Новый номер — новая плашка (с новым номером плашки).
    screen.push_look(facts_with_problem(2));
    let second = screen.state.look().banner().cloned().expect("плашка");
    assert_eq!(second.kind, first.kind);
    assert_ne!(second.number, first.number);
}

#[test]
fn dismissing_when_there_is_no_banner_changes_nothing() {
    let mut screen = Screen::calibrating(look_facts());

    let transition = screen.step(ScreenAction::ProblemDismissed { number: 1 });

    assert_eq!(
        transition.outcome,
        ActionOutcome::NoChange(NoChangeReason::NothingToShow)
    );
}

#[test]
fn the_same_look_facts_twice_change_nothing() {
    let mut screen = Screen::calibrating(look_facts());

    let again = screen.step(ScreenAction::LookFactsChanged(Box::new(look_facts())));

    assert_eq!(again.outcome, ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
    assert!(again.effects.is_empty());
}

#[test]
fn dismissing_an_old_banner_does_not_remove_a_newer_one_that_arrived_in_the_same_frame() {
    let mut screen = Screen::calibrating(look_facts());
    screen.push_look(facts_with_problem(1));
    let old = screen.state.look().banner().cloned().expect("плашка");
    // В том же кадре, до того как закрытие старой дошло до редьюсера, пришла новая проблема.
    screen.push_look(facts_with_problem(2));
    let newer = screen.state.look().banner().cloned().expect("новая плашка");
    assert_ne!(old.number, newer.number);

    let stale = screen.step(ScreenAction::ProblemDismissed { number: old.number });

    assert_eq!(stale.outcome, ActionOutcome::NoChange(NoChangeReason::NothingToShow));
    assert_eq!(screen.state.look().banner(), Some(&newer), "новая плашка осталась");

    let current = screen.step(ScreenAction::ProblemDismissed { number: newer.number });
    assert_eq!(current.outcome, ActionOutcome::Applied);
    assert!(screen.state.look().banner().is_none());
}
