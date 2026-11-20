//! Колесо зума (K3 `SnapScale` с логарифмической моделью): дуговая шкала над нижним рядом, открывается
//! долгим тапом или перетаскиванием ленты и уходит через ≈ 2,15 с покоя (`behavior.md` 2.3, `motion.md`).
//!
//! # Кто владелец значения
//! Значение зума принадлежит `ScreenState` ([`crate::zoom::ZoomState`]); шкала отвечает только за физику
//! (ведение за пальцем, бросок, магнит упора, хаптика рисок). Как у EV (`exposure_link.rs`):
//! * шкала → экран: `ValueChanged` становится `ZoomWheelDragged` **только пока значение двигает палец**
//!   (включая бросок после отпускания); программные изменения эхом не уходят;
//! * экран → шкала: если значение состояния разошлось со шкалой (тап по кнопке, щипок, переворот), шкала
//!   встаёт на него сразу и без хаптики.
//!
//! # Хаптика
//! Тик на каждой риске и щелчок на упорах приходят событиями шкалы; щипок даёт щелчок на упорах каталога.
//! Оба потока идут через один ограничитель частоты `ui_kit::haptic_throttle` (мотор один): не чаще одного
//! паттерна за интервал токена (`[estimate]` 30 мс, решение 112).
//!
//! # Появление и уход
//! Появление: 0,2 с, подъём на 10,7 pt с перелётом 1,7 pt; уход: 0,11 с, вниз на 33 pt с растворением
//! (`[measured: R00 t=88.5–100]`). Reduce Motion: без перемещений, только растворение.

