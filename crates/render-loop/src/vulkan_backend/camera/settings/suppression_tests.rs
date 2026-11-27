//! Подавление работы GPU замером (16a) в настройках видоискателя: единственное место решения о приборах и учёт команды.

use scope_contract::{ScopeKind, ScopeScan, VectorscopeAxes};

use super::*;

fn defaults() -> ViewfinderSettings {
    ViewfinderSettings::from_config(&ViewfinderConfig::default()).expect("конфиг по умолчанию корректен")
}

fn waveform() -> ScopeRequest {
    ScopeRequest {
        kind: ScopeKind::Waveform,
        vectorscope_axes: VectorscopeAxes::YCbCrBt709,
        vectorscope_zoom: scope_contract::VectorscopeZoom::Normal,
        cie_diagram: scope_contract::CieDiagram::default(),
        histogram_mode: scope_contract::HistogramMode::Rgb,
        scan: ScopeScan::Upright,
        image_size: scope_contract::ScopeImageSize::try_new(400, 300).expect("размер"),
    }
}

fn suppress(work: SuppressibleGpuWork) -> GpuWorkSuppression {
    GpuWorkSuppression::NONE.suppressing(work)
}

#[test]
fn product_settings_start_without_suppression() {
    assert_eq!(defaults().gpu_work_suppression, GpuWorkSuppression::NONE);
}

#[test]
fn scope_demand_is_decided_by_painter_ui_source_and_suppression_in_every_combination() {
    let suppressions = [
        GpuWorkSuppression::NONE,
        suppress(SuppressibleGpuWork::ColorScopes),
        suppress(SuppressibleGpuWork::UiOverlayDrawing),
        suppress(SuppressibleGpuWork::ColorScopes).suppressing(SuppressibleGpuWork::UiOverlayDrawing),
    ];
    for painter in [ViewfinderPainter::Isp, ViewfinderPainter::DebugMosaic] {
        for ui_source in [UiSourcePresence::Installed, UiSourcePresence::Absent] {
            for suppression in suppressions {
                let mut settings = defaults();
                settings.painter = painter;
                settings.scopes = Some(waveform());
                settings.gpu_work_suppression = suppression;
                let expected = (painter == ViewfinderPainter::Isp
                    && ui_source == UiSourcePresence::Installed
                    && !suppression.suppresses(SuppressibleGpuWork::ColorScopes))
                .then(waveform);
                assert_eq!(
                    settings.effective_scope_demand(ui_source),
                    expected,
                    "{painter:?} / {ui_source:?} / {suppression:?}"
                );
            }
        }
    }
}

#[test]
fn suppressing_scopes_keeps_the_screen_request_and_lifting_it_resumes_without_a_new_show() {
    let mut settings = defaults();
    settings.apply(ViewfinderCommand::Scopes(ScopeIntent::Show(waveform())));
    let shown = settings.effective_scope_demand(UiSourcePresence::Installed);
    assert_eq!(shown, Some(waveform()), "без подавления приборы считаются");

    let suppress_scopes = ViewfinderCommand::SuppressGpuWork(suppress(SuppressibleGpuWork::ColorScopes));
    assert_eq!(settings.apply(suppress_scopes), ViewfinderCommandOutcome::Applied);
    assert_eq!(settings.effective_scope_demand(UiSourcePresence::Installed), None);
    assert_eq!(settings.scopes, Some(waveform()), "запрос экрана не стёрт");

    assert_eq!(
        settings.apply(ViewfinderCommand::SuppressGpuWork(GpuWorkSuppression::NONE)),
        ViewfinderCommandOutcome::Applied
    );
    assert_eq!(
        settings.effective_scope_demand(UiSourcePresence::Installed),
        shown,
        "приборы возобновились сами, без повторного Scopes(Show)"
    );
}

#[test]
fn suppress_command_replaces_the_set_and_a_repeat_is_already_in_effect() {
    let mut settings = defaults();
    let scopes_only = ViewfinderCommand::SuppressGpuWork(suppress(SuppressibleGpuWork::ColorScopes));
    assert_eq!(settings.apply(scopes_only.clone()), ViewfinderCommandOutcome::Applied);
    assert_eq!(settings.apply(scopes_only), ViewfinderCommandOutcome::AlreadyInEffect);
    assert_eq!(
        settings.apply(ViewfinderCommand::SuppressGpuWork(GpuWorkSuppression::NONE)),
        ViewfinderCommandOutcome::Applied,
        "NONE при подавленном наборе — изменение"
    );
    assert_eq!(
        settings.apply(ViewfinderCommand::SuppressGpuWork(GpuWorkSuppression::NONE)),
        ViewfinderCommandOutcome::AlreadyInEffect,
        "NONE при продукте — ничего"
    );
    // Набор заменяется целиком: был UI, стал только приборы.
    settings.apply(ViewfinderCommand::SuppressGpuWork(suppress(
        SuppressibleGpuWork::UiOverlayDrawing,
    )));
    settings.apply(ViewfinderCommand::SuppressGpuWork(suppress(
        SuppressibleGpuWork::ColorScopes,
    )));
    assert!(
        !settings
            .gpu_work_suppression
            .suppresses(SuppressibleGpuWork::UiOverlayDrawing)
    );
    assert!(
        settings
            .gpu_work_suppression
            .suppresses(SuppressibleGpuWork::ColorScopes)
    );
}

#[test]
fn suppress_command_touches_nothing_but_the_suppression_mask() {
    let mut settings = defaults();
    settings.apply(ViewfinderCommand::Scopes(ScopeIntent::Show(waveform())));
    settings.apply(ViewfinderCommand::SetPainter(ViewfinderPainter::Isp));
    let before = settings.clone();
    let both = suppress(SuppressibleGpuWork::ColorScopes).suppressing(SuppressibleGpuWork::UiOverlayDrawing);
    settings.apply(ViewfinderCommand::SuppressGpuWork(both));
    assert_eq!(settings.gpu_work_suppression, both);
    assert_eq!(
        ViewfinderSettings {
            gpu_work_suppression: GpuWorkSuppression::NONE,
            ..settings
        },
        before,
        "рисовальщик, стадии, вид, основа тона, запрос приборов и кадрирование остались прежними"
    );
}
