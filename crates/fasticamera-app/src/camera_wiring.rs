//! Запуск движка камеры и сборка [`CameraServices`] (по образцу `render_wiring`).
//!
//! Модуль нейтрален к платформе: фабрика бэкенда и источник диагностики приходят снаружи (на
//! телефоне — Camera2 NDK из `camera-android`, в тестах — fake `camera-core`), поэтому путь
//! «событие жизненного цикла → команда → бэкенд» проверяется на ПК настоящим потоком движка.
//!
//! Движок стартует с присутствием `Background`: Android отдаёт камеру только приложению на переднем
//! плане, а событие `Resumed` маршрутизатора переведёт движок в `Foreground`. Сразу после запуска
//! движка ставится желаемое состояние живого видоискателя (временная политика до сессии 12,
//! `viewfinder_autostart`, конфиг `camera.viewfinder_autostart`): в фоне это только желаемое,
//! камера откроется по `Resumed`. Маршрутизатор об автозапуске не знает.

use std::sync::Arc;

use camera_contract::three_a::DirectiveSlot;
use camera_contract::white::{WhiteAnswerChannel, WhiteDirective, WhiteSourceId};
use camera_contract::white_reading::{WhiteReading, WhiteReadingBoard};
use camera_contract::zoom::LensIntentChannel;
use camera_core::backend::BackendFactory;
use camera_core::flash_status::FlashStillObserver;
use camera_core::white::WhiteWiring;
use camera_core::{AppPresence, CameraRuntime, LensWiring, ThreeAWiring};
use look_contract::LookWhiteStopsBoard;
use render_loop::viewfinder::WhiteLinks;
use scope_contract::ScopeLabelsBoard;

use crate::auto_exposure_link::{
    AutoExposureLink, AutoExposureRenderEnd, EngineBinding, ExposureControllerChoice, ExposureControllerKind,
    RenderAutoExposure,
};
use crate::camera_config::CameraConfig;
use crate::camera_service::{BackendDiagnosticsSource, CameraEngineAccess, CameraServices, RunningCamera};
use crate::flash_still_relay::FlashStillRelay;
use crate::raw_stream_services::RawStreamServices;
use crate::viewfinder_autostart::ViewfinderAutostart;
use crate::white_memory::WhiteMemoryHandoff;

const LOG_TARGET: &str = "fasticamera::camera::wiring";

/// То, что платформа даёт движку: как создать бэкенд и откуда читать его диагностику.
pub struct CameraBackendParts {
    /// Фабрика бэкенда: вызывается один раз на потоке камеры.
    pub factory: BackendFactory,
    pub diagnostics: Arc<dyn BackendDiagnosticsSource>,
    /// Наблюдательные сервисы потока кадров, который бэкенд наполняет (счётчики, проба CPU, часы).
    pub raw_stream: RawStreamServices,
}

/// Ящики, которыми движок камеры связан с потоком рендера. Корень композиции создаёт их один раз на
/// процесс: одна половина уходит движку, такой же клон — конструктору бэкенда рендера.
#[derive(Debug, Clone, Default)]
pub struct CameraLinks {
    /// Указания рендеру, какую белую держать (план white-balance): владелец белого (движок) пишет, рендер читает.
    pub white_directives: DirectiveSlot<WhiteDirective>,
    /// Ответы источников белого: рендер пишет и толкает движок, движок читает.
    pub white_answers: WhiteAnswerChannel,
    /// Показания источников белого (WB33): рендер пишет, экран читает («≈ 3100» на кнопке источника). Владельцу
    /// белого они не идут: показание — не ответ.
    pub white_readings: WhiteReadingBoard,
    /// Упоры ручек теплоты и оттенка (решения 288, 293): рендер пишет при смене белой сцены или этих ручек, приложение
    /// читает для экрана калибровки ([`crate::look_white_stops::LookWhiteStopsReader`]). Движку они не идут.
    pub look_white_stops: LookWhiteStopsBoard,
    /// Подписи прибора (решения 326, 327): рендер пишет при смене решённого прибора, приложение читает для экрана
    /// ([`crate::scope_labels::ScopeLabelsReader`]). Движку они не идут.
    pub scope_labels: ScopeLabelsBoard,
    /// Открытая память белой (файл в папке приложения): движок забирает её один раз при сборке. Пустая — без
    /// сохранения (тесты, стенды, режим `none`).
    pub white_memory: WhiteMemoryHandoff,
    /// Предложения и указания нашего AE (08a); с вендорским AE не используются.
    pub auto_exposure: AutoExposureLink,
    /// Поднялся ли рендер, который будет мерить сцену для нашего AE (решает, какой AE получит камера).
    pub render_auto_exposure: RenderAutoExposure,
    /// Намерения объектива (сессия 13): экран камеры публикует (`Effect::Lens`), движок читает и выбирает
    /// камеру с гистерезисом. Один ящик на процесс: тот же клон получает исполнитель эффектов экрана.
    pub lens_intents: LensIntentChannel,
    /// Наблюдатель итогов снимков со вспышкой (14d): движок сообщает ему итог, а служба снимков при запуске
    /// подключает к нему рендер (поздняя привязка, `FlashStillRelay`). Один на процесс.
    pub flash_relay: Arc<FlashStillRelay>,
}

