//! Фасад экрана камеры для приложения: [`CameraScreen`] — сборка виджетов `ui-kit` поверх живого видоискателя.
//!
//! # Контракт с приложением (пакет P6)
//! * Экран живёт на потоке рендера как [`UiFrameSource`] (трейт реализован здесь, `Send`): кадр строится от
//!   `present_time` этого же показа, без пересылки списков между потоками.
//! * Вход: касания [`CameraScreen::handle_touch`] (пиксели surface; разрыв потока — [`ScreenTouch::Gap`]) и
//!   внешние факты [`CameraScreen::handle_action`] (камера, ориентация, гравитация, клавиша громкости, итог
//!   снимка). Выход: [`CameraScreen::drain_effects`] — намерения для камеры, сервиса съёмки, хаптики, галереи
//!   и рендера (кадрирование видоискателя).
//! * Время — только `UiFrameRequest::present_time` и метки касаний; `Instant::now()` нет.
//!
//! # Порядок кадра (`produce_frame`)
//! 1. размер/масштаб surface изменились → идущие жесты отменяются явным `Cancelled` (разрыв потока), затем
//!    `SurfaceResized` в редьюсер и пересборка сцены;
//! 2. часы сцены → таймеры жестов (долгий тап) → забрать события жестов → виджеты → действия → `reduce`;
//! 3. `Tick(present_time)` (отсчёт таймера, затухание индикатора);
//! 4. синхронизация элементов с состоянием (без эха) → анимации → события элементов → действия (до
//!    [`MAX_SETTLE_PASSES`] проходов, пока действия появляются);
//! 5. список рисования по слоям и проверка словаря.
//!
//! # Ошибки кадра
//! Ошибка кадра делится по судьбе сцены:
//! * ошибка **вида** (сборка списка рисования — сцена при этом только читается): сцена и идущие жесты
//!   сохраняются, следующий кадр пробует снова (`frame_failures_scene_kept`);
//! * ошибка, **изменявшая** сцену (геометрия, размер, жесты, элементы), или ошибка самой сборки: сцена
//!   сбрасывается, жесты отменяются явным `Cancelled`, а следующая пересборка не раньше чем через
//!   `stage_rebuild_backoff_ms` — устойчивая ошибка не пересобирает сцену на каждом кадре. Кадры паузы
//!   отдают ошибку источника (`frames_skipped_rebuild_backoff`), касания без сцены считаются
//!   (`touches_dropped_while_stage_missing`), а не пропадают молча.
//!
//! Единственная точка изменения состояния — [`CameraScreen::apply`]: действие → `reduce` → очередь эффектов →
//! счётчики. Затемнение при снимке запускается именно здесь по `Effect::Capture`, поэтому его дают и кнопка,
//! и клавиша громкости, и таймер.

mod calibration_targets;
mod diagnostics;
mod effect_queue;
mod error;
mod stage;
mod system_gesture_filter;
#[cfg(test)]
mod touch_map_test_support;
mod white_targets;

use std::sync::Arc;

use shell_contract::clock::MonotonicNanos;
use shell_contract::surface::SurfaceSize;
use shell_contract::system_gesture::SystemGestureZones;
use shell_contract::touch::TouchSample;
use ui_core::time::MotionPreference;
use ui_draw::draw_list::DrawList;
use ui_draw::frame_source::{UiFrameError, UiFrameRequest, UiFrameSource};
use ui_draw::light::LightDirection;
use ui_kit::tokens::UiKitTokens;
use ui_kit::view::PxPerPt;

pub use calibration_targets::CalibrationTouchTargets;
pub use diagnostics::{GestureCounters, ScreenDiagnostics};
pub use error::ScreenBuildError;
pub use white_targets::{WhiteCardTargets, WhiteTouchTargets};

