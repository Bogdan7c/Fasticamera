//! Синхронизация рендера с библиотекой (решения 189, 195): что и когда уходит рендеру после намерения, как работает
//! чистый вид и как копятся проблемы для экрана. Порт рендера и хранилище подставные; проверяются записанные команды
//! рендеру и состояние исполнителя, а не вызовы.

use look_contract::{
    BuiltinPreset, CleanView, LookIntent, LookProblem, LookProblemKind, LookSettings, PresetError, PresetId,
    PresetName, StorageFailure, ToneBaseModel,
};
use look_presets::{PresetStoreConfig, SaveOutcome, SaveRequestOutcome};
use render_loop::viewfinder::ViewfinderCommand;

use super::*;
use crate::gpu_test_support::RecordingRenderControl;
use crate::look_test_support::{StubPersistence, contrast_look, handle_with};
use crate::viewfinder_control::RenderAdjustment;

/// Порт рендера, который отказывает, пока флаг поднят.
struct Flaky(std::sync::atomic::AtomicBool);

impl RenderControl for Flaky {
    fn adjust(&self, _adjustment: RenderAdjustment) -> ControlDelivery {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) {
            ControlDelivery::RenderGone
        } else {
            ControlDelivery::Delivered
        }
    }

    fn ui_overlay(&self, _command: render_loop::ui_overlay::UiOverlayCommand) -> ControlDelivery {
        ControlDelivery::Delivered
    }
}

/// Команды видоискателю в порядке отправки.
fn viewfinder_commands(render: &RecordingRenderControl) -> Vec<ViewfinderCommand> {
    render
        .recorded()
        .into_iter()
        .filter_map(|adjustment| match adjustment {
            RenderAdjustment::Viewfinder(command) => Some(command),
            _ => None,
        })
        .collect()
}

fn set_look(settings: LookSettings) -> ViewfinderCommand {
    ViewfinderCommand::SetLook(Arc::new(settings))
}

fn effects_with_stub(stub: &StubPersistence) -> LookEffects {
    LookEffects::new(Some(handle_with(stub.clone())))
}

/// Библиотека стартует на встроенной основе из конфига; берём её и «другую», чтобы смена была настоящей.
fn startup_base(effects: &LookEffects) -> ToneBaseModel {
    effects.current_recipe().base
}

fn other_base(base: ToneBaseModel) -> ToneBaseModel {
    if base == ToneBaseModel::Aces {
        ToneBaseModel::Filmic
    } else {
        ToneBaseModel::Aces
    }
}

#[test]
fn a_settings_change_sends_only_set_look_and_a_base_change_sends_only_set_tone_base() {
    let stub = StubPersistence::accepting();
    let mut effects = effects_with_stub(&stub);
    let render = RecordingRenderControl::new();
    let base = startup_base(&effects);
    let changed_base = other_base(base);

    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), render.as_ref());
    assert_eq!(viewfinder_commands(&render), vec![set_look(contrast_look(0.5))]);

    effects.run_intent(LookIntent::SetToneBase(changed_base), render.as_ref());
    assert_eq!(
        viewfinder_commands(&render),
        vec![
            set_look(contrast_look(0.5)),
            ViewfinderCommand::SetToneBase(changed_base)
        ],
        "основа ушла отдельной командой, ручки заново не слались"
    );
    assert_eq!(effects.counters().tone_base_changed, 1);
}

