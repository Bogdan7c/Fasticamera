//! Завеса перехода между камерами (решения 110, 112, 113): размытый удержанный кадр и поворот-карточка при
//! перевороте. Рисует UI поверх кадра, который рендер держит на смене объектива (`BackdropContinuity`), —
//! нового GPU-кода нет: размытие даёт то же стекло (`GlassRole::Frost`, пресет без линзы и блика).
//!
//! # Автомат размытия
//! * рендер сказал `HeldLastFrame` → размытие включено **сразу** (за кадр) и держится, пока кадр удержан;
//! * рендер вернулся к `Live` → размытие растворяется за `blur_fade_out_ms` (0,2 с, `[estimate]`, образец —
//!   смена режима iOS, `motion.md` M:108); после переворота — за `flip_fade_out_ms`;
//! * `NoCameraFrame` размытия не требует (под UI фон, размывать нечего).
//!
//! # Переворот
//! Метка переворота (`ZoomState::last_flip`) запускает карточку: размытый кадр делает полный оборот вокруг
//! вертикальной оси за `flip_rotate_ms`, вне карточки чёрный фон — вместо чёрной паузы iOS (решение 113).
//! Оборот изображён **сужением**: ширина окна на размытый кадр равна `|cos θ|` ширины кадра, содержимое внутри
//! окна не сжимается (стекло берёт фон там, где стоит; настоящего поворота текстуры без нового GPU-кода нет).
//! При `θ = 90°` ширина нулевая, преобразование вырождено, а список с таким преобразованием рендер отвергает
//! целиком — поэтому ниже порога `min_card_width_fraction` окно не рисуется (остаётся чёрный фон).
//! Reduce Motion: без оборота и чёрного, только размытие и растворение.

use ui_core::geometry::{Rect, Size};
use ui_core::time::{MonotonicNanos, MotionPreference};
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, PressState};
use ui_draw::frame_source::BackdropContinuity;
use ui_draw::geometry::PxSize;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::transform::Affine2;
use ui_kit::tokens::GlassRole;
use ui_kit::view::PxPerPt;
use ui_kit::view::{GlassGroupBuilder, GlassShape};

use super::error::WidgetError;
use super::layers::{DrawLayers, Layer};
use super::{Ctx, rect_item};
use crate::zoom::FlipMark;
use crate::zoom_config::ZoomTransitionConfig;

const NANOS_PER_MS: f64 = 1_000_000.0;

/// Допуск шума f32 при округлении границ окна наружу: край 1080,00002 px (ошибка pt → px) — это граница
/// пикселя 1080, а не повод расширять окно до 1081. Реальные дробные края (≈ 1896,6) допуск не затрагивает.
const PX_SNAP_EPSILON: f32 = 1.0e-3;

/// Что с размытием сейчас.
#[derive(Debug, Clone, Copy, PartialEq)]
enum BlurPhase {
    Off,
    /// Кадр удержан (или идёт карточка): размытие полное.
    Held,
    /// Живой кадр вернулся: размытие растворяется.
    FadingOut {
        since: MonotonicNanos,
        duration_ms: f64,
    },
}

/// Идущий оборот-карточка.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CardRun {
    started: MonotonicNanos,
}

/// Завеса перехода. Живёт во фасаде экрана (не в сцене): зависит от кадра рендера, а не от жестов.
#[derive(Debug)]
pub(crate) struct TransitionVeil {
    phase: BlurPhase,
    card: Option<CardRun>,
    /// Номер переворота, для которого карточка уже запущена.
    flip_seen: Option<u32>,
    /// Растворение после этого перехода — переворотное (короче) или обычное.
    fade_ms: f64,
}

impl TransitionVeil {
    pub(crate) fn new(config: &ZoomTransitionConfig) -> Self {
        Self {
            phase: BlurPhase::Off,
            card: None,
            flip_seen: None,
            fade_ms: config.blur_fade_out_ms,
        }
    }

    /// Продвинуть автомат на кадр: что говорит рендер, был ли переворот, который момент.
    pub(crate) fn advance(
        &mut self,
        backdrop: BackdropContinuity,
        flip: Option<FlipMark>,
        now: MonotonicNanos,
        motion: MotionPreference,
        config: &ZoomTransitionConfig,
    ) {
        if let Some(mark) = flip.filter(|mark| Some(mark.sequence) != self.flip_seen) {
            self.flip_seen = Some(mark.sequence);
            self.fade_ms = config.flip_fade_out_ms;
            self.phase = BlurPhase::Held;
            self.card = (motion != MotionPreference::Reduced).then_some(CardRun { started: mark.at });
        }
        if let Some(card) = self.card
            && elapsed_ms(card.started, now) >= config.flip_rotate_ms
        {
            self.card = None;
        }
        let must_hold = matches!(backdrop, BackdropContinuity::HeldLastFrame { .. }) || self.card.is_some();
        self.phase = match (self.phase, must_hold) {
            // Удержание перекрывает растворение: размытие снова полное.
            (_, true) => BlurPhase::Held,
            (BlurPhase::Held, false) => BlurPhase::FadingOut {
                since: now,
                duration_ms: self.fade_ms,
            },
            (BlurPhase::FadingOut { since, duration_ms }, false) if elapsed_ms(since, now) >= duration_ms => {
                self.fade_ms = config.blur_fade_out_ms;
                BlurPhase::Off
            }
            (phase, false) => phase,
        };
    }