use crate::action::ScreenAction;
use crate::config::ScreenConfig;
use crate::effect::Effect;
use crate::outcome::ActionOutcome;
use crate::reduce::reduce;
use crate::shutter::{CaptureOutcome, ShotRequestId};
use crate::state::ScreenState;
use crate::widget::capture_blink::CaptureBlinkWidget;
use crate::widget::error::WidgetError;
use crate::widget::geometry::ScreenGeometry;
use crate::widget::transition_veil::TransitionVeil;
use crate::widget::{Ctx, Outputs};
use effect_queue::{EffectQueue, PushOutcome};
use error::FrameBuildError;
use stage::Stage;
use system_gesture_filter::{SystemGestureFilter, TouchAdmission};
use ui_draw::picture::UiPicture;

/// Сколько раз за кадр повторяется «синхронизация → события элементов → действия». Действие элемента меняет
/// состояние, а оно — другие элементы; три прохода покрывают цепочку «шкала → EV → «солнце»/панель».
///
/// Это технический предел цикла (защита от бесконечной цепочки действий), а не число экрана: он не зависит от
/// раскладки, размеров и стиля, поэтому остаётся константой, а не полем конфига.
const MAX_SETTLE_PASSES: usize = 3;

/// Наносекунд в миллисекунде (конфиг хранит паузы в мс, часы экрана — в нс).
const NANOS_PER_MS: f64 = 1_000_000.0;

/// Допустимое относительное расхождение `px_per_pt` рендера и сцены (поправка на `f32`).
const SCALE_TOLERANCE: f64 = 1.0e-4;

/// Вход касаний экрана.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScreenTouch {
    /// Касание в пикселях surface с меткой времени.
    Sample(TouchSample),
    /// Разрыв потока касаний (`ShellEvent::TouchGap`): идущие жесты отменяются без действий.
    Gap,
}

/// Монотонные часы экрана: время в элементы идёт только вперёд (событие с меткой раньше кадра — на время кадра).
#[derive(Debug, Clone, Copy)]
struct TimeKeeper {
    last: MonotonicNanos,
}

impl TimeKeeper {
    fn observe(&mut self, moment: MonotonicNanos) -> MonotonicNanos {
        self.last = self.last.max(moment);
        self.last
    }
}

/// Экран камеры.
pub struct CameraScreen {
    config: ScreenConfig,
    tokens: UiKitTokens,
    motion: MotionPreference,
    state: ScreenState,
    /// Сцена; `None` до первого кадра и после ошибки, потребовавшей пересборки.
    stage: Option<Stage>,
    /// Строилась ли сцена хоть раз успешно: отличает касания «до первого кадра» от касаний «пока сцену
    /// пересобирают».
    stage_ever_built: bool,
    /// Раньше этого момента сцену не пересобирают (пауза после ошибки); `None` — пауза не идёт.
    rebuild_not_before: Option<MonotonicNanos>,
    blink: CaptureBlinkWidget,
    /// Размытие удержанного кадра и поворот-карточка при смене камеры (сессия 13).
    veil: TransitionVeil,
    effects: EffectQueue,
    clock: TimeKeeper,
    diagnostics: ScreenDiagnostics,
    light: LightDirection,
    /// Зоны системных жестов и указатели, отданные системе (фильтр входа касаний).
    system_gestures: SystemGestureFilter,
}

impl CameraScreen {
    /// Экран с проверенными конфигом и токенами. Сцена строится на первом кадре (там известен размер surface).
    pub fn new(config: ScreenConfig, tokens: UiKitTokens, motion: MotionPreference) -> Result<Self, ScreenBuildError> {
        tokens.validate()?;
        // Диаметр затвора — токен, а не раскладка: сверка кнопки белого с ним возможна только здесь.
        config
            .widgets
            .white_button
            .check_clearance(&config.widgets.bottom, tokens.shutter.outer_diameter_pt)?;
        let state = ScreenState::new(&config)?;
        let light = LightDirection::from_angle_degrees(tokens.glass.button.specular.rest_angle_deg);
        let veil = TransitionVeil::new(&config.zoom.transition);
        Ok(Self {
            effects: EffectQueue::new(config.widgets.effect_queue_capacity),
            config,
            tokens,
            motion,
            state,
            stage: None,
            stage_ever_built: false,
            rebuild_not_before: None,
            blink: CaptureBlinkWidget::default(),
            veil,
            clock: TimeKeeper {
                last: MonotonicNanos::from_nanos(0),
            },
            diagnostics: ScreenDiagnostics::default(),
            light,
            system_gestures: SystemGestureFilter::default(),
        })
    }