#[test]
fn an_intent_that_changes_both_sends_both_and_one_that_changes_nothing_sends_nothing() {
    let stub = StubPersistence::accepting();
    let mut effects = effects_with_stub(&stub);
    let render = RecordingRenderControl::new();
    let changed_base = other_base(startup_base(&effects));
    effects.run_intent(LookIntent::SetToneBase(changed_base), render.as_ref());
    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), render.as_ref());
    let sent_before = viewfinder_commands(&render).len();

    // Выбор встроенного пресета возвращает и его основу, и его ручки: меняется всё сразу.
    effects.run_intent(
        LookIntent::SelectPreset(PresetId::Builtin(BuiltinPreset::Honest)),
        render.as_ref(),
    );
    let commands = viewfinder_commands(&render);
    assert_eq!(
        commands[sent_before..],
        [
            ViewfinderCommand::SetToneBase(effects.current_recipe().base),
            set_look(effects.current_recipe().settings)
        ],
        "изменились и основа, и ручки: уходят обе команды"
    );

    // Повтор того же значения основы и тех же ручек не меняет ничего.
    let sent_after_select = commands.len();
    effects.run_intent(LookIntent::SetToneBase(effects.current_recipe().base), render.as_ref());
    effects.run_intent(LookIntent::SetLook(effects.current_recipe().settings), render.as_ref());
    assert_eq!(
        viewfinder_commands(&render).len(),
        sent_after_select,
        "ничего не изменилось — ничего не ушло"
    );
}

#[test]
fn an_undelivered_command_is_resent_by_the_next_intent() {
    let stub = StubPersistence::accepting();
    let mut effects = effects_with_stub(&stub);
    let changed_base = other_base(startup_base(&effects));
    let flaky = Flaky(std::sync::atomic::AtomicBool::new(true));

    effects.run_intent(LookIntent::SetToneBase(changed_base), &flaky);
    assert_eq!(effects.counters().render.render_gone, 1);
    assert_eq!(
        effects.counters().tone_base_changed,
        0,
        "недоставленное не считается сменой"
    );

    // Рендер ожил; безобидное намерение (ручки те же, основа та же) всё равно донесёт потерянную основу.
    flaky.0.store(false, std::sync::atomic::Ordering::SeqCst);
    effects.run_intent(LookIntent::SetToneBase(changed_base), &flaky);
    assert_eq!(effects.counters().tone_base_changed, 1);
    assert_eq!(effects.counters().render.delivered, 1);
}

#[test]
fn show_base_sends_neutral_and_keeps_the_base_and_show_edited_returns_the_active_look() {
    let stub = StubPersistence::accepting();
    let mut effects = effects_with_stub(&stub);
    let render = RecordingRenderControl::new();
    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), render.as_ref());
    let sent_before = viewfinder_commands(&render).len();
    let library_before = effects.current_recipe();
    let saves_before = stub.requested().len();

    effects.set_clean_view(CleanView::ShowBase, render.as_ref());
    effects.set_clean_view(CleanView::ShowBase, render.as_ref());

    assert_eq!(
        viewfinder_commands(&render)[sent_before..],
        [set_look(LookSettings::NEUTRAL)],
        "один SetLook(нейтраль), основа не тронута, повтор дубля не даёт"
    );
    assert_eq!(
        effects.current_recipe(),
        library_before,
        "библиотека чистым видом не тронута"
    );
    assert_eq!(stub.requested().len(), saves_before, "ничего не сохраняется");

    effects.set_clean_view(CleanView::ShowEdited, render.as_ref());
    assert_eq!(
        viewfinder_commands(&render)[sent_before + 1..],
        [set_look(contrast_look(0.5))],
        "отпустили: вернулись правленые ручки"
    );
    effects.set_clean_view(CleanView::ShowEdited, render.as_ref());
    assert_eq!(
        viewfinder_commands(&render).len(),
        sent_before + 2,
        "повторный ShowEdited — без команд"
    );
}

#[test]
fn while_the_base_is_shown_a_base_change_is_forwarded_at_once_but_knob_changes_wait_for_the_release() {
    let stub = StubPersistence::accepting();
    let mut effects = effects_with_stub(&stub);
    let render = RecordingRenderControl::new();
    let changed_base = other_base(startup_base(&effects));
    effects.set_clean_view(CleanView::ShowBase, render.as_ref());
    let sent_before = viewfinder_commands(&render).len();

    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), render.as_ref());
    assert_eq!(
        viewfinder_commands(&render).len(),
        sent_before,
        "ручки при зажатой «основе» рендеру не уходят"
    );

    effects.run_intent(LookIntent::SetToneBase(changed_base), render.as_ref());
    assert_eq!(
        viewfinder_commands(&render)[sent_before..],
        [ViewfinderCommand::SetToneBase(changed_base)],
        "смена основы видна сразу"
    );

    effects.run_intent(LookIntent::SetLook(contrast_look(-0.25)), render.as_ref());
    effects.set_clean_view(CleanView::ShowEdited, render.as_ref());
    assert_eq!(
        viewfinder_commands(&render)[sent_before + 1..],
        [set_look(contrast_look(-0.25))],
        "на отпускании уходят ПОСЛЕДНИЕ активные ручки, а не промежуточные"
    );
}

