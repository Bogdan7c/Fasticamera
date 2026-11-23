//! Тесты исполнителя намерений вида: намерение → команда `SetLook` рендеру с теми же значениями, ошибка → счётчик
//! её вида и никакой команды, изменение → просьба записи, итоги записи → счётчики. Порт рендера и хранилище подставные.

use look_contract::{BuiltinPreset, LookIntent, PresetId, PresetName};
use look_presets::{PresetStoreConfig, SaveOutcome, SaveRequestOutcome};
use render_loop::viewfinder::ViewfinderCommand;

use super::*;
use crate::gpu_test_support::RecordingRenderControl;
use crate::look_test_support::{StubPersistence, contrast_look, handle_with};
use crate::viewfinder_control::RenderAdjustment;

/// Порт рендера с заданным итогом доставки.
struct FixedDelivery(fn() -> ControlDelivery);

impl RenderControl for FixedDelivery {
    fn adjust(&self, _adjustment: RenderAdjustment) -> ControlDelivery {
        (self.0)()
    }

    fn ui_overlay(&self, _command: render_loop::ui_overlay::UiOverlayCommand) -> ControlDelivery {
        (self.0)()
    }
}

fn set_looks(render: &RecordingRenderControl) -> Vec<LookSettings> {
    render
        .recorded()
        .into_iter()
        .filter_map(|adjustment| match adjustment {
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetLook(look)) => Some((*look).clone()),
            _ => None,
        })
        .collect()
}

fn effects_with(stub: &StubPersistence) -> LookEffects {
    LookEffects::new(Some(handle_with(stub.clone())))
}

#[test]
fn a_look_intent_reaches_the_render_as_set_look_with_exactly_those_values() {
    let stub = StubPersistence::accepting();
    let mut effects = effects_with(&stub);
    let render = RecordingRenderControl::new();

    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), render.as_ref());
    effects.run_intent(LookIntent::SetLook(contrast_look(-0.25)), render.as_ref());

    assert_eq!(set_looks(&render), vec![contrast_look(0.5), contrast_look(-0.25)]);
    let counters = effects.counters();
    assert_eq!((counters.look_changed, counters.render.delivered), (2, 2));
}

#[test]
fn a_change_asks_for_a_file_write_and_the_write_outcomes_are_counted_on_polling() {
    let stub = StubPersistence::accepting();
    let mut effects = effects_with(&stub);
    let render = RecordingRenderControl::new();

    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), render.as_ref());

    assert_eq!(stub.requested().len(), 1, "просьба записи ушла");
    assert_eq!(effects.counters().save_requests.queued, 1);
    assert_eq!(
        effects.counters().writes,
        LookWriteCounters::default(),
        "итогов ещё нет"
    );

    stub.push_outcome(SaveOutcome::Saved);
    stub.push_outcome(SaveOutcome::Failed(look_contract::PresetError::Storage(
        look_contract::StorageFailure::WriteFailed("диск полон".to_owned()),
    )));
    effects.poll_save_outcomes();

    assert_eq!(
        effects.counters().writes,
        LookWriteCounters {
            saved: 1,
            failed: 1,
            ..LookWriteCounters::default()
        }
    );
}

#[test]
fn a_rejected_intent_counts_its_own_kind_and_sends_nothing() {
    let stub = StubPersistence::accepting();
    let mut effects = effects_with(&stub);
    let render = RecordingRenderControl::new();
    let honest = PresetStoreConfig::default().builtin_names.honest;

    effects.run_intent(
        LookIntent::SaveAsNewPreset(PresetName::new(&honest).expect("имя")),
        render.as_ref(),
    );
    effects.run_intent(
        LookIntent::SelectPreset(PresetId::User(look_contract::UserPresetId(404))),
        render.as_ref(),
    );
    effects.run_intent(
        LookIntent::OverwritePreset(look_contract::UserPresetId(404)),
        render.as_ref(),
    );

    let counters = effects.counters();
    assert_eq!(
        counters.rejected,
        LookRejectionCounters {
            name_taken: 1,
            not_found: 2,
            builtin_is_read_only: 0,
            storage: 0,
        }
    );
    assert!(render.recorded().is_empty(), "ошибка не отправляет команду");
    assert!(stub.requested().is_empty(), "и не просит запись");
    assert_eq!(counters.look_changed, 0);
}

