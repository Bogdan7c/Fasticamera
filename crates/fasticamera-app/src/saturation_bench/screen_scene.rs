//! Настоящие списки рисования экрана камеры (PHOTO и CALIBRATE) для работ показа стенда насыщения.
//!
//! Список строит НАСТОЯЩИЙ экран (`camera-screen`) на фактах продукта ([`ProductInputs`]), теми же путями, что в
//! приложении: действие → редьюсер → виджеты → список рисования. Подставных рисунков нет: стеклянные группы,
//! фигуры, подписи и значки — те, что рисует продукт на экране 1080×2412 на плотности этого экрана.
//!
//! # Что берётся у экрана
//! * сам список (`DrawList`) — на покоящемся экране (пружины и появления доехали);
//! * прямоугольник видоискателя: для PHOTO — кадр выбранного соотношения из раскладки, для CALIBRATE — весь
//!   экран (в разделе калибровки видоискатель на весь экран, как в продукте);
//! * запрос прибора (`ScopeRequest`) из эффектов раздела CALIBRATE: размер картинки и положение глаза берёт
//!   раскладка экрана, а не оценка — по нему приборы стенда считают картинку ровно такого размера.

use camera_screen::modes::CaptureMode;
use camera_screen::{CameraScreen, Effect, ScreenAction};
use gpu_vulkan::compositor::LogicalRect;
use scope_contract::{ScopeIntent, ScopeRequest};
use shell_contract::clock::MonotonicNanos;
use shell_contract::orientation::DeviceOrientation;
use ui_core::input::{PointScale, PointScaleRule};
use ui_draw::draw_list::{DrawItem, DrawList, ImageSource};
use ui_draw::frame_source::{BackdropContinuity, UiFrameRequest};
use ui_draw::geometry::PxSize;
use ui_draw::light::LightDirection;
use ui_kit::tokens::UiKitTokens;

use super::catalog::DisplayScreen;
use super::inputs::ProductInputs;
use super::report::SceneSummary;

/// Размер экрана стенда, px: OnePlus 11 в портрете.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceSizePx {
    pub width: u32,
    pub height: u32,
}

impl SurfaceSizePx {
    /// Экран OnePlus 11 (`layout.md` раздел 3).
    pub const ONEPLUS_11: Self = Self {
        width: 1080,
        height: 2412,
    };
}

/// Шаг кадров покоя, мс (60 Гц).
const FRAME_STEP_MS: i64 = 16;
/// Время покоя после действия: все пружины и появления доехали (самые долгие — шторка и поворот значков, < 1 с).
const SETTLE_MS: i64 = 1_200;

