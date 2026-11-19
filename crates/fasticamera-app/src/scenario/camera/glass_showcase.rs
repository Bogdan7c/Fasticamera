//! Демо-сцена стекла для smoke-сценария `glass` (сессия 11): стекло поверх живого видоискателя.
//!
//! # Что это
//! «Стенд» материала Liquid Glass, а не экран камеры (его делает сессия 12): элементы похожи на экран iOS
//! размером и местом (раскладка в pt, привязка к низу — числа в [`GlassShowcaseConfig`]), но набор подобран
//! так, чтобы на живом видео проверить все свойства стекла:
//! 1. капсула режимов с подписью «PHOTO» (стекло кнопочного пресета, подпись — `TextItem` поверх);
//! 2. две круглые кнопки (вспышка, переворот), которые по сценарию «нажимаются»: `PressState.amount` едет
//!    пружиной `snappy`, масштаб члена запекает строитель (`PressParamsPx::scale_for`);
//! 3. ряд из трёх кружков зума в ОДНОЙ группе: периодически сходятся и расходятся — слияние «как капли»;
//! 4. панель-морф: вырастает из кнопки меню пружиной `camera.sheet`, появляется «материализацией»
//!    (`effect` 0 → 1, а не прозрачностью);
//! 5. обычные фигуры и текст: жёлтая рамка фокуса и плашка «AE/AF LOCK».
//!
//! # Архитектура
//! * `config` и `timeline` — все числа сцены (serde, `deny_unknown_fields`, `validate()`);
//! * `scene` — `ui_core::scene::UiScene`: узлы, вёрстка, пружины свойств узлов;
//! * `motion` — пружины нажатий и исполнение событий расписания;
//! * `build` — единственное место перевода «сцена → `DrawList`» (через `node_placement_px`);
//! * этот файл — [`GlassShowcase`]: `UiFrameSource` со своим жизненным циклом сцены.
//!
//! # Время
//! Всё — функция `UiFrameRequest::present_time`. Фаза цикла = время по модулю периода; события расписания
//! исполняются во время СОБЫТИЯ (пружины стартуют не от кадра), поэтому 60 и 120 Гц дают одинаковый список
//! в общие моменты. Сцена пересоздаётся при первом кадре, смене размера или масштаба и скачке времени
//! (назад или больше чем на период): состояние цикла тогда восстанавливается проигрыванием событий текущего
//! цикла с его начала — сцена всегда согласована с часами.

use std::sync::Arc;

use shell_contract::gravity::GravitySlot;
use ui_core::time::MonotonicNanos;
use ui_draw::draw_list::DrawList;
use ui_draw::frame_source::{UiFrameError, UiFrameRequest, UiFrameSource};
use ui_draw::light::{LightDirection, TiltLight};
use ui_kit::tokens::UiKitTokens;

#[path = "glass_showcase_build.rs"]
mod build;
#[path = "glass_showcase_config.rs"]
pub mod config;
#[path = "glass_showcase_error.rs"]
pub mod error;
#[path = "glass_showcase_motion.rs"]
mod motion;
#[path = "glass_showcase_scene.rs"]
mod scene;
#[path = "glass_showcase_timeline.rs"]
pub mod timeline;

use build::{DrawContext, FrameFacts, ShowcaseStyles, ShowcaseTexts, build_draw_list};
use config::GlassShowcaseConfig;
use error::{ShowcaseConfigError, ShowcaseError};
use motion::ShowcaseMotion;
use scene::{ShowcaseScene, surface_from_pixels};
use timeline::Schedule;

/// Допустимое относительное расхождение `px_per_pt` рендера и сцены (поправка на `f32`).
const SCALE_TOLERANCE: f64 = 1.0e-4;

/// Сцена со всем, что привязано к размеру экрана и масштабу.
struct Stage {
    scene: ShowcaseScene,
    texts: ShowcaseTexts,
    /// Размер и масштаб, под которые построена сцена.
    surface_px: (u32, u32),
    px_per_pt: f32,
}

/// Демо-сцена стекла: источник списка рисования для потока рендера.
pub struct GlassShowcase {
    config: GlassShowcaseConfig,
    styles: ShowcaseStyles,
    schedule: Schedule,
    motion: ShowcaseMotion,
    tilt: TiltLight,
    gravity: Option<Arc<GravitySlot>>,
    /// Сцена; `None` до первого кадра и после ошибки (следующий кадр построит заново).
    stage: Option<Stage>,
    /// Время предыдущего кадра: по нему ловятся скачки времени.
    last_frame: Option<MonotonicNanos>,
    /// Сквозной индекс следующего необработанного события расписания.
    next_event_index: i64,
}