    /// Непрозрачность размытия 0..1 на момент `now`.
    pub(crate) fn blur_opacity(&self, now: MonotonicNanos) -> f32 {
        match self.phase {
            BlurPhase::Off => 0.0,
            BlurPhase::Held => 1.0,
            BlurPhase::FadingOut { since, duration_ms } => {
                (1.0 - elapsed_ms(since, now) / duration_ms).clamp(0.0, 1.0) as f32
            }
        }
    }

    /// Доля ширины кадра, которую занимает карточка: `|cos θ|`, θ = 0…180° по smoothstep времени.
    /// `None` — карточки нет, кадр на всю ширину.
    pub(crate) fn card_width_fraction(&self, now: MonotonicNanos, rotate_ms: f64) -> Option<f64> {
        let card = self.card?;
        let progress = (elapsed_ms(card.started, now) / rotate_ms).clamp(0.0, 1.0);
        let eased = progress * progress * (3.0 - 2.0 * progress);
        Some((std::f64::consts::PI * eased).cos().abs())
    }

    /// Элементы завесы: чёрный фон под карточкой и стекло-размытие над кадром.
    pub(crate) fn draw(&self, ctx: &Ctx<'_>, layers: &mut DrawLayers) -> Result<(), WidgetError> {
        let blur = self.blur_opacity(ctx.now);
        if blur <= 0.0 {
            return Ok(());
        }
        let transition = &ctx.config.zoom.transition;
        let frame: Rect = ctx.geometry.frame(ctx.state.settings().aspect);
        let width_fraction = self.card_width_fraction(ctx.now, transition.flip_rotate_ms);
        if width_fraction.is_some() {
            // Вне карточки — чёрный: оборот идёт «в пустоте», а не поверх живого кадра.
            // Границы наружу, как у окна карточки: иначе нижняя строка кадра светила бы мимо подложки.
            layers.push(
                Layer::FrameOverlay,
                rect_item(
                    outward_pt_rect(frame, ctx.px),
                    DisplayColor::BLACK,
                    Opacity::OPAQUE,
                    ctx.px,
                ),
            );
        }
        let fraction = width_fraction.unwrap_or(1.0);
        if fraction < transition.min_card_width_fraction {
            return Ok(());
        }
        // Окно карточки: центр кадра, ширина по доле. Границы переводятся в px наружу (см. `outward_px_bounds`).
        let card = Rect::centered_at(
            frame.center(),
            Size::new(frame.size.width * fraction, frame.size.height),
        );
        let placement = outward_placement(card, ctx.px);
        let mut builder = GlassGroupBuilder::new(ctx.tokens.glass.style_for(GlassRole::Frost), ctx.px);
        builder.add_member(
            GlassShape::RoundedRect { corner_radius_pt: 0.0 },
            placement,
            PressState::RELEASED,
        )?;
        let opacity =
            Opacity::new(blur).map_err(|_| WidgetError::Inconsistent(format!("прозрачность завесы {blur}")))?;
        layers.push(Layer::FrameOverlay, DrawItem::Glass(builder.build(1.0, opacity)?));
        Ok(())
    }
}

/// Границы окна в целых px, округлённые **наружу**: левый/верхний край вниз, правый/нижний вверх.
///
/// Корень «лишней строки кадра»: низ кадра 4:3 на поверхности 1080×2412 лежит на y = 1896,6 px. Стекло
/// закрывает пиксель по расстоянию от его центра до края формы, поэтому край посреди пикселя оставлял у
/// границы строку, где кадр виден неразмытым. Край на границе пикселей даёт полное покрытие; окно
/// никогда не меньше кадра (избыток — доля пикселя за кадром, над чёрной маской). Возвращает
/// `(левый, верхний, правый, нижний)`.
pub(crate) fn outward_px_bounds(rect: Rect, px: PxPerPt) -> (f32, f32, f32, f32) {
    (
        (px.px(rect.min_x()) + PX_SNAP_EPSILON).floor(),
        (px.px(rect.min_y()) + PX_SNAP_EPSILON).floor(),
        (px.px(rect.max_x()) - PX_SNAP_EPSILON).ceil(),
        (px.px(rect.max_y()) - PX_SNAP_EPSILON).ceil(),
    )
}

/// То же окно в pt (для `rect_item`, который сам переводит pt в px): границы целых px, наружу.
fn outward_pt_rect(rect: Rect, px: PxPerPt) -> Rect {
    let (left, top, right, bottom) = outward_px_bounds(rect, px);
    let scale = f64::from(px.get());
    Rect::from_xywh(
        f64::from(left) / scale,
        f64::from(top) / scale,
        f64::from(right - left) / scale,
        f64::from(bottom - top) / scale,
    )
}

/// Размещение окна стекла по границам из [`outward_px_bounds`]: размер и центр в целых/полуцелых px.
fn outward_placement(rect: Rect, px: PxPerPt) -> NodePlacementPx {
    let (left, top, right, bottom) = outward_px_bounds(rect, px);
    NodePlacementPx {
        size: PxSize::new(right - left, bottom - top),
        transform: Affine2::translate((left + right) * 0.5, (top + bottom) * 0.5),
        opacity: 1.0,
    }
}

fn elapsed_ms(from: MonotonicNanos, to: MonotonicNanos) -> f64 {
    to.as_nanos().saturating_sub(from.as_nanos()) as f64 / NANOS_PER_MS
}