use camera_contract::zoom::ZoomRatio;
use ui_core::animator::AnimatedProperty;
use ui_core::geometry::Rect;
use ui_core::gesture::{GestureEvent, GestureState, PanDirection, PanSpec, PressSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_core::time::{MonotonicNanos, MotionPreference};
use ui_draw::transform::Affine2;
use ui_kit::drive::{GrowthDirection, ScrollTransition};
use ui_kit::haptic_throttle::{HapticThrottle, ThrottleVerdict};
use ui_kit::pan::PanUpdate;
use ui_kit::scale::{ScaleEvent, ScaleSpec, SnapPolicy, SnapScale, TickVisibility};
use ui_kit::tokens::{HapticEvent, MotionToken, ScaleDensity, TextRoleKind};
use ui_kit::value::{Logarithmic, ValueModel};
use ui_kit::view::{ArcLabel, arc_scale_items, centered_text, opacity_of_placement};

use super::error::{WidgetError, element};
use super::intent::ScreenGesture;
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::text_runs::TextRuns;
use super::zoom_wheel_model::{WheelSignature, build_model, is_bright};
use super::{Ctx, Outputs};
use crate::action::ScreenAction;
use crate::zoom_labels::{focal_label, selected_label, unselected_label};

/// Допуск расхождения шкалы и состояния, октавы: меньше этого — одно и то же значение.
const SYNC_TOLERANCE_OCTAVES: f64 = 1.0e-4;

/// Ограничитель вибраций колеса и щипка.
struct ZoomHaptics {
    throttle: HapticThrottle,
    /// Коэффициент прошлого кадра во время щипка (для поиска пройденного упора).
    last_pinch_ratio: Option<f32>,
}

impl ZoomHaptics {
    fn play(&mut self, pattern: shell_contract::haptics::HapticPattern, now: MonotonicNanos, out: &mut Outputs) {
        if self.throttle.admit(now) == ThrottleVerdict::Play {
            out.haptics.push(pattern);
        }
    }
}

pub(crate) struct ZoomWheelWidget {
    zone: NodeId,
    zone_rect: Rect,
    scale: Option<(WheelSignature, SnapScale<Logarithmic>)>,
    /// Значение сейчас двигает палец (включая бросок после отпускания).
    finger_driven: bool,
    /// Идёт пан или долгий тап, ведущий колесо.
    drag_active: bool,
    /// Погашено эхо программных установок (диагностика и тесты).
    suppressed_echo: u64,
    haptics: ZoomHaptics,
    /// Состояние открытости, под которое уже настроены узлы.
    open_applied: bool,
}

impl ZoomWheelWidget {
    pub(crate) fn build(kit: &mut SceneKit, ctx: &Ctx<'_>, parent: NodeId) -> Result<Self, WidgetError> {
        let zone_rect = ctx.geometry.zoom_wheel_zone();
        let zone = kit.add_node(parent, "zoom_wheel", zone_rect, ctx.geometry.screen())?;
        kit.add_recognizer(
            zone,
            RecognizerSpec::Pan(PanSpec {
                direction: PanDirection::Horizontal,
                ..PanSpec::default()
            }),
            ScreenGesture::ZoomWheelPan,
        )?;
        kit.add_recognizer(
            zone,
            RecognizerSpec::Press(PressSpec::default()),
            ScreenGesture::ZoomWheelPress,
        )?;
        kit.set_shown(zone, false)?;
        let throttle = HapticThrottle::new(&ctx.tokens.haptic_throttle).map_err(element("ограничитель хаптики"))?;
        Ok(Self {
            zone,
            zone_rect,
            scale: None,
            finger_driven: false,
            drag_active: false,
            suppressed_echo: 0,
            haptics: ZoomHaptics {
                throttle,
                last_pinch_ratio: None,
            },
            open_applied: false,
        })
    }

    pub(crate) fn suppressed_echo_events(&self) -> u64 {
        self.suppressed_echo
    }

    // ---- жесты ----

    pub(crate) fn handle_gesture(
        &mut self,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        let update = match event.intent {
            ScreenGesture::ZoomWheelPan | ScreenGesture::ZoomStripPan => {
                PanUpdate::from_gesture(event.state, event.data).map_err(element("пан колеса зума"))?
            }
            ScreenGesture::ZoomStripLongPress => {
                PanUpdate::from_long_press(event.state, event.data).map_err(element("долгий тап ленты зума"))?
            }
            ScreenGesture::ZoomWheelPress => return self.handle_press(event, out),
            _ => return Ok(false),
        };
        let Some(update) = update else {
            return Ok(true);
        };
        match update {
            PanUpdate::Began { .. } => {
                self.drag_active = true;
                self.finger_driven = true;
                out.actions.push(ScreenAction::ZoomWheelTouched { at: event.time });
            }
            PanUpdate::Ended { .. } | PanUpdate::Cancelled => {
                self.drag_active = false;
                out.actions.push(ScreenAction::ZoomWheelReleased { at: event.time });
            }
            PanUpdate::Changed { .. } => {}
        }
        if let Some((_, scale)) = &mut self.scale {
            scale.apply_pan(event.time, update).map_err(element("колесо зума"))?;
        }
        Ok(true)
    }

    /// Нажатие на открытое колесо: палец лёг (колесо не уходит, бросок гаснет) или поднят без перетаскивания.
    fn handle_press(&mut self, event: &GestureEvent<ScreenGesture>, out: &mut Outputs) -> Result<bool, WidgetError> {
        match event.state {
            GestureState::Began => {
                out.actions.push(ScreenAction::ZoomWheelTouched { at: event.time });
                if let Some((_, scale)) = &mut self.scale {
                    scale.catch(event.time);
                }
                self.finger_driven = true;
            }
            GestureState::Ended | GestureState::Cancelled | GestureState::Failed if !self.drag_active => {
                out.actions.push(ScreenAction::ZoomWheelReleased { at: event.time });
                if let Some((_, scale)) = &mut self.scale {
                    scale.release_without_drag(event.time).map_err(element("колесо зума"))?;
                }
            }
            _ => {}
        }
        Ok(true)
    }

    // ---- состояние → узлы и шкала ----

    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        self.sync_scale(ctx)?;
        let open = ctx.state.zoom().is_wheel_open();
        if open != self.open_applied {
            self.open_applied = open;
            if open {
                self.animate_open(kit, ctx)?;
            } else {
                self.animate_close(kit, ctx)?;
            }
        }
        Ok(())
    }

    /// Шкала по группе направления: пересборка при смене отпечатка, встать на значение состояния при расхождении.
    fn sync_scale(&mut self, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let Some(group) = ctx.state.zoom_group() else {
            self.scale = None;
            return Ok(());
        };
        let signature = WheelSignature::of(group);
        let state_value = f64::from(ctx.state.zoom().ratio().value());
        let needs_rebuild = self.scale.as_ref().is_none_or(|(built, _)| *built != signature);
        if needs_rebuild {
            let layout = &ctx.geometry.zoom_config().wheel;
            let spec = ScaleSpec {
                direction: GrowthDirection::FingerLeft,
                snap: SnapPolicy::ContinuousWithMagnets,
                density: ScaleDensity::Dial,
                ticks: TickVisibility::Visible,
                ..ScaleSpec::default()
            };
            let scale = SnapScale::new(
                build_model(layout, group)?,
                state_value,
                spec,
                &ctx.environment(self.zone_rect.size.width),
                ctx.now,
            )
            .map_err(element("колесо зума"))?;
            self.scale = Some((signature, scale));
            self.finger_driven = false;
            return Ok(());
        }
        if let Some((_, scale)) = &mut self.scale
            && !self.finger_driven
        {
            let current = scale.value(ctx.now);
            if (current.log2() - state_value.log2()).abs() > SYNC_TOLERANCE_OCTAVES {
                // Программная установка: без рубежей по пути и без хаптики; события значения погасит `translate`.
                scale
                    .set_value(state_value, ScrollTransition::Immediate, ctx.now)
                    .map_err(element("колесо зума"))?;
            }
        }
        Ok(())
    }

    fn animate_open(&self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let wheel = &ctx.geometry.zoom_config().wheel;
        kit.set_shown(self.zone, true)?;
        kit.scene_mut()
            .set_property(self.zone, AnimatedProperty::Opacity, 0.0)?;
        kit.fade_to(self.zone, 1.0, ctx.spring(MotionToken::WheelAppear)?, ctx.now)?;
        if ctx.motion == MotionPreference::Reduced {
            return kit.jump_vertically_to(self.zone, 0.0);
        }
        // Колесо стартует ниже на высоту подъёма и поднимается на место; перелёт 1,7 pt даёт пружина токена.
        kit.jump_vertically_to(self.zone, wheel.appear_rise_pt)?;
        kit.slide_vertically_to(self.zone, 0.0, ctx.spring(MotionToken::WheelAppear)?, ctx.now)
    }

    fn animate_close(&self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let wheel = &ctx.geometry.zoom_config().wheel;
        // Не ловит касаний сразу, растворяется и опускается.
        kit.set_interactive(self.zone, false)?;
        kit.fade_to(self.zone, 0.0, ctx.spring(MotionToken::WheelDismiss)?, ctx.now)?;
        if ctx.motion != MotionPreference::Reduced {
            kit.slide_vertically_to(
                self.zone,
                wheel.dismiss_drop_pt,
                ctx.spring(MotionToken::WheelDismiss)?,
                ctx.now,
            )?;
        }
        Ok(())
    }

    // ---- кадр ----

    /// События шкалы → действия экрана и хаптика; хаптика щипка по пройденным упорам.
    pub(crate) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        let events = match &mut self.scale {
            Some((_, scale)) => {
                scale.advance(ctx.now);
                scale.drain_events()
            }
            None => Vec::new(),
        };
        for event in events {
            match event {
                ScaleEvent::ValueChanged { value } if self.finger_driven => {
                    if let Ok(requested) = ZoomRatio::new(value as f32) {
                        out.actions
                            .push(ScreenAction::ZoomWheelDragged { requested, at: ctx.now });
                    }
                }
                ScaleEvent::ValueChanged { .. } => self.suppressed_echo += 1,
                ScaleEvent::Haptic { pattern } => self.haptics.play(pattern, ctx.now, out),
                ScaleEvent::Settled { .. } => self.finger_driven = false,
                ScaleEvent::TickCrossed { .. } | ScaleEvent::ReferenceCrossed { .. } => {}
            }
        }
        self.pinch_haptics(ctx, out);
    }

    /// Щипок: щелчок на каждом упоре каталога, который значение прошло с прошлого кадра.
    fn pinch_haptics(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        if !ctx.config.zoom.pinch.haptics_at_stops || !ctx.state.zoom().is_pinching() {
            self.haptics.last_pinch_ratio = None;
            return;
        }
        let now_ratio = ctx.state.zoom().ratio().value();
        let previous = self.haptics.last_pinch_ratio.replace(now_ratio);
        let (Some(previous), Some(group)) = (previous, ctx.state.zoom_group()) else {
            return;
        };
        let crossed = group.stops.iter().any(|stop| {
            let stop_ratio = stop.ratio.value();
            (previous < stop_ratio) != (now_ratio < stop_ratio)
        });
        if crossed && let Some(pattern) = ctx.tokens.haptics.pattern_for(HapticEvent::ScaleReference) {
            self.haptics.play(pattern, ctx.now, out);
        }
    }

    // ---- рисование ----

    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let (Some(placement), Some((_, scale)), Some(group)) = (
            visuals.placement(self.zone),
            self.scale.as_ref(),
            ctx.state.zoom_group(),
        ) else {
            return Ok(());
        };
        if placement.opacity <= 0.0 {
            return Ok(());
        }
        let wheel = &ctx.geometry.zoom_config().wheel;
        let colors = &ctx.tokens.colors;
        let zone_center = self.zone_rect.center();
        let disc_center = ctx.geometry.zoom_wheel_center();
        let center_offset = (disc_center.x - zone_center.x, disc_center.y - zone_center.y);

        // Подписи у рисок: упоры каталога и круглые значения из конфига (кроме тех, что вплотную к упору).
        let points_per_unit = scale.points_per_unit();
        let offset_of = |value: f64| -> Result<f64, WidgetError> {
            Ok(scale
                .model()
                .position_of(value)
                .map_err(element("модель колеса зума"))?
                * points_per_unit)
        };
        let mut labels = Vec::new();
        let stop_values: Vec<f64> = group.stops.iter().map(|stop| f64::from(stop.ratio.value())).collect();
        let configured = wheel.number_label_values.iter().copied().filter(|value| {
            let (low, high) = (f64::from(group.range.min.value()), f64::from(group.range.max.value()));
            (low..=high).contains(value)
                && stop_values
                    .iter()
                    .all(|stop| (value.log2() - stop.log2()).abs() > wheel.tick_merge_octaves)
        });
        for value in stop_values.iter().copied().chain(configured) {
            let ratio = ZoomRatio::new(value as f32).map_err(element("подпись колеса"))?;
            labels.push(ArcLabel {
                offset_points: offset_of(value)?,
                run: texts
                    .run(ctx, &unselected_label(ratio), TextRoleKind::ZoomLabel)?
                    .clone(),
                color: colors.text_primary,
                radius_pt: wheel.number_label_radius_pt,
            });
        }
        for stop in group
            .stops
            .iter()
            .filter(|stop| matches!(stop.kind, camera_contract::zoom::ZoomStopKind::Lens(_)))
        {
            if let Some(text) = focal_label(group, stop.ratio) {
                labels.push(ArcLabel {
                    offset_points: offset_of(f64::from(stop.ratio.value()))?,
                    run: texts.run(ctx, &text, TextRoleKind::ZoomFocalLabel)?.clone(),
                    color: colors.text_muted,
                    radius_pt: wheel.focal_label_radius_pt,
                });
            }
        }
        let drawing = arc_scale_items(
            &placement,
            center_offset,
            scale,
            ctx.now,
            ctx.px,
            &ctx.tokens.arc_scale,
            colors,
            &labels,
            |value| is_bright(wheel, value),
        )?;
        layers.extend(Layer::Plain, drawing.backing);
        layers.extend(Layer::Content, drawing.foreground);

        // Показание: «1.7×» жёлтым и фокусное под ним — на месте выбранной кнопки ленты, которую колесо заменяет.
        let opacity = opacity_of_placement(&placement, "zoom_wheel")?;
        let ratio = ctx.state.zoom().ratio();
        let readout_host = |center_y_pt: f64| {
            Affine2::translate(
                ctx.px.px(disc_center.x - zone_center.x),
                ctx.px.px(center_y_pt + ctx.geometry.bottom_shift_pt() - zone_center.y),
            )
            .then(placement.transform)
        };
        let value_run = texts.run(ctx, &selected_label(ratio), TextRoleKind::ZoomLabel)?;
        layers.push(
            Layer::Content,
            centered_text(
                value_run,
                readout_host(wheel.readout_value_center_y_pt),
                colors.zoom_selected,
                opacity,
            ),
        );
        if let Some(text) = focal_label(group, ratio) {
            let focal_run = texts.run(ctx, &text, TextRoleKind::ZoomFocalLabel)?;
            layers.push(
                Layer::Content,
                centered_text(
                    focal_run,
                    readout_host(wheel.readout_focal_center_y_pt),
                    colors.text_primary,
                    opacity,
                ),
            );
        }
        Ok(())
    }
}