impl GlassShowcase {
    /// Создать сцену. `tokens` даёт пружины (`snappy`, `camera.sheet`) и пресеты стекла (кнопка, панель);
    /// `gravity` — ящик гравитации для света от наклона (`None` — свет в покое).
    pub fn new(
        config: GlassShowcaseConfig,
        tokens: &UiKitTokens,
        gravity: Option<Arc<GravitySlot>>,
    ) -> Result<Self, ShowcaseError> {
        config.validate()?;
        tokens.validate()?;
        let styles = ShowcaseStyles {
            button: tokens.glass.button,
            panel: tokens.glass.panel,
        };
        // Свет в покое — угол покоя пресета кнопки; панель и кнопки одного кадра освещены одним светом.
        let rest_light = LightDirection::from_angle_degrees(styles.button.specular.rest_angle_deg);
        let tilt = TiltLight::new(config.tilt_light, rest_light).map_err(ShowcaseConfigError::from)?;
        let motion = ShowcaseMotion::new(
            tokens,
            config.motion,
            config.timeline.unit_rest_floor,
            MonotonicNanos::from_nanos(0),
        )?;
        Ok(Self {
            schedule: Schedule::from_timeline(&config.timeline),
            config,
            styles,
            motion,
            tilt,
            gravity,
            stage: None,
            last_frame: None,
            next_event_index: 0,
        })
    }

    /// Сцена под текущий кадр: прежняя, если ничего не изменилось, иначе новая, приведённая к часам.
    fn stage_for(&mut self, request: &UiFrameRequest) -> Result<Stage, ShowcaseError> {
        let surface = surface_from_pixels(request.logical_size.width, request.logical_size.height)?;
        let surface_px = (surface.width_px, surface.height_px);
        let now = request.present_time;
        let time_jumped = self.last_frame.is_some_and(|last| {
            now < last || now.as_nanos().saturating_sub(last.as_nanos()) > self.schedule.period_ns()
        });
        if let Some(stage) = self.stage.take()
            && !time_jumped
            && stage.surface_px == surface_px
            && stage.px_per_pt.to_bits() == request.px_per_pt.to_bits()
        {
            return Ok(stage);
        }
        // Новая сцена: покой в начале текущего цикла; события цикла до `now` проиграет `run_due_events`.
        let cycle_start = self.schedule.cycle_start(now);
        let mut scene = ShowcaseScene::new(&self.config.layout, self.config.motion, surface)?;
        scene.advance_to(cycle_start);
        self.motion.reset(cycle_start)?;
        self.next_event_index = self.schedule.first_index_of_cycle(cycle_start);
        Ok(Stage {
            texts: ShowcaseTexts::layout(&self.config.look, request.px_per_pt)?,
            scene,
            surface_px,
            px_per_pt: request.px_per_pt,
        })
    }

    /// Исполнить события расписания, наступившие к `now`, каждое во время самого события.
    fn run_due_events(&mut self, stage: &mut Stage, now: MonotonicNanos) -> Result<(), ShowcaseError> {
        loop {
            let (event_time, event) = self.schedule.event(self.next_event_index);
            if event_time > now {
                return Ok(());
            }
            stage.scene.advance_to(event_time);
            self.motion.handle(&mut stage.scene, event, event_time)?;
            self.next_event_index += 1;
        }
    }

    /// Направление света на этот кадр: по ящику гравитации (время датчика, сглаживание в `TiltLight`) или покой.
    fn light_for_frame(&mut self) -> LightDirection {
        if let Some(slot) = &self.gravity {
            // Результат (учтено, дубль, время назад) кадру не нужен: счётчики живут в `TiltLight`.
            let _update = self.tilt.apply_reading(slot.read());
        }
        self.tilt.direction()
    }

    fn build_frame(&mut self, request: &UiFrameRequest, stage: &mut Stage) -> Result<DrawList, ShowcaseError> {
        let now = request.present_time;
        let scene_scale = stage.scene.px_per_pt();
        if (scene_scale - f64::from(request.px_per_pt)).abs() > SCALE_TOLERANCE * scene_scale {
            return Err(ShowcaseError::ScaleMismatch {
                requested: request.px_per_pt,
                scene: scene_scale,
            });
        }
        self.run_due_events(stage, now)?;
        stage.scene.advance_to(now);
        let facts = FrameFacts {
            press: self.motion.press_amounts(now),
            panel_progress: stage.scene.panel_progress(),
            light: self.light_for_frame(),
        };
        let context = DrawContext {
            look: &self.config.look,
            styles: &self.styles,
            texts: &stage.texts,
            px_per_pt: request.px_per_pt,
        };
        build_draw_list(&context, stage.scene.nodes(), &stage.scene.visuals(), &facts)
    }

    fn produce(&mut self, request: &UiFrameRequest) -> Result<DrawList, ShowcaseError> {
        let mut stage = self.stage_for(request)?;
        let list = self.build_frame(request, &mut stage)?;
        // Сцена возвращается на место только после успешного кадра: при ошибке следующий кадр начнёт с нуля.
        self.stage = Some(stage);
        self.last_frame = Some(request.present_time);
        Ok(list)
    }
}

impl UiFrameSource for GlassShowcase {
    fn name(&self) -> &'static str {
        "glass_showcase"
    }

    fn produce_frame(&mut self, request: &UiFrameRequest) -> Result<DrawList, UiFrameError> {
        let list = self.produce(request).map_err(|error| UiFrameError::SourceFailed {
            reason: error.to_string(),
        })?;
        // Свой список проверяем так же, как проверит рендер: ошибка словаря — это ошибка источника.
        list.validate()?;
        Ok(list)
    }
}

#[cfg(test)]
#[path = "glass_showcase_tests.rs"]
mod tests;