/// Почему список рисования не построен.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScreenPlanError {
    #[error("экран камеры не собран: {0}")]
    ScreenBuild(String),
    #[error("масштаб pt для ширины экрана {width_px} px: {reason}")]
    PointScale { width_px: u32, reason: String },
    #[error("экран не построил кадр: {0}")]
    Frame(String),
    #[error("после первого кадра у экрана нет раскладки ({0})")]
    NoLayout(&'static str),
    #[error("у экрана нет размера прямоугольника видоискателя ({0})")]
    EmptyViewfinderRect(&'static str),
}

/// Готовая сцена одного экрана.
pub struct ScreenScene {
    pub screen: DisplayScreen,
    pub draw_list: DrawList,
    /// Где лежит картинка видоискателя, px экрана.
    pub viewfinder: LogicalRect,
    /// Запрос прибора раздела (только CALIBRATE).
    pub scope_request: Option<ScopeRequest>,
    pub summary: SceneSummary,
}

/// Сцены обоих экранов для работ показа.
pub struct ScreenPlan {
    photo: ScreenScene,
    calibrate: ScreenScene,
}

impl ScreenPlan {
    /// Строит оба экрана на входах продукта.
    pub fn build(inputs: &ProductInputs, surface: SurfaceSizePx) -> Result<Self, ScreenPlanError> {
        Ok(Self {
            photo: build_scene(inputs, surface, DisplayScreen::Photo)?,
            calibrate: build_scene(inputs, surface, DisplayScreen::Calibrate)?,
        })
    }

    pub fn scene(&self, screen: DisplayScreen) -> &ScreenScene {
        match screen {
            DisplayScreen::Photo => &self.photo,
            DisplayScreen::Calibrate => &self.calibrate,
        }
    }
}

/// Плотность экрана: пикселей на pt по правилу продукта (`ui-core`, решение 82), а не константа здесь.
fn pixels_per_point(width_px: u32) -> Result<f32, ScreenPlanError> {
    PointScale::for_surface_width(PointScaleRule::default(), width_px)
        .map(|scale| scale.pixels_per_point() as f32)
        .map_err(|error| ScreenPlanError::PointScale {
            width_px,
            reason: error.to_string(),
        })
}

/// Кадры экрана по шагу 16 мс: время детерминировано, как у golden экрана.
struct FrameDriver {
    screen: CameraScreen,
    now_ms: i64,
    surface: SurfaceSizePx,
    px_per_pt: f32,
    last_list: DrawList,
}

impl FrameDriver {
    fn frame(&mut self) -> Result<(), ScreenPlanError> {
        let request = UiFrameRequest {
            present_time: MonotonicNanos::from_nanos(self.now_ms * 1_000_000),
            logical_size: PxSize::new(self.surface.width as f32, self.surface.height as f32),
            px_per_pt: self.px_per_pt,
            backdrop: BackdropContinuity::Live,
        };
        self.last_list = self
            .screen
            .produce_frame(&request)
            .map_err(|error| ScreenPlanError::Frame(error.to_string()))?;
        Ok(())
    }

    /// Действие и кадр, на котором экран синхронизирует его с виджетами.
    fn act(&mut self, action: ScreenAction) -> Result<(), ScreenPlanError> {
        self.screen.handle_action(action);
        self.frame()
    }

    /// Проходит `duration_ms` кадрами по 60 Гц.
    fn settle(&mut self, duration_ms: i64) -> Result<(), ScreenPlanError> {
        let end = self.now_ms + duration_ms;
        while self.now_ms < end {
            self.now_ms = (self.now_ms + FRAME_STEP_MS).min(end);
            self.frame()?;
        }
        Ok(())
    }
}

fn build_scene(
    inputs: &ProductInputs,
    surface: SurfaceSizePx,
    which: DisplayScreen,
) -> Result<ScreenScene, ScreenPlanError> {
    let px_per_pt = pixels_per_point(surface.width)?;
    let screen = CameraScreen::new(inputs.screen_config.clone(), UiKitTokens::default(), inputs.motion)
        .map_err(|error| ScreenPlanError::ScreenBuild(error.to_string()))?;
    let mut driver = FrameDriver {
        screen,
        now_ms: 0,
        surface,
        px_per_pt,
        last_list: DrawList::new(LightDirection::from_angle_degrees(45.0)),
    };
    // Как приложение: первый кадр собирает сцену, затем приходят факты камеры, затем экран успокаивается.
    driver.frame()?;
    driver.act(ScreenAction::CameraFactsChanged(inputs.camera_facts.clone()))?;
    driver.settle(SETTLE_MS)?;
    if which == DisplayScreen::Calibrate {
        if let Some(look_facts) = &inputs.look_facts {
            driver.act(ScreenAction::LookFactsChanged(Box::new(look_facts.clone())))?;
        }
        driver.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::Portrait))?;
        driver.act(ScreenAction::ModeSelected(CaptureMode::Calibrate))?;
        driver.settle(SETTLE_MS)?;
    }
    let scope_request = last_scope_request(&driver.screen.drain_effects());
    let viewfinder = viewfinder_rect(&driver, which)?;
    let summary = summarize(&driver.last_list);
    Ok(ScreenScene {
        screen: which,
        draw_list: driver.last_list,
        viewfinder,
        scope_request,
        summary,
    })
}

/// Последнее «показать прибор» среди эффектов экрана.
fn last_scope_request(effects: &[Effect]) -> Option<ScopeRequest> {
    effects.iter().rev().find_map(|effect| match effect {
        Effect::Scopes(ScopeIntent::Show(request)) => Some(*request),
        _ => None,
    })
}

/// Прямоугольник видоискателя в px экрана (см. модуль).
fn viewfinder_rect(driver: &FrameDriver, which: DisplayScreen) -> Result<LogicalRect, ScreenPlanError> {
    let SurfaceSizePx { width, height } = driver.surface;
    let rect = match which {
        DisplayScreen::Calibrate => LogicalRect {
            x: 0,
            y: 0,
            width,
            height,
        },
        DisplayScreen::Photo => {
            let state = driver.screen.state();
            let layout = state
                .layout()
                .ok_or(ScreenPlanError::NoLayout("PHOTO"))?
                .frame_rect(state.settings().aspect);
            let scale = f64::from(driver.px_per_pt);
            let (left, top) = ((layout.min_x() * scale).round(), (layout.min_y() * scale).round());
            let (right, bottom) = ((layout.max_x() * scale).round(), (layout.max_y() * scale).round());
            LogicalRect {
                x: left as u32,
                y: top as u32,
                width: (right - left) as u32,
                height: (bottom - top) as u32,
            }
        }
    };
    if rect.width == 0 || rect.height == 0 {
        return Err(ScreenPlanError::EmptyViewfinderRect(match which {
            DisplayScreen::Photo => "PHOTO",
            DisplayScreen::Calibrate => "CALIBRATE",
        }));
    }
    Ok(rect)
}

/// Состав списка рисования: сколько чего в сцене.
pub fn summarize(list: &DrawList) -> SceneSummary {
    let mut summary = SceneSummary::default();
    for item in &list.items {
        match item {
            DrawItem::Shape(_) => summary.shapes += 1,
            DrawItem::Glass(group) => {
                summary.glass_groups += 1;
                summary.glass_members += group.members.len() as u64;
            }
            DrawItem::Text(_) => summary.texts += 1,
            DrawItem::Icon(_) => summary.icons += 1,
            DrawItem::Image(image) => {
                summary.images += 1;
                if matches!(image.source, ImageSource::Live(_)) {
                    summary.live_images += 1;
                }
            }
        }
    }
    summary
}

#[cfg(test)]
#[path = "screen_scene_tests.rs"]
mod tests;
