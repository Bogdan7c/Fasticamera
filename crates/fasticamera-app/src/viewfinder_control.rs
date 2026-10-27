//! Регулировки рендера из сценариев: чем рисует видоискатель, какие стадии ISP включены, какая
//! политика линии кадра, включён ли стенд запаса под видео, какая частота экрана (сессия 07).
//!
//! Зачем отдельный модуль. Сценарии замера (`viewfinder-latency`, `viewfinder-stage-budget`,
//! `video-headroom-bench`) меняют настройки потока рендера на ходу. Продукт после сценария обязан
//! вернуться в штатный вид при ЛЮБОМ конце сценария (успех, провал, паника, вытеснение другим
//! сценарием) — ровно как камера (`CameraServices::scenario_ended`). Поэтому:
//! * сценарий меняет рендер только intent-методами `GpuServices::*_for_scenario`, и каждый метод
//!   запоминает, ЧТО именно сценарий трогал ([`RenderScenarioClaim`]);
//! * маршрутизатор в `finish` вызывает [`GpuServices::render_scenario_ended`], и тот возвращает
//!   штатное значение только тронутых настроек (значения — из конфига рендера, не догадка).
//!
//! Сценарии не знают ни `RenderInbox`, ни потока рендера: только трейт [`RenderControl`]
//! (на телефоне — ящик потока рендера, в тестах — запись запросов).

use std::sync::{Mutex, MutexGuard, PoisonError};

use isp_vulkan::viewfinder::IspStageSet;
use render_loop::config::{AdpfMode, DepthReductionMode, LateLatchMode, PresentTimelinePolicy};
use render_loop::mailbox::{InboxDelivery, RenderInbox};
use render_loop::thread::{AdpfCommand, PresentTargetCommand};
use render_loop::viewfinder::{HeadroomBench, ViewfinderCommand, ViewfinderPainter};
use serde::Serialize;
use shell_contract::commands::{FrameRate, FrameRateOutcome, InvalidFrameRate, ShellCommands};

use crate::gpu_services::GpuServices;

const LOG_TARGET: &str = "fasticamera::render::scenario_control";

/// Политика линии кадра, поздний захват и сход на ближнюю линию — одной настройкой (так их
/// принимает поток рендера).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PresentTargetSetting {
    pub policy: PresentTimelinePolicy,
    pub late_latch: LateLatchMode,
    /// Сход на ближнюю линию пропуском vsync (решение 54).
    pub depth_reduction: DepthReductionMode,
}

/// Одна регулировка рендера. Каждая описывает намерение целиком.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RenderAdjustment {
    /// Команда видоискателю (рисовальщик, стадии ISP, стенд запаса).
    Viewfinder(ViewfinderCommand),
    /// Политика линии кадра.
    PresentTarget(PresentTargetSetting),
    /// Режим подсказок производительности системе (ADPF).
    Adpf(AdpfMode),
}

/// Дошла ли регулировка до потока рендера. Итог ПРИМЕНЕНИЯ поток пишет в свой статус
/// (`viewfinder_commands`, `present_target_commands`); здесь — только доставка.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "delivery", rename_all = "snake_case")]
pub enum ControlDelivery {
    /// Лежит в ящике потока рендера.
    Delivered,
    /// Поток рендера уже завершён: регулировка потеряна.
    RenderGone,
    /// Рендер не поднимался (GPU или vsync): регулировать нечего.
    RenderUnavailable { reason: String },
}

impl ControlDelivery {
    fn of_inbox(delivery: InboxDelivery) -> Self {
        match delivery {
            InboxDelivery::Delivered => Self::Delivered,
            InboxDelivery::ReceiverGone => Self::RenderGone,
        }
    }

    /// Доставлено ли.
    pub fn is_delivered(&self) -> bool {
        matches!(self, Self::Delivered)
    }
}

/// Куда сценарии шлют регулировки (на телефоне — ящик потока рендера, в тестах — запись).
pub trait RenderControl: Send + Sync {
    fn adjust(&self, adjustment: RenderAdjustment) -> ControlDelivery;
}

/// Живой поток рендера: регулировки идут тем же ящиком, что события surface.
pub struct InboxRenderControl {
    inbox: RenderInbox,
}