/// Рендерная половина тех же ящиков ([`CameraLinks::render_side`]).
#[derive(Debug, Clone)]
pub struct RenderLinks {
    /// Белая рендера: читатель указаний и писатель ответов. Реестр источников пуст: его собирает корень
    /// композиции из конфига рендера (`white_sources`) и кладёт в `white.sources`.
    pub white: WhiteLinks,
    /// Сторона рендера нашего AE; `None`, если у камеры вендорский AE.
    pub auto_exposure: Option<AutoExposureRenderEnd>,
}

impl CameraLinks {
    /// Привязывает запущенный движок к толчку ответов источников белого; при падении привязки толчок отвязывается.
    pub(crate) fn white_binding(
        &self,
        receiver: Arc<dyn camera_contract::auto_exposure::RefreshSignal>,
    ) -> EngineBinding {
        EngineBinding::bind(self.white_answers.refresh_signal(), receiver)
    }

    /// Последнее показание источника белого для экрана; `None` — рендер ещё ничего о нём не показывал.
    pub fn white_reading_of(&self, source: WhiteSourceId) -> Option<WhiteReading> {
        self.white_readings.latest_of(source)
    }

    /// Половина для потока рендера: клоны тех же ящиков, что держит движок.
    pub fn render_side(&self, exposure: ExposureControllerKind) -> RenderLinks {
        RenderLinks {
            white: WhiteLinks {
                directives: self.white_directives.clone(),
                answers: self.white_answers.clone(),
                readings: self.white_readings.clone(),
                look_white_stops: self.look_white_stops.clone(),
                scope_labels: self.scope_labels.clone(),
                sources: Default::default(),
            },
            auto_exposure: self.auto_exposure.render_end(exposure),
        }
    }
}

/// Запускает движок камеры. Любой отказ запуска (неверный конфиг, не создавшийся поток) не роняет
/// приложение: возвращается сервис «камера недоступна: причина».
///
/// Ящики связи здесь свои и никем не читаются: так запускают тесты и стенды без рендера. Боевой корень
/// композиции зовёт [`start_camera_with_links`].
pub fn start_camera(config: &CameraConfig, backend: CameraBackendParts) -> CameraServices {
    start_camera_with_links(config, backend, CameraLinks::default())
}

/// Запускает движок камеры, отдавая ему **камерную** половину ящиков связи с рендером (решения 64, 08a).
///
/// Ни движок, ни рендер не знают друг о друге: между ними только нейтральные ящики из `camera-contract`.
pub fn start_camera_with_links(
    config: &CameraConfig,
    backend: CameraBackendParts,
    links: CameraLinks,
) -> CameraServices {
    let services = spawn_engine(config, backend, links);
    start_product_viewfinder(services, config)
}

/// Состав 3A по переключателю `camera.exposure_controller`: наш AE (с ящиками связи) или вендорский.
fn three_a_wiring(config: &CameraConfig, links: &CameraLinks, effective: ExposureControllerKind) -> ThreeAWiring {
    let wiring = match effective {
        ExposureControllerKind::Own => ThreeAWiring::own(
            &config.engine.three_a,
            links.auto_exposure.engine_proposals(),
            config.android.mains_frequency,
        )
        .with_exposure_directive_slot(links.auto_exposure.engine_directive_slot()),
        ExposureControllerKind::Vendor => ThreeAWiring::vendor(&config.engine.three_a),
    };
    // Белая: ящики общие с рендером, память открыта корнем композиции до движка (WB26).
    let memory = links.white_memory.take();
    let white = WhiteWiring {
        directives: links.white_directives.clone(),
        answers: links.white_answers.clone(),
        remembered: memory.load,
        remembered_resume_rule: memory.resume_rule,
        remembered_follow: memory.follow,
        memory: memory.memory,
        opening_source: WhiteSourceId::PHONE,
    };
    // Итоги снимков со вспышкой идут и событием движка, и наблюдателю: в проде события никто не разбирает.
    wiring
        .with_white(white)
        .with_flash_observer(Arc::clone(&links.flash_relay) as Arc<dyn FlashStillObserver>)
}

