//! Нейтральные типы нашего AE на потоке рендера (сессия 08a): связи с движком камеры и диагностика.
//!
//! Здесь нет Vulkan: это словарь между потоком рендера и композицией. Сам исполнитель живёт в
//! `vulkan_backend::camera::exposure`.

use camera_contract::auto_exposure::{
    ExposureAuthority, ExposureDirective, ExposureProposal, ExposureProposalChannel, MeasurementQuality, MeteredArea,
    ProposalConvergence,
};
use camera_contract::three_a::DirectiveSlot;
use serde::Serialize;

/// Два ящика между потоком рендера и движком камеры: что композиция передаёт бэкенду, чтобы включить
/// наш AE (`VulkanDisplayBackend::with_auto_exposure`). Без них AE на потоке рендера не работает:
/// подтяжка ISP равна 1, предложений нет (вендорский AE в композиции).
#[derive(Debug, Clone)]
pub struct AutoExposureLinks {
    /// Указания движка (читающая половина: писатель — у движка камеры, как у AWB).
    pub directives: DirectiveSlot<ExposureDirective>,
    /// Предложения рендера движку (писатель — поток рендера; читатель и «толчок» — у движка).
    pub proposals: ExposureProposalChannel,
}

/// Счётчики решений AE (накопительные за сессию показа камеры).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct AutoExposureCounters {
    /// Замеров принято контроллером.
    pub accepted: u64,
    /// Замеров без результата (ошибка исполнителя замера).
    pub failed: u64,
    /// Замеров чужой области отброшено.
    pub rejected_foreign_area: u64,
    /// Устаревших замеров отброшено.
    pub rejected_stale: u64,
    /// Замеров проигнорировано ручной экспозицией.
    pub ignored_manual: u64,
    /// Замеров проигнорировано блокировкой.
    pub ignored_holding: u64,
    /// Кадров пропущено: у кадра нет спаренной выдержки и ISO.
    pub skipped_no_paired_exposure: u64,
    /// Кадров пропущено: исполнитель замера занят.
    pub skipped_meter_busy: u64,
    /// Кадров пропущено: область замера не получилось построить (регион тапа вне сетки статистики
    /// или положение сетки не вычисляется по паспорту).
    pub skipped_area_unavailable: u64,
    /// Статистик отброшено: кадр чужого потока (камера сменилась, пока слот был в полёте).
    pub statistics_stale: u64,
    /// Предложений опубликовано движку.
    pub proposals_published: u64,
    /// Кадров, на которых AE потребовал статистику «на каждом кадре» (холодный старт замера).
    pub cold_start_statistics_frames: u64,
    /// Кадров, показанных с остаточной подтяжкой (множитель не 1).
    pub frames_with_residual_gain: u64,
}

/// Диагностика нашего AE на потоке рендера. `None` в `ViewfinderDiagnostics`, пока AE не включён.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AutoExposureDiagnostics {
    /// Поколение применённого указания (`None` — движок ещё ничего не публиковал).
    pub directive_generation: Option<u64>,
    /// Кто ведёт экспозицию по действующему указанию.
    pub authority: ExposureAuthority,
    /// Область, по которой идёт замер.
    pub metered_area: MeteredArea,
    /// Сглаженная цель замера, log2 от «выдержка × ISO» в с·ISO (`None` — оценки сцены ещё нет).
    pub target_log2_seconds_iso: Option<f64>,
    /// Остаточная подтяжка ISP на последнем показанном кадре, EV (0 — без подтяжки).
    pub display_gain_ev: f32,
    /// Сходимость последнего опубликованного предложения (`None` — предложений ещё не было).
    pub convergence: Option<ProposalConvergence>,
    /// Надёжность замера последнего опубликованного предложения.
    pub quality: Option<MeasurementQuality>,
    /// Последнее опубликованное предложение целиком.
    pub last_proposal: Option<ExposureProposal>,
    /// Счётчики решений.
    pub counters: AutoExposureCounters,
}