    /// Зоны обязательных системных жестов по краям surface (пиксели). Касание, начатое в зоне, принадлежит
    /// системе и экраном не обрабатывается (см. [`system_gesture_filter`]). Это фильтр входа, а не состояние
    /// экрана, поэтому метод фасада, а не действие редьюсера. Идущие касания решение не меняют.
    pub fn set_system_gesture_zones(&mut self, zones: SystemGestureZones) {
        self.system_gestures.set_zones(zones);
    }

    /// Зоны, которые экран применяет сейчас.
    pub fn system_gesture_zones(&self) -> SystemGestureZones {
        self.system_gestures.zones()
    }

    /// Касание с экрана. Пока сцены нет, касание теряется со счётчиком, а не молча: до первого успешного
    /// кадра — `touches_dropped_before_first_frame`, после ошибки или смены размера —
    /// `touches_dropped_while_stage_missing`.
    ///
    /// Касание, начатое в зоне системного жеста, не доходит до сцены вообще (`touches_ignored_in_system_gesture_zone`
    /// считает касания, а не образцы). Разрыв потока забывает такие касания даже без сцены.
    pub fn handle_touch(&mut self, input: ScreenTouch) {
        if matches!(input, ScreenTouch::Gap) {
            self.system_gestures.reset_pointers();
        }
        let Some(stage) = &mut self.stage else {
            if self.stage_ever_built {
                self.diagnostics.touches_dropped_while_stage_missing += 1;
            } else {
                self.diagnostics.touches_dropped_before_first_frame += 1;
            }
            return;
        };
        match input {
            ScreenTouch::Sample(sample) => {
                match self.system_gestures.admit(self.state.surface_size(), &sample) {
                    TouchAdmission::Process => {}
                    TouchAdmission::IgnoredStart => {
                        self.diagnostics.touches_ignored_in_system_gesture_zone += 1;
                        return;
                    }
                    TouchAdmission::IgnoredContinuation => return,
                }
                self.diagnostics.touches_received += 1;
                stage.kit_mut().scene_mut().handle_touch(&sample);
            }
            ScreenTouch::Gap => {
                self.diagnostics.touch_gaps += 1;
                stage.kit_mut().scene_mut().handle_pointer_gap();
            }
        }
    }

    /// Внешнее действие: факты камеры, ориентация, гравитация, клавиша громкости, итог снимка.
    pub fn handle_action(&mut self, action: ScreenAction) {
        self.apply(action);
    }

    /// Итог «сохранён» вместе с картинкой миниатюры (14g). Картинка и итог входят в экран ОДНИМ вызовом:
    /// частичной доставки (картинка без итога или итог без картинки) не бывает. Редьюсер остаётся чистым:
    /// пиксели попадают в состояние через «входящий» ящик трекера миниатюры, а сам итог идёт обычным
    /// `CaptureFinished(Saved)`. Итог, отвергнутый как устаревший, картинку не ставит (учёт в диагностике).
    pub fn handle_saved_with_picture(&mut self, request: ShotRequestId, picture: UiPicture) {
        self.state.thumbnail_tracker.stage_incoming(request, picture);
        self.apply(ScreenAction::CaptureFinished {
            request,
            outcome: CaptureOutcome::Saved,
        });
        // Ящик обязан быть пуст и при отказе редьюсера: ненужная картинка не должна дожить до чужого итога.
        self.state.thumbnail_tracker.discard_incoming();
    }

    /// Эффекты, накопленные с прошлого вызова (исполняет приложение).
    pub fn drain_effects(&mut self) -> Vec<Effect> {
        self.effects.drain()
    }

    /// Масштаб рамки фокуса сейчас (проба для тестов повторного тапа).
    #[cfg(test)]
    pub(crate) fn focus_frame_scale(&self) -> f64 {
        self.stage
            .as_ref()
            .map_or(f64::NAN, |stage| stage.focus().appearance_scale(self.clock.last))
    }

