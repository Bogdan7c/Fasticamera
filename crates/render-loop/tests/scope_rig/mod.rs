//! Общий стенд тестов цветовых приборов поверх настоящего бэкенда показа (`vulkan_scopes*.rs`): конфиг с маленькой
//! моделью приборов, запуск стенда с пустым источником UI и без него, команды и чтение диагностики.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]
#![allow(dead_code, reason = "каждый тест-крейт берёт из стенда своё подмножество")]

use gpu_vulkan::live_objects::{LiveObjectKind, LiveObjectSnapshot};
use render_loop::backend::DisplayBackend;
use render_loop::config::{DisplayColorConfig, RenderLoopConfig, ScopesConfig, ViewfinderConfig};
use render_loop::ui_overlay::{UiOverlayCommand, UiOverlayOutcome};
use render_loop::viewfinder::{ScopeDiagnostics, ViewfinderCommand, ViewfinderCommandOutcome};
use scope_contract::{ScopeImageSize, ScopeIntent, ScopeKind, ScopeRequest, ScopeScan, VectorscopeAxes};
use scope_model::{SampleStride, ScopeModelSettings};
use ui_draw::draw_list::DrawList;
use ui_draw::frame_source::{UiFrameError, UiFrameRequest, UiFrameSource};
use ui_draw::light::LightDirection;

use super::viewfinder_rig::{CellValues, ViewfinderBench};

/// Сколько кадров показа гарантированно хватает, чтобы все слоты кольца были собраны после fence.
pub const FRAMES_TO_FLUSH_SLOTS: usize = 8;
/// Кадры без приборов до измерения исходного числа объектов: ленивые объекты (статистика AWB и т. п.) уже созданы.
pub const WARM_UP_FRAMES: usize = 6;

/// Модель приборов поменьше (тест не гоняет эталон по 256×256×3), шаг 1: меряется каждый пиксель.
pub fn small_model() -> ScopeModelSettings {
    ScopeModelSettings {
        trace_columns: 48,
        trace_levels: 64,
        vectorscope_size: 64,
        sample_stride: SampleStride::try_new(1).expect("шаг"),
        ..ScopeModelSettings::default()
    }
}

/// Конфиг с sRGB-выходом (охват эталона однозначен) и маленькой моделью приборов.
pub fn config() -> RenderLoopConfig {
    RenderLoopConfig {
        display_color: DisplayColorConfig::SrgbOnly,
        viewfinder: ViewfinderConfig {
            scopes: ScopesConfig {
                model: small_model(),
                ..ScopesConfig::default()
            },
            ..ViewfinderConfig::default()
        },
        ..RenderLoopConfig::default()
    }
}

pub fn request(kind: ScopeKind, axes: VectorscopeAxes, width: u32, height: u32) -> ScopeRequest {
    request_with_scan(kind, axes, ScopeScan::Upright, width, height)
}

/// Запрос с явным чтением картинки глазом (положение телефона).
pub fn request_with_scan(
    kind: ScopeKind,
    axes: VectorscopeAxes,
    scan: ScopeScan,
    width: u32,
    height: u32,
) -> ScopeRequest {
    ScopeRequest {
        kind,
        vectorscope_axes: axes,
        scan,
        image_size: ScopeImageSize::try_new(width, height).expect("размер"),
    }
}

pub fn waveform() -> ScopeRequest {
    request(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709, 160, 96)
}

/// Бэкенд без источника UI: приборы здесь отвергаются (они нужны только для UI).
pub fn start_without_ui(scenes: &[CellValues]) -> Option<ViewfinderBench> {
    ViewfinderBench::start_with_scenes(&config(), scenes)
}

/// Бэкенд с установленным пустым источником UI: приборы считаются, а UI ничего не рисует. Большинству тестов нужно
/// именно это: прибор принимается только пока есть источник (решение 190).
pub fn start(scenes: &[CellValues]) -> Option<ViewfinderBench> {
    let mut bench = start_without_ui(scenes)?;
    install(&mut bench, Box::new(IdleSource));
    Some(bench)
}

/// То же, что `start`, но с нашим AE: единственным потребителем статистики кадра (без него она не считается).
pub fn start_with_auto_exposure(scenes: &[CellValues]) -> Option<ViewfinderBench> {
    let mut bench = ViewfinderBench::start_with_scenes_and_auto_exposure(&config(), scenes)?;
    install(&mut bench, Box::new(IdleSource));
    Some(bench)
}

pub fn apply_scopes(bench: &mut ViewfinderBench, intent: ScopeIntent) -> ViewfinderCommandOutcome {
    bench
        .backend
        .apply_viewfinder_command(ViewfinderCommand::Scopes(intent))
}

pub fn scopes_of(bench: &mut ViewfinderBench) -> ScopeDiagnostics {
    bench
        .backend
        .diagnostics()
        .viewfinder
        .expect("диагностика видоискателя")
        .scopes
}

/// Ждёт GPU: счётчики и картинки читаются только после завершения отправленных кадров.
pub fn settle(bench: &ViewfinderBench) {
    bench.backend.context().wait_idle().expect("wait_idle");
}

/// Число объектов GPU, которыми владеют пайплайны (приборы строят свои при создании).
pub fn pipeline_objects(bench: &ViewfinderBench) -> usize {
    let live = bench.backend.context().live_objects();
    [
        LiveObjectKind::Pipeline,
        LiveObjectKind::PipelineLayout,
        LiveObjectKind::ShaderModule,
        LiveObjectKind::DescriptorSetLayout,
    ]
    .into_iter()
    .map(|kind| live.count(kind))
    .sum()
}

/// Изменения числа живых объектов GPU между снимками БЕЗ примитивов синхронизации (`Fence`, `Semaphore`): цепочка
/// показа сама докупает их по мере прохода колец кадров, и к приборам они не относятся (приборы fence и семафоров не
/// создают). Всё остальное (пайплайны, картинки, буферы, память, пулы запросов) считается.
pub fn object_differences(
    before: &LiveObjectSnapshot,
    after: &LiveObjectSnapshot,
) -> Vec<(LiveObjectKind, usize, usize)> {
    after
        .differences_from(before)
        .into_iter()
        .filter(|(kind, _, _)| !matches!(kind, LiveObjectKind::Fence | LiveObjectKind::Semaphore))
        .collect()
}

pub fn flush_frames(bench: &mut ViewfinderBench, frames: usize) {
    for _ in 0..frames {
        bench.show_next_frame();
    }
}

/// Источник UI без рисования: делает приборы допустимыми, не добавляя в кадр ничего.
pub struct IdleSource;

impl UiFrameSource for IdleSource {
    fn name(&self) -> &'static str {
        "idle-test"
    }

    fn produce_frame(&mut self, _request: &UiFrameRequest) -> Result<DrawList, UiFrameError> {
        Ok(DrawList::new(LightDirection::from_angle_degrees(45.0)))
    }
}

pub fn install(bench: &mut ViewfinderBench, source: Box<dyn UiFrameSource>) {
    assert_eq!(
        bench
            .backend
            .apply_ui_overlay_command(UiOverlayCommand::Install(source)),
        UiOverlayOutcome::Applied
    );
}
