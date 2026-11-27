//! Поведение владельца приборов `ScopePass` на настоящем GPU (headless, полная validation), без кадров камеры:
//! подготовка между кадрами, устойчивые отказы сообщаются один раз, отказ проверки записи не прячет прежнюю картинку.
//! Сквозные проверки на настоящем бэкенде — `tests/vulkan_scopes*.rs`.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ash::vk;
use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::live_objects::LiveObjectKind;
use gpu_vulkan::source_window::SourceWindow;
use gpu_vulkan::storage_image::SampledImageView;
use gpu_vulkan::validation::ValidationMode;
use isp_model::image::DisplayGamut;
use scope_contract::{ScopeImageSize, ScopeKind, ScopeRequest, VectorscopeAxes};
use scope_model::ScopeModelSettings;
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

use super::{LOG_TARGET, ScopePass, ScopeRecordInput};
use crate::config::ScopesConfig;
use crate::viewfinder::{ScopeEvents, ScopeResources};

const SKIP_ENV: &str = "FASTICAMERA_SKIP_GPU_TESTS";
const FRAME_SLOTS: usize = 2;

fn context_or_skip() -> Option<GpuContext> {
    match GpuContext::new(&GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::FULL)) {
        Ok(context) => Some(context),
        Err(error) => {
            if std::env::var(SKIP_ENV).as_deref() == Ok("1") {
                eprintln!("ПРОПУСК GPU-теста: {error} ({SKIP_ENV}=1)");
                None
            } else {
                panic!("GPU недоступен: {error}. Установите {SKIP_ENV}=1, чтобы пропустить GPU-тесты осознанно");
            }
        }
    }
}

/// Настройки «прибор на каждом кадре»: тестам судьбы отказов и неготовности каденция мешала бы (её проверяют свои
/// тесты ниже), а умолчание — каждый второй кадр (решение 329).
fn every_frame_config() -> ScopesConfig {
    ScopesConfig {
        every_nth_frame: 1,
        ..ScopesConfig::default()
    }
}

fn request(width: u32, height: u32) -> ScopeRequest {
    ScopeRequest {
        kind: ScopeKind::Waveform,
        vectorscope_axes: VectorscopeAxes::YCbCrBt709,
        vectorscope_zoom: scope_contract::VectorscopeZoom::Normal,
        scan: scope_contract::ScopeScan::Upright,
        image_size: ScopeImageSize::try_new(width, height).expect("размер"),
    }
}

/// Настройки, которые создание набора отвергает (нулевой потолок столбцов): устойчивый отказ создания.
fn unusable_config() -> ScopesConfig {
    ScopesConfig {
        model: ScopeModelSettings {
            trace_max_columns: 0,
            ..ScopeModelSettings::default()
        },
        ..ScopesConfig::default()
    }
}

/// Вид входа, которого приборы не примут (формат не 10-бит UNORM и не fp16): отказ проверки ДО первой команды.
/// Командный буфер пуст (`null`): до команд дело не доходит, а барьер-замыкание ничего не пишет.
fn input_view(view_format: vk::Format) -> SampledImageView {
    SampledImageView {
        view: vk::ImageView::null(),
        extent: vk::Extent2D { width: 64, height: 48 },
        format: view_format,
    }
}

/// Слот кадра, в который пишут тесты.
const FRAME_SLOT: usize = 0;

fn record_with(pass: &mut ScopePass, view: SampledImageView) {
    pass.record(&ScopeRecordInput {
        command_buffer: vk::CommandBuffer::null(),
        frame_slot: FRAME_SLOT,
        view,
        window: SourceWindow::of_content(view.extent),
        gamut: DisplayGamut::Srgb,
        record_compute_read_barrier: &|_| {},
    });
}

/// Подписчик `tracing`, считающий предупреждения цели приборов (лог «один раз» проверяется по нему).
struct WarningCounter {
    warnings: Arc<AtomicU64>,
}