    /// Фаза рамки фокуса сейчас (проба для тестов: рамка не должна жить дольше индикатора в состоянии).
    #[cfg(test)]
    pub(crate) fn focus_frame_phase(&self) -> Option<ui_kit::focus_frame::FocusPhase> {
        self.stage.as_ref().map(|stage| stage.focus().frame_phase())
    }

    /// Сцена экрана (проба для тестов соседства областей касания); `None` — сцены ещё нет (нет размера поверхности).
    #[cfg(test)]
    pub(crate) fn scene_for_tests(&self) -> Option<&ui_core::scene::UiScene<crate::widget::intent::ScreenGesture>> {
        self.stage.as_ref().map(stage::Stage::scene)
    }

    /// Центры видимых узлов сцены с подписью `label` в pt поверхности (проба для тестов касаний раздела).
    #[cfg(test)]
    pub(crate) fn node_centers(&self, label: &str) -> Vec<ui_core::geometry::Point> {
        self.stage
            .as_ref()
            .map_or_else(Vec::new, |stage| stage.node_centers(label))
    }

    /// Положение линейки строки раздела `slot` в шагах (проба для тестов).
    #[cfg(test)]
    pub(crate) fn calibration_row_position_steps(&self, slot: u8) -> Option<f64> {
        self.stage
            .as_ref()?
            .calibration()?
            .row_position_steps(slot, self.clock.last)
    }

    /// Смещение прокрутки списка ручек, pt (проба для тестов).
    #[cfg(test)]
    pub(crate) fn calibration_scroll_offset(&self) -> Option<f64> {
        Some(self.stage.as_ref()?.calibration()?.scroll_offset(self.clock.last))
    }

    /// Значение шкалы панели Exposure, EV (проба для тестов «без эха»).
    #[cfg(test)]
    pub(crate) fn slider_value_ev(&self) -> f64 {
        self.stage
            .as_ref()
            .map_or(f64::NAN, |stage| stage.card().ruler_value_ev(self.clock.last))
    }

    /// Значение «солнца» рамки фокуса, EV (проба для тестов смены пределов без пересоздания рамки).
    #[cfg(test)]
    pub(crate) fn sun_value_ev(&self) -> f64 {
        self.stage
            .as_ref()
            .map_or(f64::NAN, |stage| stage.focus().sun_value_ev(self.clock.last))
    }

    /// Где на поверхности стоит круг колеса цвета (pt): центр, сторона и радиус области толчка. `None` — круга
    /// сейчас нет: не режим калибровки, вид шкал (колёса не выбраны), нет размера поверхности или колонка слишком
    /// низкая для круга. Нужен сквозным тестам, которые ведут настоящий палец по кругу, и не раскрывает устройство
    /// виджета: ответ считает та же функция, по которой строится раздел.
    pub fn wheel_pad_placement(&self) -> Option<crate::calibration::WheelPadPlacement> {
        let calibration = self.state.calibration();
        if calibration.control_view() != crate::calibration::ControlViewKind::Wheels {
            return None;
        }
        crate::calibration::wheel_pad_placement(
            calibration.layout()?,
            &self.config.calibration.widgets,
            self.state.pixels_per_point()?,
        )
        .ok()
    }

    pub fn state(&self) -> &ScreenState {
        &self.state
    }

    /// Диагностика (снимок счётчиков).
    pub fn diagnostics(&self) -> ScreenDiagnostics {
        let mut snapshot = self.diagnostics.clone();
        snapshot.effects_queued = self.effects.queued();
        snapshot.effects_overflowed = self.effects.overflowed();
        snapshot.standby_demand_transitions = self.state.zoom().standby_transitions();
        snapshot.thumbnail = self.state.thumbnail_counters();
        if let Some(stage) = &self.stage {
            snapshot.echo_events_suppressed = stage.echo_suppressed();
            snapshot.ignored_sun_pans = stage.focus().ignored_sun_pans();
            snapshot.invalid_exposure_values = stage.invalid_exposure_values();
            snapshot.invalid_knob_values = stage.invalid_knob_values();
            snapshot.invalid_white_values = stage.invalid_white_values();
            snapshot.preset_chips_overflowed = stage.overflowed_preset_chips();
            snapshot.text_layouts = stage.text_layouts();
            snapshot.text_cache_evictions = stage.text_cache_evictions();
        }
        snapshot
    }

