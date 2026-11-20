//! Факты об объективах: перевод статуса движка в язык экрана. Главное — номер неудачи смены: строго растёт и не
//! повторяется при перечитывании статуса (иначе экран откатывал бы показанный коэффициент дважды или ни разу).

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::ids::{LensFacing, LensRole};
use camera_core::lens_status::{ActiveLens as EngineActiveLens, LensBlock, LensSwitchFailure as EngineFailure};

use super::*;
use crate::camera_test_support::oneplus_zoom_catalog;

fn camera_of(catalog: &ZoomCatalog, role: LensRole) -> CameraId {
    camera_of_role(catalog, role).expect("роль есть в каталоге OnePlus 11")
}

fn status_with_catalog() -> LensStatus {
    LensStatus {
        zoom_catalog: oneplus_zoom_catalog(),
        ..LensStatus::initial()
    }
}

fn active(status: &LensStatus, role: LensRole, facing: LensFacing) -> Option<EngineActiveLens> {
    Some(EngineActiveLens {
        role,
        camera: camera_of(&status.zoom_catalog, role),
        facing,
    })
}

/// Строитель, уже увидевший статус без неудач (точка отсчёта), и этот статус для дальнейших правок.
fn baselined_builder() -> (LensFactsBuilder, LensStatus) {
    let mut builder = LensFactsBuilder::default();
    let status = status_with_catalog();
    assert_eq!(builder.facts_of(&status).last_failure, None);
    (builder, status)
}

fn block(role: LensRole) -> Option<LensBlock> {
    Some(LensBlock {
        role,
        reason: EngineFailure::Unavailable {
            reason: CameraUnavailableReason::DeviceFailure,
        },
    })
}

#[test]
fn empty_initial_status_gives_no_catalog_no_camera_and_a_steady_picture() {
    let facts = LensFactsBuilder::default().facts_of(&LensStatus::initial());

    assert_eq!(facts, LensFacts::NONE);
}

#[test]
fn published_catalog_active_camera_and_switch_are_translated_to_cameras() {
    let mut status = status_with_catalog();
    status.active = active(&status, LensRole::Wide, LensFacing::Back);
    status.switch = LensSwitchState::Switching {
        from: LensRole::Wide,
        to: LensRole::UltraWide,
    };
    let facts = LensFactsBuilder::default().facts_of(&status);

    let catalog = facts.catalog.expect("каталог с шкалами показан экрану");
    assert_eq!(catalog.as_ref(), status.zoom_catalog.as_ref());
    let active = facts.active.expect("активная камера");
    assert_eq!((active.role, active.facing), (LensRole::Wide, LensFacing::Back));
    assert_eq!(active.camera, camera_of(&catalog, LensRole::Wide));
    assert_eq!(
        facts.switch,
        LensSwitch::Switching {
            from: camera_of(&catalog, LensRole::Wide),
            to: camera_of(&catalog, LensRole::UltraWide),
        }
    );
    assert_eq!(facts.last_failure, None);
}

#[test]
fn steady_and_idle_engine_states_are_not_a_switch() {
    let mut status = status_with_catalog();
    for state in [LensSwitchState::Idle, LensSwitchState::Steady { role: LensRole::Wide }] {
        status.switch = state;
        assert_eq!(LensFactsBuilder::default().facts_of(&status).switch, LensSwitch::Steady);
    }
}

#[test]
fn a_switch_to_a_role_missing_from_the_catalog_is_not_shown_instead_of_guessing_cameras() {
    let mut status = status_with_catalog();
    status.switch = LensSwitchState::Switching {
        from: LensRole::Wide,
        to: LensRole::Tele,
    };

    assert_eq!(LensFactsBuilder::default().facts_of(&status).switch, LensSwitch::Steady);
}