impl Subscriber for WarningCounter {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _attributes: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _span: &Id, _values: &Record<'_>) {}
    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let metadata = event.metadata();
        if metadata.target() == LOG_TARGET && *metadata.level() == Level::WARN {
            self.warnings.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn enter(&self, _span: &Id) {}
    fn exit(&self, _span: &Id) {}
}

/// Прогоняет `body` с подписчиком и возвращает число предупреждений цели приборов.
fn warnings_during(body: impl FnOnce()) -> u64 {
    let warnings = Arc::new(AtomicU64::new(0));
    let subscriber = WarningCounter {
        warnings: Arc::clone(&warnings),
    };
    tracing::subscriber::with_default(subscriber, body);
    warnings.load(Ordering::Relaxed)
}

fn counters_of(pass: &mut ScopePass) -> ScopeEvents {
    pass.take_events()
}

#[test]
fn a_persistent_creation_failure_is_reported_once_per_demand_and_shown_as_failed() {
    let Some(context) = context_or_skip() else { return };
    let mut pass = ScopePass::new(&context, FRAME_SLOTS, &unusable_config());
    let view = input_view(vk::Format::R8G8B8A8_UNORM);

    let warnings = warnings_during(|| {
        pass.set_demand(Some(request(160, 96)));
        for _ in 0..6 {
            pass.prepare_between_frames(pass.demand());
            record_with(&mut pass, view);
        }
    });
    let events = counters_of(&mut pass);
    assert_eq!(warnings, 1, "в лог устойчивый отказ попал один раз");
    assert_eq!(events.counters.record_failures, 1, "и в счётчик — один раз");
    assert_eq!(
        events.counters.frames_skipped_blocked, 6,
        "остальное — пропущенные кадры"
    );
    assert_eq!(events.counters.passes_recorded, 0);
    assert_eq!(events.counters.resources_created, 0);
    assert!(
        events
            .counters
            .last_failure
            .as_deref()
            .is_some_and(|reason| reason.contains("не созданы")),
        "причина сохранена: {:?}",
        events.counters.last_failure
    );
    assert_eq!(
        pass.residency().resources,
        ScopeResources::Failed,
        "диагностика видит блокировку"
    );

    // Новый запрос — новая попытка и новое сообщение; закрытие приборов снимает блокировку.
    let warnings_after_change = warnings_during(|| {
        pass.set_demand(Some(request(200, 120)));
        pass.prepare_between_frames(pass.demand());
        pass.prepare_between_frames(pass.demand());
    });
    assert_eq!(
        warnings_after_change, 1,
        "смена запроса сообщает об отказе заново, один раз"
    );
    assert_eq!(counters_of(&mut pass).counters.record_failures, 1);
    pass.set_demand(None);
    assert_eq!(pass.residency().resources, ScopeResources::Absent);
}

#[test]
fn a_pre_command_rejection_keeps_the_last_picture_and_a_repeated_one_is_reported_once() {
    let Some(context) = context_or_skip() else { return };
    let config = every_frame_config();
    let mut pass = ScopePass::new(&context, FRAME_SLOTS, &config);
    pass.set_demand(Some(request(160, 96)));
    pass.prepare_between_frames(pass.demand());
    assert_eq!(pass.residency().resources, ScopeResources::Live);
    // Картинка прошлой успешной записи (эту запись здесь не делаем: проверяется судьба картинки при отказе).
    let last_picture = SampledImageView {
        view: vk::ImageView::null(),
        extent: vk::Extent2D { width: 160, height: 96 },
        format: vk::Format::R16G16B16A16_SFLOAT,
    };
    pass.live.as_mut().expect("набор").picture = Some(last_picture);
    counters_of(&mut pass);

    let view = input_view(vk::Format::R8G8B8A8_UNORM);
    let warnings = warnings_during(|| {
        for _ in 0..4 {
            record_with(&mut pass, view);
        }
    });
    assert_eq!(
        pass.picture_for_ui(),
        Some(last_picture),
        "отказ проверки до команд не прячет прежнюю картинку"
    );
    assert_eq!(
        pass.residency().resources,
        ScopeResources::Live,
        "набор цел и не выведен"
    );
    assert_eq!(warnings, 1, "одинаковый отказ подряд сообщён один раз");
    let events = counters_of(&mut pass);
    assert_eq!(events.counters.record_failures, 1);
    assert_eq!(events.counters.frames_skipped_blocked, 3);
    assert_eq!(events.counters.passes_recorded, 0);
}

#[test]
fn a_frame_before_the_set_is_prepared_is_skipped_and_counted_without_creating_gpu_objects() {
    let Some(context) = context_or_skip() else { return };
    let mut pass = ScopePass::new(&context, FRAME_SLOTS, &every_frame_config());
    let before = context.live_objects();
    let view = input_view(vk::Format::R16G16B16A16_SFLOAT);

    // Запрос есть, но подготовки между кадрами не было: запись кадра ничего не создаёт.
    pass.set_demand(Some(request(160, 96)));
    for _ in 0..3 {
        record_with(&mut pass, view);
    }
    let events = counters_of(&mut pass);
    assert_eq!(events.counters.frames_skipped_not_ready, 3);
    assert_eq!(events.counters.resources_created, 0);
    assert_eq!(events.counters.record_failures, 0, "неготовность — не отказ");
    assert_eq!(context.live_objects().differences_from(&before), Vec::new());
    assert_eq!(pass.residency().resources, ScopeResources::Absent);

    // Подготовка поднимает набор и картинку; запрос другого размера снова требует подготовки.
    pass.prepare_between_frames(pass.demand());
    assert_eq!(pass.residency().resources, ScopeResources::Live);
    assert!(
        context.live_objects().count(LiveObjectKind::Image) > before.count(LiveObjectKind::Image),
        "картинка прибора создана подготовкой"
    );
    let prepared = context.live_objects();
    pass.set_demand(Some(request(200, 120)));
    record_with(&mut pass, view);
    assert_eq!(counters_of(&mut pass).counters.frames_skipped_not_ready, 1);
    assert_eq!(
        context.live_objects().differences_from(&prepared),
        Vec::new(),
        "запись при смене размера объектов не создаёт"
    );
    pass.prepare_between_frames(pass.demand());
    assert_eq!(
        context.live_objects().count(LiveObjectKind::Image),
        prepared.count(LiveObjectKind::Image),
        "новая картинка создана подготовкой, а прежняя (её никто не записывал) уничтожена сразу"
    );
}

#[test]
fn frames_without_input_are_counted_only_while_scopes_are_requested() {
    let Some(context) = context_or_skip() else { return };
    let mut pass = ScopePass::new(&context, FRAME_SLOTS, &every_frame_config());
    pass.note_frame_without_input();
    assert_eq!(
        counters_of(&mut pass).counters.frames_skipped_no_input,
        0,
        "приборы закрыты — считать нечего"
    );
    pass.set_demand(Some(request(160, 96)));
    pass.note_frame_without_input();
    pass.note_frame_without_input();
    assert_eq!(counters_of(&mut pass).counters.frames_skipped_no_input, 2);
}

/// После пересоздания цели под новый формат счёт «каждый N-й кадр» начинается заново: первый кадр после пересоздания
/// считается всегда. Без сброса он попал бы в прежнюю фазу прореживания, и прибор молчал бы до N кадров.
#[test]
fn the_cadence_restarts_after_a_format_rebuild_so_the_first_frame_after_it_is_measured() {
    let Some(context) = context_or_skip() else { return };
    let config = ScopesConfig {
        every_nth_frame: 3,
        ..ScopesConfig::default()
    };
    let mut pass = ScopePass::new(&context, FRAME_SLOTS, &config);
    // Вход заведомо отвергается проверкой записи (формат не 10-бит и не fp16): отказ после прохождения прореживания
    // — наблюдаемый признак того, что кадр дошёл до прибора, а не был пропущен по каденции.
    let view = input_view(vk::Format::R8G8B8A8_UNORM);
    pass.prepare_between_frames(Some(request(160, 96)));
    // Кадр 1 считается, кадр 2 пропущен по каденции (счёт «с открытия» равен 2 из 3).
    record_with(&mut pass, view);
    record_with(&mut pass, view);
    let before = counters_of(&mut pass).counters;
    assert_eq!(before.frames_skipped_by_cadence, 1, "второй кадр пропущен по каденции");
    assert_eq!(before.record_failures, 1, "первый кадр дошёл до прибора");

    pass.reset_for_format();
    pass.prepare_between_frames(Some(request(160, 96)));
    counters_of(&mut pass);
    // Первый кадр после пересоздания: в старой фазе он был бы третьим (счёт 2 из 3: пропуск), в новой — первый.
    record_with(&mut pass, view);
    let after = counters_of(&mut pass).counters;
    assert_eq!(
        after.frames_skipped_by_cadence, 0,
        "первый кадр после пересоздания не пропущен по каденции"
    );
    assert_eq!(
        after.record_failures, 1,
        "он дошёл до прибора (отказ проверки входа — признак)"
    );
}

/// Смена запроса (свайп страницы, оси, зум) считается на ближайшем кадре, а не ждёт фазы прореживания: иначе при
/// N > 1 под новым именем страницы стояла бы картинка прежнего прибора (решение 329).
#[test]
fn a_changed_request_is_measured_on_the_very_next_frame_whatever_the_cadence_phase() {
    let Some(context) = context_or_skip() else { return };
    let config = ScopesConfig {
        every_nth_frame: 2,
        ..ScopesConfig::default()
    };
    let mut pass = ScopePass::new(&context, FRAME_SLOTS, &config);
    // Вход отвергается проверкой записи: отказ — признак, что кадр дошёл до прибора, а не пропущен по каденции.
    let view = input_view(vk::Format::R8G8B8A8_UNORM);
    pass.prepare_between_frames(Some(request(160, 96)));
    // Кадр 1 считается; после него фаза «пропуск».
    record_with(&mut pass, view);
    let opened = counters_of(&mut pass).counters;
    assert_eq!((opened.record_failures, opened.frames_skipped_by_cadence), (1, 0));

    // Свайп на parade прямо перед кадром, который в старой фазе был бы пропущен.
    let parade = ScopeRequest {
        kind: ScopeKind::RgbParade,
        ..request(160, 96)
    };
    pass.prepare_between_frames(Some(parade));
    counters_of(&mut pass);
    record_with(&mut pass, view);
    let changed = counters_of(&mut pass).counters;
    assert_eq!(changed.frames_skipped_by_cadence, 0, "новый запрос не ждёт фазы");
    assert_eq!(changed.record_failures, 1, "кадр дошёл до прибора");

    // Дальше прореживание идёт как обычно: следующий кадр пропущен.
    record_with(&mut pass, view);
    assert_eq!(counters_of(&mut pass).counters.frames_skipped_by_cadence, 1);

    // Тот же запрос повторно — не смена: фаза не сбрасывается, кадр после пропуска считается, следующий снова нет.
    pass.prepare_between_frames(Some(parade));
    record_with(&mut pass, view);
    record_with(&mut pass, view);
    assert_eq!(counters_of(&mut pass).counters.frames_skipped_by_cadence, 1);
}