    /// Единственная точка изменения состояния: редьюсер → очередь эффектов → счётчики.
    fn apply(&mut self, action: ScreenAction) {
        let transition = reduce(&self.state, action, &self.config);
        match transition.outcome {
            ActionOutcome::Applied => self.diagnostics.actions_applied += 1,
            ActionOutcome::NoChange(_) => self.diagnostics.actions_no_change += 1,
            ActionOutcome::Rejected(_) => self.diagnostics.actions_rejected += 1,
        }
        self.state = transition.state;
        for effect in transition.effects {
            if matches!(effect, Effect::Capture(_)) {
                self.blink.trigger(self.clock.last);
            }
            // Переполнение уже посчитано очередью; здесь отдельная реакция не нужна.
            let _outcome: PushOutcome = self.effects.push(effect);
        }
    }

    /// Применить выход виджетов: действия — в редьюсер, хаптика — эффектом.
    fn flush(&mut self, outputs: Outputs) {
        for pattern in outputs.haptics {
            let _outcome = self.effects.push(Effect::Haptic(pattern));
        }
        for action in outputs.actions {
            self.apply(action);
        }
    }

    fn context<'a>(&'a self, geometry: &'a ScreenGeometry, px: PxPerPt, now: MonotonicNanos) -> Ctx<'a> {
        Ctx {
            state: &self.state,
            config: &self.config,
            tokens: &self.tokens,
            motion: self.motion,
            geometry,
            px,
            now,
        }
    }

    /// Сцена под кадр: прежняя, если размер и масштаб те же, иначе новая (после `SurfaceResized`).
    ///
    /// Смена размера или масштаба посреди жеста отменяет жест явным `Cancelled` (разрыв потока старой
    /// сцены, фазы учтены в счётчиках), а не теряет его молча. Любая ошибка сборки — «сцену сбросить»
    /// (кроме непригодного размера и паузы: сцену они не трогают), и следующая попытка ждёт паузу.
    fn ensure_stage(&mut self, request: &UiFrameRequest, now: MonotonicNanos) -> Result<PxPerPt, FrameBuildError> {
        let usable = |value: f32| value.is_finite() && value >= 1.0 && value <= u32::MAX as f32;
        let (width, height) = (request.logical_size.width, request.logical_size.height);
        if !usable(width) || !usable(height) {
            // Сцена не тронута: если она была, остаётся (размер surface мог мигнуть на один кадр).
            return Err(FrameBuildError::UnusableSurface {
                width_px: width,
                height_px: height,
            });
        }
        let surface = SurfaceSize {
            width_px: width.round() as u32,
            height_px: height.round() as u32,
        };
        let px = PxPerPt::new(request.px_per_pt).map_err(FrameBuildError::from)?;
        if self
            .stage
            .as_ref()
            .is_some_and(|stage| stage.matches(surface, request.px_per_pt))
        {
            return Ok(px);
        }
        self.check_rebuild_backoff(now)?;
        self.retire_stage();
        self.diagnostics.stage_rebuilds += 1;
        if let Err(error) = self.build_stage(request, surface, px, now) {
            self.start_rebuild_backoff(now);
            return Err(error);
        }
        Ok(px)
    }