impl InboxRenderControl {
    pub fn new(inbox: RenderInbox) -> Self {
        Self { inbox }
    }
}

impl RenderControl for InboxRenderControl {
    fn adjust(&self, adjustment: RenderAdjustment) -> ControlDelivery {
        let delivery = match adjustment {
            RenderAdjustment::Viewfinder(command) => self.inbox.viewfinder_command(command),
            RenderAdjustment::PresentTarget(setting) => self.inbox.present_target_command(PresentTargetCommand::Set {
                policy: setting.policy,
                late_latch: setting.late_latch,
                depth_reduction: setting.depth_reduction,
            }),
            RenderAdjustment::Adpf(mode) => self.inbox.adpf_command(AdpfCommand::Set { mode }),
        };
        ControlDelivery::of_inbox(delivery)
    }
}

/// Рендер не поднялся: каждая регулировка честно отвечает причиной.
pub struct UnavailableRenderControl {
    reason: String,
}

impl UnavailableRenderControl {
    pub fn new(reason: impl Into<String>) -> Self {
        Self { reason: reason.into() }
    }
}

impl RenderControl for UnavailableRenderControl {
    fn adjust(&self, _adjustment: RenderAdjustment) -> ControlDelivery {
        ControlDelivery::RenderUnavailable {
            reason: self.reason.clone(),
        }
    }
}

/// Какие настройки рендера тронул текущий сценарий (их и только их вернёт конец сценария).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TouchedRenderSettings {
    painter: bool,
    isp_stages: bool,
    headroom_bench: bool,
    present_target: bool,
    adpf: bool,
    display_refresh: bool,
}

impl TouchedRenderSettings {
    fn any(self) -> bool {
        self.painter
            || self.isp_stages
            || self.headroom_bench
            || self.present_target
            || self.adpf
            || self.display_refresh
    }
}

/// Учёт тронутых сценарием настроек рендера. Владелец — `GpuServices` (общий на клоны).
#[derive(Debug, Default)]
pub struct RenderScenarioClaim {
    touched: Mutex<TouchedRenderSettings>,
}

impl RenderScenarioClaim {
    fn lock(&self) -> MutexGuard<'_, TouchedRenderSettings> {
        // Флаги — простые bool: «отравленный» замок не портит их смысла.
        self.touched.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn note(&self, mark: impl FnOnce(&mut TouchedRenderSettings)) {
        mark(&mut self.lock());
    }

    /// Забирает учёт (после этого «ничего не тронуто»).
    fn take(&self) -> TouchedRenderSettings {
        std::mem::take(&mut *self.lock())
    }
}

/// Что вернула в штатный вид [`GpuServices::render_scenario_ended`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderRestoreEffects {
    /// Регулировки возврата и их доставка (только для тронутых настроек).
    pub adjustments: Vec<(&'static str, ControlDelivery)>,
    /// Частота экрана возвращена к цели рендера (`Some` — сценарий её менял).
    pub display_refresh: Option<FrameRateOutcome>,
}

impl RenderRestoreEffects {
    /// Сценарий рендер не трогал — возвращать нечего.
    pub fn is_empty(&self) -> bool {
        self.adjustments.is_empty() && self.display_refresh.is_none()
    }
}

impl GpuServices {
    /// Сценарий: рисовать видоискатель этим рисовальщиком (вернётся к рисовальщику из конфига).
    pub fn set_viewfinder_painter_for_scenario(&self, painter: ViewfinderPainter) -> ControlDelivery {
        self.scenario_claim.note(|touched| touched.painter = true);
        self.control
            .adjust(RenderAdjustment::Viewfinder(ViewfinderCommand::SetPainter(painter)))
    }

    /// Сценарий: включить ровно этот набор стадий ISP (вернётся к `IspStageSet::ALL`).
    pub fn set_isp_stages_for_scenario(&self, stages: IspStageSet) -> ControlDelivery {
        self.scenario_claim.note(|touched| touched.isp_stages = true);
        self.control
            .adjust(RenderAdjustment::Viewfinder(ViewfinderCommand::SetIspStages(stages)))
    }