#[test]
fn each_new_failure_gets_the_next_sequence_number_and_the_blocked_camera_as_target() {
    let (mut builder, mut status) = baselined_builder();
    status.active = active(&status, LensRole::Wide, LensFacing::Back);

    status.counters.switches_failed = 1;
    status.blocked = block(LensRole::UltraWide);
    let first = builder.facts_of(&status).last_failure.expect("первая неудача");
    assert_eq!(first.sequence, 1);
    assert_eq!(first.target, camera_of(&status.zoom_catalog, LensRole::UltraWide));

    status.counters.switches_failed = 2;
    status.blocked = block(LensRole::Front);
    let second = builder.facts_of(&status).last_failure.expect("вторая неудача");
    assert_eq!(second.sequence, 2, "номер растёт на каждой неудаче");
    assert_eq!(second.target, camera_of(&status.zoom_catalog, LensRole::Front));
}

#[test]
fn rereading_the_same_status_repeats_the_same_failure_without_a_new_number() {
    let (mut builder, mut status) = baselined_builder();
    status.counters.switches_failed = 3;
    status.blocked = block(LensRole::UltraWide);

    let once = builder.facts_of(&status);
    let twice = builder.facts_of(&status);

    assert_eq!(once.last_failure, twice.last_failure);
    assert_eq!(once.last_failure.expect("неудача").sequence, 3);
}

#[test]
fn the_failure_stays_known_after_the_engine_clears_the_block_and_the_target_is_not_lost() {
    let (mut builder, mut status) = baselined_builder();
    status.counters.switches_failed = 1;
    status.blocked = block(LensRole::UltraWide);
    let before = builder.facts_of(&status).last_failure;

    // Пользователь выбрал другую роль: движок снял блокировку, счётчик не изменился.
    status.blocked = None;
    let after = builder.facts_of(&status).last_failure;

    assert_eq!(before, after, "неудача липкая: цель запомнена в момент роста счётчика");
}

#[test]
fn failure_without_a_blocked_role_falls_back_to_the_active_camera_and_is_counted() {
    let (mut builder, mut status) = baselined_builder();
    status.active = active(&status, LensRole::Wide, LensFacing::Back);
    status.counters.switches_failed = 1;

    let failure = builder.facts_of(&status).last_failure.expect("неудача не потеряна");

    assert_eq!(failure.target, camera_of(&status.zoom_catalog, LensRole::Wide));
    assert_eq!(builder.diagnostics().failure_target_unresolved, 1);
    assert_eq!(builder.diagnostics().failures_without_any_target, 0);
}

#[test]
fn failure_with_no_target_at_all_is_counted_in_diagnostics_not_swallowed() {
    let (mut builder, mut status) = baselined_builder();
    status.counters.switches_failed = 1;

    let facts = builder.facts_of(&status);

    assert_eq!(facts.last_failure, None);
    assert_eq!(builder.diagnostics().failures_without_any_target, 1);
}

#[test]
fn facts_are_equal_for_equal_statuses_so_the_watcher_does_not_resend_them() {
    let (mut builder, mut status) = baselined_builder();
    status.counters.switches_failed = 1;
    status.blocked = block(LensRole::UltraWide);

    assert_eq!(builder.facts_of(&status), builder.facts_of(&status));
}

#[test]
fn failures_that_happened_before_the_screen_appeared_are_history_not_a_new_failure() {
    let mut status = status_with_catalog();
    status.counters.switches_failed = 4;
    status.blocked = block(LensRole::UltraWide);

    let mut fresh_screen_builder = LensFactsBuilder::default();
    let facts = fresh_screen_builder.facts_of(&status);

    assert_eq!(
        facts.last_failure, None,
        "переустановленный экран не откатывает значение из-за старой неудачи"
    );
    assert_eq!(fresh_screen_builder.diagnostics(), LensFactsDiagnostics::default());

    status.counters.switches_failed = 5;
    let next = fresh_screen_builder
        .facts_of(&status)
        .last_failure
        .expect("новая неудача после отсчёта");
    assert_eq!(
        next.sequence, 5,
        "номер — значение счётчика движка, а не счёт строителя"
    );
}