/// Запуск потока движка; отказ — сервис «камера недоступна: причина».
fn spawn_engine(config: &CameraConfig, backend: CameraBackendParts, links: CameraLinks) -> CameraServices {
    if let Err(error) = config.validate() {
        return CameraServices::unavailable(format!("неверные настройки камеры: {error}"), config.scenarios.clone());
    }
    let CameraBackendParts {
        factory,
        diagnostics,
        raw_stream,
    } = backend;
    let choice = ExposureControllerChoice::resolve(config.exposure_controller, &links.render_auto_exposure);
    if let Some(reason) = &choice.fallback_reason {
        tracing::warn!(
            target: LOG_TARGET,
            configured = ?choice.configured,
            effective = ?choice.effective,
            %reason,
            "исполнитель экспозиции заменён"
        );
    }
    let wiring = three_a_wiring(config, &links, choice.effective);
    let white_memory_diagnostics = links.white_memory.diagnostics().clone();
    let white_readings = links.white_readings.clone();
    let look_white_stops = links.look_white_stops.clone();
    let scope_labels = links.scope_labels.clone();
    let lens_intents = links.lens_intents.clone();
    match CameraRuntime::spawn_with_lens_wiring(
        config.engine.clone(),
        AppPresence::Background,
        factory,
        wiring,
        LensWiring::new(lens_intents.clone()),
    ) {
        Ok((runtime, handle)) => {
            tracing::info!(
                target: LOG_TARGET,
                exposure_controller = ?choice.effective,
                "движок камеры запущен, присутствие: фон до события Resumed"
            );
            let mut running = RunningCamera::new(runtime, handle, diagnostics);
            send_white_resume_choice(config, running.handle());
            send_white_follow_speed_choice(config, running.handle());
            // Ответы источников белого будят движок тем же толчком, что предложения AE. Привязка не зависит от
            // выбора исполнителя экспозиции: белая владеет движком всегда.
            let white_waker = running.handle().three_a_waker();
            running = running.with_white_binding(links.white_binding(Arc::new(white_waker)));
            if choice.effective == ExposureControllerKind::Own {
                // Толчок рендера привязываем только после запуска: `ThreeAWaker` есть лишь у живого движка.
                // Предложение, опубликованное раньше, не потеряно: оно лежит в ящике, движок прочтёт его
                // при первом же плане.
                let waker = running.handle().three_a_waker();
                running = running.with_exposure_binding(links.auto_exposure.bind_engine(Arc::new(waker)));
            }
            CameraServices::new(CameraEngineAccess::Running(Arc::new(running)), config.scenarios.clone())
                .with_raw_stream(raw_stream)
                .with_exposure_controller(choice)
                .with_lens_intents(lens_intents)
                .with_white_memory(white_memory_diagnostics)
                .with_white_readings(white_readings)
                .with_look_white_stops(look_white_stops)
                .with_scope_labels(scope_labels)
        }
        Err(error) => {
            CameraServices::unavailable(format!("движок камеры не запущен: {error}"), config.scenarios.clone())
        }
    }
}

/// Выбор правила возобновления белого, заданный параметром запуска (`camera.white_resume_choice`): уходит движку
/// командой ДО первого `EnterForeground` (приложение стартует в фоне, `Resumed` приходит позже), поэтому первое же
/// возобновление уже идёт по выбранному правилу, а сам выбор ложится в память как выбор человека. Не задан — ничего
/// не шлём: правило решают память и конфиг движка. Отказ отправки (движок уже остановлен) не роняет запуск, но не
/// молчит.
fn send_white_resume_choice(config: &CameraConfig, handle: &camera_core::CameraHandle) {
    let Some(rule) = config.white_resume_choice else {
        return;
    };
    if let Err(error) = handle.set_white_resume_rule(rule) {
        tracing::warn!(
            target: LOG_TARGET,
            ?rule,
            %error,
            "выбор правила возобновления белого не отправлен движку: действует память или конфиг"
        );
    }
}

/// Скорость слежения за белым, заданная параметром запуска (`camera.white_follow_speed_choice`): уходит движку
/// командой ДО первого `EnterForeground`, как и выбор правила возобновления, и ложится в память как выбор человека.
/// Не задана — ничего не шлём: скорость решают память и конфиг движка. Режим слежения параметр не включает.
/// Отказ отправки (движок уже остановлен) не роняет запуск, но не молчит.
fn send_white_follow_speed_choice(config: &CameraConfig, handle: &camera_core::CameraHandle) {
    let Some(speed) = config.white_follow_speed_choice else {
        return;
    };
    if let Err(error) = handle.set_white_follow_speed(speed) {
        tracing::warn!(
            target: LOG_TARGET,
            ?speed,
            %error,
            "скорость слежения за белым не отправлена движку: действует память или конфиг"
        );
    }
}

/// Подключает автозапуск видоискателя и выставляет его желаемое состояние. При недоступной камере
/// автозапуск тоже подключается (если его секция верна): в логе честно «камера недоступна:
/// видоискатель не открыт», а не тишина. Итог уже записан в лог владельцем политики.
fn start_product_viewfinder(services: CameraServices, config: &CameraConfig) -> CameraServices {
    let autostart = match ViewfinderAutostart::from_config(&config.viewfinder_autostart) {
        Ok(autostart) => autostart,
        Err(error) => {
            // Сюда попадаем только с уже недоступной камерой: `spawn_engine` проверил весь конфиг.
            tracing::error!(target: LOG_TARGET, %error, "автозапуск видоискателя не подключён");
            ViewfinderAutostart::disabled()
        }
    };
    let services = services.with_viewfinder_autostart(autostart);
    let _logged_by_owner = services.start_product_viewfinder();
    services
}

#[cfg(test)]
#[path = "camera_wiring_tests.rs"]
mod tests;