    /// Строит сцену под `surface` и кладёт её в `self.stage`.
    fn build_stage(
        &mut self,
        request: &UiFrameRequest,
        surface: SurfaceSize,
        px: PxPerPt,
        now: MonotonicNanos,
    ) -> Result<(), FrameBuildError> {
        // Раскладка — владелец геометрии: сначала редьюсер узнаёт размер (и шлёт `Viewfinder` рендеру).
        if self.state.layout().is_none() || self.state_surface() != Some(surface) {
            let before = self.diagnostics.actions_rejected;
            self.apply(ScreenAction::SurfaceResized(surface));
            if self.diagnostics.actions_rejected != before {
                return Err(FrameBuildError::SurfaceRejected(
                    crate::outcome::Rejection::InvalidSurface,
                ));
            }
        }
        let layout = *self.state.layout().ok_or(crate::widget::error::WidgetError::NoLayout)?;
        let geometry = ScreenGeometry::new(&layout, &self.config.widgets, &self.config.zoom, &self.tokens);
        let geometry = Arc::new(geometry);
        let stage = Stage::build(
            &self.context(&geometry, px, now),
            surface,
            request.px_per_pt,
            Arc::clone(&geometry),
        )?;
        let scene_scale = stage.kit().scene().point_scale().pixels_per_point();
        if (scene_scale - f64::from(request.px_per_pt)).abs() > SCALE_TOLERANCE * scene_scale {
            return Err(FrameBuildError::ScaleMismatch {
                requested: request.px_per_pt,
                scene: scene_scale,
            });
        }
        self.stage = Some(stage);
        self.stage_ever_built = true;
        self.rebuild_not_before = None;
        Ok(())
    }

    /// Пауза после ошибки ещё идёт: пересборку не начинаем, кадр отдаёт ошибку (счётчик, не тишина).
    fn check_rebuild_backoff(&mut self, now: MonotonicNanos) -> Result<(), FrameBuildError> {
        let Some(not_before) = self.rebuild_not_before else {
            return Ok(());
        };
        if now >= not_before {
            return Ok(());
        }
        self.diagnostics.frames_skipped_rebuild_backoff += 1;
        let remaining_ms = (not_before.as_nanos() - now.as_nanos()) as f64 / NANOS_PER_MS;
        Err(FrameBuildError::RebuildBackoff { remaining_ms })
    }

    /// Запускает паузу до следующей попытки пересборки (длина — `stage_rebuild_backoff_ms`).
    fn start_rebuild_backoff(&mut self, now: MonotonicNanos) {
        // Конфиг проверен (`validate`): значение конечно и > 0; приведение к целым наносекундам безопасно.
        let backoff_nanos = (self.config.widgets.stage_rebuild_backoff_ms * NANOS_PER_MS) as i64;
        self.rebuild_not_before = Some(MonotonicNanos::from_nanos(now.as_nanos().saturating_add(backoff_nanos)));
    }

    /// Снимает текущую сцену (если есть): её идущие жесты отменяются явным `Cancelled`.
    fn retire_stage(&mut self) {
        if let Some(stage) = self.stage.take() {
            self.retire(stage);
        }
    }

    fn retire(&mut self, mut stage: Stage) {
        self.diagnostics.gestures_cancelled_by_stage_change += stage.cancel_gestures(&mut self.diagnostics.gestures);
    }

    fn state_surface(&self) -> Option<SurfaceSize> {
        self.state.surface_size()
    }

    fn build_frame(&mut self, request: &UiFrameRequest) -> Result<DrawList, FrameBuildError> {
        let now = self.clock.observe(request.present_time);
        let px = self.ensure_stage(request, now)?;
        let Some(mut stage) = self.stage.take() else {
            return Err(WidgetError::NoLayout.into());
        };
        // Фаза 1: изменение сцены (жесты, синхронизация, анимации). Ошибка здесь — сцена в неизвестном
        // состоянии: сбрасываем и пересобираем с паузой.
        if let Err(error) = self.update_scene(&mut stage, px, now) {
            self.retire(stage);
            self.start_rebuild_backoff(now);
            return Err(error.into());
        }
        // Фаза 2: сборка списка рисования только читает сцену. Ошибка вида сцену не портит: она остаётся
        // со всеми идущими жестами, следующий кадр пробует снова.
        self.veil.advance(
            request.backdrop,
            self.state.zoom().last_flip(),
            now,
            self.motion,
            &self.config.zoom.transition,
        );
        let geometry = stage.geometry();
        let drawn = stage.draw(&self.context(&geometry, px, now), &self.blink, &self.veil);
        self.stage = Some(stage);
        match drawn {
            Ok(items) => Ok(DrawList {
                items,
                light: self.light,
            }),
            Err(error) => {
                self.diagnostics.frame_failures_scene_kept += 1;
                Err(error.into())
            }
        }
    }