#[test]
fn selecting_the_other_builtin_preset_sends_its_values_not_the_old_ones() {
    let stub = StubPersistence::accepting();
    let mut effects = effects_with(&stub);
    let render = RecordingRenderControl::new();

    effects.run_intent(
        LookIntent::SelectPreset(PresetId::Builtin(BuiltinPreset::Honest)),
        render.as_ref(),
    );

    assert_eq!(set_looks(&render), vec![LookSettings::NEUTRAL], "«Честный» — нейтраль");
    assert_eq!(effects.current_look(), LookSettings::NEUTRAL);
}

#[test]
fn an_intent_that_changes_nothing_is_counted_as_unchanged() {
    let stub = StubPersistence::accepting();
    let mut effects = effects_with(&stub);
    let render = RecordingRenderControl::new();
    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), render.as_ref());

    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), render.as_ref());

    let counters = effects.counters();
    assert_eq!((counters.look_changed, counters.unchanged), (1, 1));
    assert_eq!(set_looks(&render).len(), 1, "повтор рендеру не уходит");
    assert_eq!(stub.requested().len(), 1, "и файл не переписывается");
}

#[test]
fn a_stored_state_change_counts_separately_and_sends_nothing_to_the_render() {
    let stub = StubPersistence::accepting();
    let mut effects = effects_with(&stub);
    let render = RecordingRenderControl::new();
    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), render.as_ref());
    effects.run_intent(
        LookIntent::SaveAsNewPreset(PresetName::new("Mine").expect("имя")),
        render.as_ref(),
    );
    let looks_before = set_looks(&render).len();
    let stored_before = effects.counters().stored_state_only;

    // Переименование пресета: картинка та же, файл другой.
    let PresetId::User(user) = handle_active(&effects) else {
        panic!("активным должен быть новый пресет")
    };
    effects.run_intent(
        LookIntent::RenamePreset {
            preset: user,
            name: PresetName::new("Renamed").expect("имя"),
        },
        render.as_ref(),
    );

    assert_eq!(effects.counters().stored_state_only, stored_before + 1);
    assert_eq!(set_looks(&render).len(), looks_before, "рендер не трогали");
}

fn handle_active(effects: &LookEffects) -> PresetId {
    effects.handle.as_ref().expect("библиотека есть").active_look().preset
}

/// Рендер отказал; библиотека и файл при этом обязаны принять вид. Возвращает счётчики исполнителя.
fn counters_after_render_refusal(delivery: &FixedDelivery) -> LookCounters {
    let stub = StubPersistence::accepting();
    let mut effects = effects_with(&stub);

    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), delivery);

    assert_eq!(effects.current_look(), contrast_look(0.5), "библиотека вид приняла");
    assert_eq!(stub.requested().len(), 1, "файл записывается и без доставки рендеру");
    effects.counters()
}

#[test]
fn a_finished_render_thread_is_counted_as_gone() {
    let counters = counters_after_render_refusal(&FixedDelivery(|| ControlDelivery::RenderGone));

    assert_eq!(
        counters.render,
        LookRenderCounters {
            render_gone: 1,
            ..LookRenderCounters::default()
        }
    );
}

#[test]
fn an_unavailable_render_is_counted_as_unavailable() {
    let counters = counters_after_render_refusal(&FixedDelivery(|| ControlDelivery::RenderUnavailable {
        reason: "нет GPU".to_owned(),
    }));

    assert_eq!(
        counters.render,
        LookRenderCounters {
            render_unavailable: 1,
            ..LookRenderCounters::default()
        }
    );
}

#[test]
fn without_a_library_intents_are_counted_and_dropped_and_the_look_is_neutral() {
    let mut effects = LookEffects::new(None);
    let render = RecordingRenderControl::new();

    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), render.as_ref());
    effects.poll_save_outcomes();

    assert_eq!(effects.counters().without_library, 1);
    assert!(render.recorded().is_empty());
    assert_eq!(effects.current_look(), LookSettings::NEUTRAL);
}

#[test]
fn a_closed_writer_is_counted_but_the_look_still_reaches_the_render() {
    let mut stub = StubPersistence::accepting();
    stub.answer = SaveRequestOutcome::ReplacedPending;
    let mut effects = effects_with(&stub);
    let render = RecordingRenderControl::new();

    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), render.as_ref());

    assert_eq!(effects.counters().save_requests.replaced_pending, 1);
    assert_eq!(set_looks(&render).len(), 1);
}