#[test]
fn a_clean_view_without_a_library_sends_nothing() {
    let mut effects = LookEffects::new(None);
    let render = RecordingRenderControl::new();
    effects.set_clean_view(CleanView::ShowBase, render.as_ref());
    effects.set_clean_view(CleanView::ShowEdited, render.as_ref());
    assert!(viewfinder_commands(&render).is_empty());
}

#[test]
fn every_rejected_intent_and_failed_or_refused_save_makes_a_new_problem_with_a_growing_serial() {
    let stub = StubPersistence::accepting();
    let mut effects = effects_with_stub(&stub);
    let render = RecordingRenderControl::new();
    assert_eq!(effects.latest_problem(), None, "без сбоев проблем нет");

    // Успешные намерения и записи проблем не создают.
    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), render.as_ref());
    stub.push_outcome(SaveOutcome::Saved);
    stub.push_outcome(SaveOutcome::SavedDirectoryUnconfirmed(StorageFailure::SyncFailed(
        "каталог".to_owned(),
    )));
    effects.poll_save_outcomes();
    assert_eq!(
        effects.latest_problem(),
        None,
        "успех и неподтверждённый каталог — не проблема"
    );

    // Отвергнутое намерение: серия 1, вид «отвергнуто» с той же ошибкой.
    let honest = PresetStoreConfig::default().builtin_names.honest;
    effects.run_intent(
        LookIntent::SaveAsNewPreset(PresetName::new(&honest).expect("имя")),
        render.as_ref(),
    );
    assert_eq!(
        effects.latest_problem(),
        Some(LookProblem {
            serial: 1,
            kind: LookProblemKind::Rejected(PresetError::NameTaken)
        })
    );

    // Та же ошибка ещё раз — НОВАЯ проблема (серия растёт), экран покажет и её.
    effects.run_intent(
        LookIntent::SaveAsNewPreset(PresetName::new(&honest).expect("имя")),
        render.as_ref(),
    );
    assert_eq!(effects.latest_problem().map(|problem| problem.serial), Some(2));

    // Сбой записи файла.
    let failure = StorageFailure::WriteFailed("диск полон".to_owned());
    stub.push_outcome(SaveOutcome::Failed(PresetError::Storage(failure.clone())));
    effects.poll_save_outcomes();
    assert_eq!(
        effects.latest_problem(),
        Some(LookProblem {
            serial: 3,
            kind: LookProblemKind::SaveFailed(failure)
        })
    );
}

#[test]
fn a_refused_save_request_is_a_problem_of_its_own_kind() {
    let stub = StubPersistence {
        answer: SaveRequestOutcome::Refused,
        ..StubPersistence::accepting()
    };
    let mut effects = effects_with_stub(&stub);
    let render = RecordingRenderControl::new();

    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), render.as_ref());

    assert_eq!(
        effects.latest_problem(),
        Some(LookProblem {
            serial: 1,
            kind: LookProblemKind::SavesRefused
        })
    );
    assert_eq!(effects.counters().save_requests.refused, 1);
}

/// Рендер, который можно «оживить»: пока флаг поднят, он отказывает, потом пишет принятые команды.
struct Revivable {
    gone: std::sync::atomic::AtomicBool,
    recorded: Arc<RecordingRenderControl>,
}

impl Revivable {
    fn gone() -> Self {
        Self {
            gone: std::sync::atomic::AtomicBool::new(true),
            recorded: RecordingRenderControl::new(),
        }
    }