    /// Всё, что меняет сцену за кадр: часы и жесты, редьюсер, синхронизация, анимации.
    fn update_scene(&mut self, stage: &mut Stage, px: PxPerPt, now: MonotonicNanos) -> Result<(), WidgetError> {
        // Часы сцены и таймеры жестов: долгий тап срабатывает и при неподвижном пальце.
        let geometry = stage.geometry();
        stage.kit_mut().scene_mut().begin_frame(now);
        for event in stage.kit_mut().scene_mut().drain_gesture_events() {
            let at = self.clock.observe(event.time);
            Stage::count_gesture(&mut self.diagnostics.gestures, &event);
            let mut outputs = Outputs::default();
            let handled = stage.route(&event, &self.context(&geometry, px, at), &mut outputs)?;
            if !handled {
                self.diagnostics.unhandled_gestures += 1;
            }
            self.flush(outputs);
        }
        self.apply(ScreenAction::Tick(now));
        for _pass in 0..MAX_SETTLE_PASSES {
            stage.sync(&self.context(&geometry, px, now))?;
            let mut outputs = Outputs::default();
            stage.advance(&self.context(&geometry, px, now), &mut outputs)?;
            self.blink.advance(&self.config.widgets.overlay, now);
            let produced_actions = !outputs.actions.is_empty();
            self.flush(outputs);
            if !produced_actions {
                break;
            }
        }
        // Последняя синхронизация: состояние после действий элементов должно попасть в рисование.
        stage.sync(&self.context(&geometry, px, now))
    }

    /// Кадр: см. порядок и разбор ошибок в документации модуля. Ошибка кадра не роняет показ камеры — рендер
    /// её считает.
    pub fn produce_frame(&mut self, request: &UiFrameRequest) -> Result<DrawList, UiFrameError> {
        let built = self.build_frame(request).map_err(|error| {
            self.diagnostics.frame_failures += 1;
            self.diagnostics.last_failure = Some(error.to_string());
            UiFrameError::SourceFailed {
                reason: error.to_string(),
            }
        })?;
        // Свой список проверяем так же, как проверит рендер: ошибка словаря — это ошибка источника.
        if let Err(error) = built.validate() {
            self.diagnostics.frame_failures += 1;
            self.diagnostics.last_failure = Some(error.to_string());
            return Err(UiFrameError::InvalidDrawList(error));
        }
        self.diagnostics.frames_produced += 1;
        Ok(built)
    }
}

impl UiFrameSource for CameraScreen {
    fn name(&self) -> &'static str {
        "camera_screen"
    }

    fn produce_frame(&mut self, request: &UiFrameRequest) -> Result<DrawList, UiFrameError> {
        CameraScreen::produce_frame(self, request)
    }
}

#[cfg(test)]
#[path = "effect_queue_tests.rs"]
mod effect_queue_tests;
#[cfg(test)]
#[path = "screen_calibration_bands_tests.rs"]
mod screen_calibration_bands_tests;
#[cfg(test)]
#[path = "screen_calibration_finger_focus_tests.rs"]
mod screen_calibration_finger_focus_tests;
#[cfg(test)]
#[path = "screen_calibration_finger_tests.rs"]
mod screen_calibration_finger_tests;
#[cfg(test)]
#[path = "screen_calibration_menu_tests.rs"]
mod screen_calibration_menu_tests;
#[cfg(test)]
mod screen_calibration_neighbourhood_tests;
#[cfg(test)]
mod screen_calibration_safe_area_tests;
#[cfg(test)]
#[path = "screen_calibration_section_tests.rs"]
mod screen_calibration_section_tests;
#[cfg(test)]
#[path = "screen_calibration_strip_tests.rs"]
mod screen_calibration_strip_tests;
#[cfg(test)]
#[path = "screen_calibration_tests.rs"]
mod screen_calibration_tests;
#[cfg(test)]
#[path = "screen_menu_glide_tests.rs"]
mod screen_menu_glide_tests;
#[cfg(test)]
mod screen_photo_touch_reach_tests;

