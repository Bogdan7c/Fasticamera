//! Стенд «запас под видео» (V4, сессия 07) внутри видоискателя: управление и диагностика.
//!
//! Это НЕ продуктовая функция и по умолчанию выключено. Включённый стенд на каждом НОВОМ кадре
//! камеры записывает в тот же сабмит, что и ISP видоискателя, качественный полнокадровый конвейер
//! `isp_vulkan::quality::VideoHeadroomWorkload` (Menon 2007 на полном разрешении + временной
//! шумодав + выход в приёмник кадра) над ТЕМ ЖЕ импортированным RAW. Пользователь по-прежнему видит
//! видоискатель; стенд нужен, чтобы измерить на телефоне, сколько GPU остаётся под видео-ISP
//! (цифра для плана `video`).
//!
//! Стенд работает только с рисовальщиком ISP (`ViewfinderPainter::Isp`): отладочная мозаика держит
//! кадр для фрагментного шейдера до замены, и полнокадровый compute над ним не записывается.
//! Объекты стенда (~360 МиБ на 4K) создаются лениво на первом новом кадре после включения — этот
//! кадр дольше обычного (создание пайплайнов и буферов на потоке рендера); это цена стенда, а не
//! продукта.
//!
//! Здесь только нейтральные типы: команда ([`HeadroomBench`] в `ViewfinderCommand::SetHeadroomBench`)
//! и снимок диагностики ([`HeadroomBenchDiagnostics`] в `ViewfinderDiagnostics::headroom`).
//! Работает стенд в `vulkan_backend::camera` (`headroom_bench`, `headroom_stats`).

use serde::{Deserialize, Serialize};

use super::diagnostics::ViewfinderExtent;
use crate::thread::DurationSummary;

/// Включён ли стенд V4. Команда `ViewfinderCommand::SetHeadroomBench`; на старте всегда `Disabled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadroomBench {
    /// Стенд выключен: объектов GPU нет (освобождаются после fence слотов, где они использовались).
    #[default]
    Disabled,
    /// Каждый новый кадр камеры дополнительно проходит качественный конвейер полного кадра.
    Enabled,
}

/// Окно времени GPU одного прохода стенда.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadroomPassTime {
    /// Имя прохода (`isp_vulkan::quality::HeadroomPass::label`): `levels`, проходы Menon,
    /// `temporal_denoise`, `output`.
    pub pass: &'static str,
    /// p50/p95/max по окну `viewfinder.gpu_time_window_frames`.
    pub gpu_time: DurationSummary,
}

/// Диагностика стенда V4.
///
/// Счётчики накопительные за жизнь бэкенда (переживают пересоздание surface и выключение стенда),
/// окна времени — скользящие (`viewfinder.gpu_time_window_frames`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeadroomBenchDiagnostics {
    /// Включён ли стенд.
    pub mode: HeadroomBench,
    /// Размер живой нагрузки (= полезная область кадра); `None` — объекты стенда не созданы.
    pub workload_extent: Option<ViewfinderExtent>,
    /// Сколько выключенных или заменённых нагрузок ждут fence своих последних сабмитов, чтобы
    /// освободиться (каждая — ~10 буферов по 36 МиБ на 4K).
    pub workloads_awaiting_release: usize,
    /// Сколько нагрузок создано (включение, смена размера кадра, пересоздание после сбоя).
    pub workloads_created: u64,
    /// Сколько нагрузок освобождено после fence.
    pub workloads_released: u64,
    /// Сколько новых кадров камеры записано через стенд (в сабмиты, принятые очередью).
    pub frames_recorded: u64,
    /// Сколько кадров пропущено, потому что приёмник занят (обратное давление).
    pub sink_busy_skips: u64,
    /// Сколько новых кадров не записано, потому что параметров качественного ISP нет.
    pub frames_skipped_no_params: u64,
    /// Сколько раз сменился поток камеры (новая сессия): история шумодава начата заново.
    pub stream_changes: u64,
    /// Сколько кадров начали историю шумодава заново (первый кадр нагрузки, смена потока).
    pub history_restarts: u64,
    /// Сколько раз стенд отказал (создание, параметры, запись). После отказа стенд стоит до
    /// повторного включения командой (выключить → включить): отказ — сигнал оператору, а не повод
    /// пересоздавать ~360 МиБ на каждом кадре.
    pub failures: u64,
    /// Причина последнего отказа.
    pub last_failure: Option<String>,
    /// Сколько раз не удалось собрать метки времени стенда.
    pub timing_failures: u64,
    /// Время GPU по проходам в порядке исполнения (проходы без замеров не перечисляются).
    pub pass_times: Vec<HeadroomPassTime>,
    /// Весь кадр стенда: от начала до конца выхода.
    pub total: Option<DurationSummary>,
}