    fn revive(&self) {
        self.gone.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

impl RenderControl for Revivable {
    fn adjust(&self, adjustment: RenderAdjustment) -> ControlDelivery {
        if self.gone.load(std::sync::atomic::Ordering::SeqCst) {
            ControlDelivery::RenderGone
        } else {
            self.recorded.adjust(adjustment)
        }
    }

    fn ui_overlay(&self, _command: render_loop::ui_overlay::UiOverlayCommand) -> ControlDelivery {
        ControlDelivery::Delivered
    }
}

#[test]
fn a_failed_delivery_on_install_is_retried_on_the_next_frame_with_base_and_look() {
    let stub = StubPersistence::accepting();
    let mut effects = effects_with_stub(&stub);
    // Библиотека с правкой: рендер обязан получить именно её, а не нейтраль.
    let render = Revivable::gone();
    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), &render);
    effects.release_clean_view_on_install(&render);
    assert!(viewfinder_commands(&render.recorded).is_empty(), "рендер отказал");

    render.revive();
    effects.retry_if_render_unsure(&render);

    assert_eq!(
        viewfinder_commands(&render.recorded),
        vec![
            ViewfinderCommand::SetToneBase(effects.current_recipe().base),
            set_look(contrast_look(0.5))
        ],
        "следующий кадр дослал и основу, и ручки"
    );
}

#[test]
fn a_failed_show_edited_release_is_retried_on_the_next_frame() {
    let stub = StubPersistence::accepting();
    let mut effects = effects_with_stub(&stub);
    let render = Revivable::gone();
    render.revive();
    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), &render);
    effects.set_clean_view(CleanView::ShowBase, &render);

    // Отпускание «чистяка» не доехало до рендера: он остаётся на нейтрали.
    render.gone.store(true, std::sync::atomic::Ordering::SeqCst);
    effects.set_clean_view(CleanView::ShowEdited, &render);
    let sent_before = viewfinder_commands(&render.recorded).len();

    render.revive();
    effects.retry_if_render_unsure(&render);

    assert_eq!(
        viewfinder_commands(&render.recorded)[sent_before..],
        [set_look(contrast_look(0.5))],
        "правленый вид дослан сам, без нового намерения"
    );
}

#[test]
fn the_per_frame_retry_sends_nothing_when_the_render_is_in_sync() {
    let stub = StubPersistence::accepting();
    let mut effects = effects_with_stub(&stub);
    let render = RecordingRenderControl::new();
    effects.release_clean_view_on_install(render.as_ref());
    let sent_after_install = viewfinder_commands(&render).len();
    assert_eq!(sent_after_install, 2, "установка доставила основу и ручки");

    for _ in 0..10 {
        effects.retry_if_render_unsure(render.as_ref());
    }

    assert_eq!(viewfinder_commands(&render).len(), sent_after_install, "дублей нет");
}

#[test]
fn two_knob_intents_in_a_row_both_reach_the_render_and_the_library() {
    use look_contract::GlobalKnob;
    let stub = StubPersistence::accepting();
    let mut effects = effects_with_stub(&stub);
    let render = RecordingRenderControl::new();

    effects.run_intent(
        LookIntent::SetKnob {
            knob: GlobalKnob::Contrast,
            value: crate::look_test_support::knob(0.4),
        },
        render.as_ref(),
    );
    effects.run_intent(
        LookIntent::SetKnob {
            knob: GlobalKnob::Warmth,
            value: crate::look_test_support::knob(-0.3),
        },
        render.as_ref(),
    );

    let settings = effects.current_recipe().settings;
    assert_eq!(settings.contrast, crate::look_test_support::knob(0.4));
    assert_eq!(settings.warmth, crate::look_test_support::knob(-0.3));
    assert_eq!(
        viewfinder_commands(&render).last(),
        Some(&set_look(settings)),
        "рендер получил оба изменения в последней команде"
    );
    assert_eq!(effects.counters().look_changed, 2);
}