#[cfg(test)]
#[path = "screen_calibration_wheels_tests.rs"]
mod screen_calibration_wheels_tests;

#[cfg(test)]
#[path = "screen_calibration_wheel_feedback_tests.rs"]
mod screen_calibration_wheel_feedback_tests;
#[cfg(test)]
#[path = "screen_calibration_wheels_pad_tests.rs"]
mod screen_calibration_wheels_pad_tests;
#[cfg(test)]
#[path = "screen_calibration_zero_magnet_tests.rs"]
mod screen_calibration_zero_magnet_tests;
#[cfg(test)]
#[path = "screen_card_close_tests.rs"]
mod screen_card_close_tests;
#[cfg(test)]
#[path = "screen_card_tests.rs"]
mod screen_card_tests;

#[cfg(test)]
#[path = "screen_draw_tests.rs"]
mod screen_draw_tests;

#[cfg(test)]
#[path = "screen_card_layout_tests.rs"]
mod screen_card_layout_tests;
#[cfg(test)]
#[path = "screen_failure_tests.rs"]
mod screen_failure_tests;
#[cfg(test)]
#[path = "screen_interaction_tests.rs"]
mod screen_interaction_tests;
#[cfg(test)]
#[path = "screen_kit_tests.rs"]
mod screen_kit_tests;
#[cfg(test)]
#[path = "screen_problem_banner_tests.rs"]
mod screen_problem_banner_tests;

#[cfg(test)]
#[path = "screen_section_surface_tests.rs"]
mod screen_section_surface_tests;

#[cfg(test)]
#[path = "screen_sheet_tests.rs"]
mod screen_sheet_tests;

#[cfg(test)]
#[path = "screen_styles_card_tests.rs"]
mod screen_styles_card_tests;
#[cfg(test)]
#[path = "screen_system_gesture_tests.rs"]
mod screen_system_gesture_tests;
#[cfg(test)]
#[path = "screen_tests.rs"]
mod screen_tests;
#[cfg(test)]
#[path = "screen_text_cache_tests.rs"]
mod screen_text_cache_tests;
#[cfg(test)]
#[path = "screen_thumbnail_tests.rs"]
mod screen_thumbnail_tests;
#[cfg(test)]
#[path = "screen_white_auto_tests.rs"]
mod screen_white_auto_tests;
#[cfg(test)]
#[path = "screen_white_button_gesture_tests.rs"]
mod screen_white_button_gesture_tests;
#[cfg(test)]
#[path = "screen_white_button_layout_tests.rs"]
mod screen_white_button_layout_tests;
#[cfg(test)]
#[path = "screen_white_button_tests.rs"]
mod screen_white_button_tests;
#[cfg(test)]
#[path = "screen_white_card_tests.rs"]
mod screen_white_card_tests;
#[cfg(test)]
#[path = "screen_white_follow_tests.rs"]
mod screen_white_follow_tests;
#[cfg(test)]
#[path = "screen_white_section_tests.rs"]
mod screen_white_section_tests;
#[cfg(test)]
#[path = "screen_zoom_flip_tests.rs"]
mod screen_zoom_flip_tests;
#[cfg(test)]
#[path = "screen_zoom_gesture_tests.rs"]
mod screen_zoom_gesture_tests;
#[cfg(test)]
#[path = "screen_zoom_standby_tests.rs"]
mod screen_zoom_standby_tests;
#[cfg(test)]
#[path = "screen_zoom_tests.rs"]
mod screen_zoom_tests;
#[cfg(test)]
#[path = "screen_zoom_transition_tests.rs"]
mod screen_zoom_transition_tests;
#[cfg(test)]
mod test_rig;
#[cfg(test)]
mod white_test_support;