    /// Сценарий: включить или выключить стенд запаса под видео (вернётся к `Disabled`).
    pub fn set_headroom_bench_for_scenario(&self, bench: HeadroomBench) -> ControlDelivery {
        self.scenario_claim.note(|touched| touched.headroom_bench = true);
        self.control
            .adjust(RenderAdjustment::Viewfinder(ViewfinderCommand::SetHeadroomBench(bench)))
    }

    /// Сценарий: политика линии кадра (вернётся к `render.present_target` из конфига).
    pub fn set_present_target_for_scenario(&self, setting: PresentTargetSetting) -> ControlDelivery {
        self.scenario_claim.note(|touched| touched.present_target = true);
        self.control.adjust(RenderAdjustment::PresentTarget(setting))
    }

    /// Сценарий: режим ADPF (вернётся к `render.adpf.mode` из конфига).
    pub fn set_adpf_for_scenario(&self, mode: AdpfMode) -> ControlDelivery {
        self.scenario_claim.note(|touched| touched.adpf = true);
        self.control.adjust(RenderAdjustment::Adpf(mode))
    }

    /// Сценарий: попросить у платформы частоту экрана `hertz` (вернётся к
    /// `render.preferred_frame_rate_hz`). Касание экрана, которое на OxygenOS снимает колпак 60 Гц,
    /// — забота скрипта smoke, не этого метода.
    pub fn request_display_refresh_for_scenario(
        &self,
        commands: &dyn ShellCommands,
        hertz: f32,
    ) -> Result<FrameRateOutcome, InvalidFrameRate> {
        let rate = FrameRate::new(hertz)?;
        self.scenario_claim.note(|touched| touched.display_refresh = true);
        Ok(commands.set_preferred_frame_rate(rate))
    }

    /// Штатная политика линии кадра — из конфига рендера.
    pub fn configured_present_target(&self) -> PresentTargetSetting {
        PresentTargetSetting {
            policy: self.render_config.present_target.policy,
            late_latch: self.render_config.present_target.late_latch,
            depth_reduction: self.render_config.present_target.depth_reduction,
        }
    }

    /// Сценарий закончился любым путём: вернуть штатные значения тронутых настроек рендера.
    /// Вызывает маршрутизатор в `finish` (до строки результата), сценарии — никогда.
    pub fn render_scenario_ended(&self, commands: &dyn ShellCommands) -> RenderRestoreEffects {
        let touched = self.scenario_claim.take();
        let mut effects = RenderRestoreEffects::default();
        if !touched.any() {
            return effects;
        }
        let mut send = |name: &'static str, adjustment: RenderAdjustment| {
            let delivery = self.control.adjust(adjustment);
            if !delivery.is_delivered() {
                tracing::warn!(target: LOG_TARGET, setting = name, ?delivery, "возврат настройки рендера не доставлен");
            }
            effects.adjustments.push((name, delivery));
        };
        if touched.headroom_bench {
            // Стенд первым: он самый тяжёлый для GPU и тепла.
            send(
                "headroom_bench",
                RenderAdjustment::Viewfinder(ViewfinderCommand::SetHeadroomBench(HeadroomBench::Disabled)),
            );
        }
        if touched.painter {
            send(
                "painter",
                RenderAdjustment::Viewfinder(ViewfinderCommand::SetPainter(self.render_config.viewfinder.painter)),
            );
        }
        if touched.isp_stages {
            send(
                "isp_stages",
                RenderAdjustment::Viewfinder(ViewfinderCommand::SetIspStages(IspStageSet::ALL)),
            );
        }
        if touched.present_target {
            send(
                "present_target",
                RenderAdjustment::PresentTarget(self.configured_present_target()),
            );
        }
        if touched.adpf {
            send("adpf", RenderAdjustment::Adpf(self.render_config.adpf.mode));
        }
        if touched.display_refresh {
            effects.display_refresh = match FrameRate::new(self.render_config.preferred_frame_rate_hz) {
                Ok(rate) => Some(commands.set_preferred_frame_rate(rate)),
                Err(error) => {
                    // Конфиг рендера проверен при старте; сюда не попадаем, но молча не глотаем.
                    tracing::error!(target: LOG_TARGET, %error, "частота экрана из конфига неверна: не возвращена");
                    None
                }
            };
        }
        effects
    }
}

#[cfg(test)]
#[path = "viewfinder_control_tests.rs"]
mod tests;
