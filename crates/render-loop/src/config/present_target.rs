//! Настройки выбора линии кадра и позднего захвата кадра камеры (сессия 07).
//!
//! Зачем: задержка «сенсор → экран» видоискателя складывается из ожидания кадра камеры,
//! работы CPU/GPU и того, НА КАКОЙ vsync мы метим. Choreographer предлагает несколько линий
//! кадра; предпочитаемая платформой — не обязательно самая ранняя, к которой мы успеваем.
//! Политика [`PresentTimelinePolicy::NearestFeasible`] выбирает самую раннюю выполнимую
//! линию, а «поздний захват» (`latch_at`) говорит бэкенду, как долго можно ждать свежий кадр
//! камеры, не опоздав к сроку выбранной линии.
//!
//! Значения по умолчанию — настройки продукта после P14 сессии 07 (решения 53, 54 и ПК-стенд
//! `tests/latency_profiles_bench.rs`): `NearestFeasible` с запасом 1,5 мс, поздний захват перед
//! опросом камеры и сход на ближнюю линию пропуском vsync. Прежнее поведение (`Preferred`, без
//! захвата) — профиль `baseline` A/B на телефоне. Сама политика-перечисление по умолчанию
//! остаётся `Preferred` (так читается JSON без `margin_us`), продуктовый выбор — в
//! `PresentTargetConfig::default`.

use serde::{Deserialize, Serialize};

/// Наибольшее допустимое значение любого поля-длительности здесь, мкс (1 с). Всё, что больше,
/// — заведомая опечатка: период кадра 4–17 мс, и секундный запас означал бы «никогда не успеть».
pub const MAX_PRESENT_TARGET_DURATION_US: u64 = 1_000_000;
/// Наибольшее окно статистики, кадров (~28 минут при 60 Гц): дальше окно не «скользящее».
pub const MAX_PRESENT_TARGET_WINDOW_FRAMES: usize = 100_000;

/// Какую линию кадра выбирать.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PresentTimelinePolicy {
    /// Предпочитаемая платформой линия (как до сессии 07). Значение перечисления по умолчанию;
    /// продукт по умолчанию — `NearestFeasible` (см. модуль).
    #[default]
    Preferred,
    /// Самая ранняя линия, у которой `deadline − сейчас ≥ оценка стоимости кадра + margin`.
    /// Ни одна не подходит — предпочитаемая (со счётчиком отката).
    NearestFeasible {
        /// Запас сверх оценки стоимости кадра (CPU + GPU), мкс. Покрывает выбросы сверх p95
        /// и то, чего оценка не видит (композиция SurfaceFlinger, дрожание пробуждения).
        margin_us: u64,
    },
}

/// Ждать ли момента позднего захвата (`latch_at`) перед опросом камеры.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LateLatchMode {
    /// Камера опрашивается сразу после получения картинки swapchain (как до сессии 07). По умолчанию.
    #[default]
    Disabled,
    /// Перед опросом камеры поток рендера ждёт до `latch_at` (сон + активное ожидание):
    /// на экран попадает кадр камеры, пришедший как можно ближе к сроку линии.
    BeforeCameraPoll,
}

/// Запас `NearestFeasible` по умолчанию, мкс: 1,5 мс (~9 % периода 60 Гц) сверх p95 CPU + GPU —
/// значение профиля `tuned` ПК-стенда и A/B на телефоне.
pub const DEFAULT_NEAREST_FEASIBLE_MARGIN_US: u64 = 1_500;

/// Наибольшая длина серии «ближняя линия выполнима» перед сходом, кадров (~17 с при 60 Гц).
pub const MAX_DEPTH_REDUCTION_STREAK_FRAMES: u32 = 1_000;
/// Наибольшая пауза между сходами, мс (10 минут): больше — по сути «выключено», для этого есть режим.
pub const MAX_DEPTH_REDUCTION_INTERVAL_MS: u64 = 600_000;

/// Сходить ли на ближнюю линию кадра пропуском одного vsync (решение 54).
///
/// Зачем: `NearestFeasible` не даёт двум кадрам подряд метить на один vsync («строго позже линии
/// прошлого кадра»). Выбрав на старте дальнюю линию (консервативная оценка стоимости), планировщик
/// без схода оставался бы на ней навсегда: каждый следующий кадр снова метит не раньше. Сход —
/// один тик без рисования и показа: линия прошлого кадра остаётся прежней, и следующий тик уже
/// может метить на vsync ближе. Цена — один повтор кадра на экране (и, при буфере видоискателя,
/// один отпущенный кадр камеры: очередь стала на кадр короче); выигрыш — период задержки навсегда.
/// Действует только при `NearestFeasible`: `Preferred` всегда метит на линию платформы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepthReductionMode {
    /// Глубина очереди только растёт или стоит (поведение до решения 54).
    Disabled,
    /// Ближняя линия выполнима `depth_reduction_streak_frames` кадров подряд — один тик пропускается,
    /// дальше кадры метят ближе. По умолчанию (решение 54).
    #[default]
    SkipOneVsync,
}

/// Параметры выбора линии кадра и позднего захвата. JSON-ключ — `present_target` в
/// конфиге потока рендера; отсутствующие поля — из [`Default`], неизвестные — ошибка.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PresentTargetConfig {
    /// Политика выбора линии. По умолчанию `NearestFeasible { margin_us: 1500 }` — см. модуль.
    pub policy: PresentTimelinePolicy,
    /// Ждать ли `latch_at` перед опросом камеры. По умолчанию `BeforeCameraPoll` (см. модуль).
    pub late_latch: LateLatchMode,
    /// Запас для момента позднего захвата при политике `Preferred`, мкс:
    /// `latch_at = deadline − оценка стоимости − запас`. 1000 (1 мс) — около 6 % периода 60 Гц.
    /// При `NearestFeasible` используется `margin_us` политики, чтобы выбор линии и момент
    /// захвата опирались на один и тот же запас.
    pub latch_margin_us: u64,
    /// Оценка стоимости кадра (CPU от захвата кадра камеры до сабмита), пока замеров меньше
    /// `min_cost_samples`, мкс. 6000 — заведомо больше ожидаемых 1–3 мс: на старте лучше
    /// метить на линию позже, чем опоздать.
    pub initial_frame_cost_us: u64,
    /// Консервативная оценка времени кадра на GPU, мкс, пока замеров GPU меньше
    /// `min_cost_samples` (или бэкенд GPU не меряет: тестовый паттерн, fake). Входит в стоимость
    /// кадра вместе с CPU: кадр должен доисполниться на GPU к сроку линии. 13 000 — то, сколько на
    /// OnePlus 11 стоит даже тривиальный кадр под DVFS Adreno (сессия 07: мозаика 05 ≈ 13 мс).
    /// Ноль — слагаемого GPU до замеров нет (поведение до P13; GPU покрывает только запас).
    pub gpu_cost_fallback_us: u64,
    /// Окно скользящей оценки стоимости кадра (p95), кадров. 120 — 2 с при 60 Гц: оценка
    /// успевает за сменой нагрузки (тепло, фон), но один выброс не двигает p95.
    pub cost_window_frames: usize,
    /// Сколько замеров нужно, чтобы доверять p95 вместо `initial_frame_cost_us` (и p95 GPU
    /// вместо `gpu_cost_fallback_us` — порог общий). 30 — при
    /// меньшем числе p95 по сути равен максимуму случайной горстки. Не больше окна.
    pub min_cost_samples: usize,
    /// Окно диагностики (время в `acquire_frame`, стоимость кадра p50/p95/max), кадров.
    /// 600 — 10 с при 60 Гц, как окно пейсинга.
    pub stats_window_frames: usize,
    /// Последние сколько мкс ожидания до момента захвата крутить активно, а не спать. Сон ОС
    /// просыпается с опозданием на десятки–сотни мкс (дрожание планировщика), активное ожидание
    /// точно, но жжёт CPU. 500 мкс — компромисс: ~3 % ядра при 60 Гц ради точности захвата.
    pub latch_spin_us: u64,
    /// Дольше этого ждать момент захвата нельзя ни при каких данных, мкс: защита от абсурдного
    /// срока (сбой часов, ошибка платформы). 50 000 — три периода 60 Гц. Действующий предел
    /// кадра ещё и не больше «период vsync тика − `latch_spin_us`» (планировщик урезает его сам,
    /// счётчик `latch_wait_capped_to_period`): ожидание идёт с картинкой swapchain на руках, и
    /// дольше периода оно стоило бы пропущенного vsync.
    pub latch_max_wait_us: u64,
    /// Сход на ближнюю линию пропуском vsync (решение 54). Только при `NearestFeasible`.
    pub depth_reduction: DepthReductionMode,
    /// Сколько кадров подряд ближняя линия должна быть выполнима (её не дал взять только запрет
    /// «строго позже прошлой линии»), чтобы сойти на неё. 30 — полсекунды при 60 Гц: одиночный
    /// удачный кадр не повод платить повтором, а за полсекунды оценка стоимости видит и выбросы GPU.
    pub depth_reduction_streak_frames: u32,
    /// Не чаще одного схода за столько миллисекунд. 1000: каждый сход стоит повтора кадра на экране;
    /// дрожание оценки стоимости не должно превращать сходы в регулярное подёргивание.
    pub depth_reduction_min_interval_ms: u64,
}

impl Default for PresentTargetConfig {
    fn default() -> Self {
        Self {
            policy: PresentTimelinePolicy::NearestFeasible {
                margin_us: DEFAULT_NEAREST_FEASIBLE_MARGIN_US,
            },
            late_latch: LateLatchMode::BeforeCameraPoll,
            latch_margin_us: 1_000,
            initial_frame_cost_us: 6_000,
            gpu_cost_fallback_us: 13_000,
            cost_window_frames: 120,
            min_cost_samples: 30,
            stats_window_frames: 600,
            latch_spin_us: 500,
            latch_max_wait_us: 50_000,
            depth_reduction: DepthReductionMode::default(),
            depth_reduction_streak_frames: 30,
            depth_reduction_min_interval_ms: 1_000,
        }
    }
}

/// Что не так с [`PresentTargetConfig`]. Поле называется по имени, чтобы ошибка в JSON
/// находилась сразу.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PresentTargetConfigError {
    #[error("present_target.{field} должен лежать в [0, {MAX_PRESENT_TARGET_DURATION_US}] мкс, а не {value_us}")]
    DurationOutOfRange { field: &'static str, value_us: u64 },
    #[error("present_target.{field} должен лежать в [1, {MAX_PRESENT_TARGET_WINDOW_FRAMES}] кадров, а не {value}")]
    WindowOutOfRange { field: &'static str, value: usize },
    #[error(
        "present_target.min_cost_samples ({min_cost_samples}) должен лежать в [1, cost_window_frames ({cost_window_frames})]"
    )]
    MinSamplesOutsideWindow {
        min_cost_samples: usize,
        cost_window_frames: usize,
    },
    #[error("present_target.latch_max_wait_us должен быть больше нуля: иначе захват никогда не ждёт")]
    ZeroLatchMaxWait,
    #[error(
        "present_target.depth_reduction_streak_frames должен лежать в [1, {MAX_DEPTH_REDUCTION_STREAK_FRAMES}], а не {value}"
    )]
    DepthReductionStreakOutOfRange { value: u32 },
    #[error(
        "present_target.depth_reduction_min_interval_ms должен лежать в [0, {MAX_DEPTH_REDUCTION_INTERVAL_MS}], а не {value_ms}"
    )]
    DepthReductionIntervalOutOfRange { value_ms: u64 },
}

impl PresentTargetConfig {
    /// Проверяет значения; вызывается из `RenderLoopConfig::validate`.
    pub fn validate(&self) -> Result<(), PresentTargetConfigError> {
        let policy_margin_us = match self.policy {
            PresentTimelinePolicy::Preferred => 0,
            PresentTimelinePolicy::NearestFeasible { margin_us } => margin_us,
        };
        for (field, value_us) in [
            ("policy.margin_us", policy_margin_us),
            ("latch_margin_us", self.latch_margin_us),
            ("initial_frame_cost_us", self.initial_frame_cost_us),
            ("gpu_cost_fallback_us", self.gpu_cost_fallback_us),
            ("latch_spin_us", self.latch_spin_us),
            ("latch_max_wait_us", self.latch_max_wait_us),
        ] {
            if value_us > MAX_PRESENT_TARGET_DURATION_US {
                return Err(PresentTargetConfigError::DurationOutOfRange { field, value_us });
            }
        }
        if self.latch_max_wait_us == 0 {
            return Err(PresentTargetConfigError::ZeroLatchMaxWait);
        }
        for (field, value) in [
            ("cost_window_frames", self.cost_window_frames),
            ("stats_window_frames", self.stats_window_frames),
        ] {
            if !(1..=MAX_PRESENT_TARGET_WINDOW_FRAMES).contains(&value) {
                return Err(PresentTargetConfigError::WindowOutOfRange { field, value });
            }
        }
        if !(1..=MAX_DEPTH_REDUCTION_STREAK_FRAMES).contains(&self.depth_reduction_streak_frames) {
            return Err(PresentTargetConfigError::DepthReductionStreakOutOfRange {
                value: self.depth_reduction_streak_frames,
            });
        }
        if self.depth_reduction_min_interval_ms > MAX_DEPTH_REDUCTION_INTERVAL_MS {
            return Err(PresentTargetConfigError::DepthReductionIntervalOutOfRange {
                value_ms: self.depth_reduction_min_interval_ms,
            });
        }
        if !(1..=self.cost_window_frames).contains(&self.min_cost_samples) {
            return Err(PresentTargetConfigError::MinSamplesOutsideWindow {
                min_cost_samples: self.min_cost_samples,
                cost_window_frames: self.cost_window_frames,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_are_the_tuned_product_settings() {
        let config = PresentTargetConfig::default();
        assert_eq!(config.validate(), Ok(()));
        assert_eq!(
            config.policy,
            PresentTimelinePolicy::NearestFeasible { margin_us: 1_500 }
        );
        assert_eq!(config.late_latch, LateLatchMode::BeforeCameraPoll);
        assert_eq!(config.depth_reduction, DepthReductionMode::SkipOneVsync);
        // Перечисление по умолчанию — прежнее поведение (JSON без политики в командах и тестах).
        assert_eq!(PresentTimelinePolicy::default(), PresentTimelinePolicy::Preferred);
    }

    #[test]
    fn late_latch_mode_is_read_by_name() {
        let config: PresentTargetConfig =
            serde_json::from_str(r#"{"late_latch": "before_camera_poll"}"#).expect("json");
        assert_eq!(config.late_latch, LateLatchMode::BeforeCameraPoll);
        assert!(serde_json::from_str::<PresentTargetConfig>(r#"{"late_latch": "always"}"#).is_err());
    }

    #[test]
    fn nearest_feasible_policy_is_read_from_tagged_json() {
        let config: PresentTargetConfig =
            serde_json::from_str(r#"{"policy": {"kind": "nearest_feasible", "margin_us": 1500}}"#).expect("json");
        assert_eq!(
            config.policy,
            PresentTimelinePolicy::NearestFeasible { margin_us: 1_500 }
        );
        assert_eq!(config.cost_window_frames, 120, "остальное — по умолчанию");
    }

    #[test]
    fn unknown_keys_are_rejected_in_the_section_and_in_the_policy() {
        assert!(serde_json::from_str::<PresentTargetConfig>(r#"{"latch_margn_us": 1}"#).is_err());
        assert!(
            serde_json::from_str::<PresentTargetConfig>(
                r#"{"policy": {"kind": "nearest_feasible", "margin_us": 1, "extra": 2}}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<PresentTargetConfig>(r#"{"policy": {"kind": "earliest"}}"#).is_err());
    }

    #[test]
    fn out_of_range_values_are_rejected_by_field_name() {
        let broken = |change: fn(&mut PresentTargetConfig)| {
            let mut config = PresentTargetConfig::default();
            change(&mut config);
            config.validate()
        };
        assert_eq!(
            broken(|c| c.policy = PresentTimelinePolicy::NearestFeasible { margin_us: 2_000_000 }),
            Err(PresentTargetConfigError::DurationOutOfRange {
                field: "policy.margin_us",
                value_us: 2_000_000
            })
        );
        assert_eq!(
            broken(|c| c.cost_window_frames = 0),
            Err(PresentTargetConfigError::WindowOutOfRange {
                field: "cost_window_frames",
                value: 0
            })
        );
        assert_eq!(
            broken(|c| c.min_cost_samples = 121),
            Err(PresentTargetConfigError::MinSamplesOutsideWindow {
                min_cost_samples: 121,
                cost_window_frames: 120
            })
        );
        assert!(matches!(
            broken(|c| c.min_cost_samples = 0),
            Err(PresentTargetConfigError::MinSamplesOutsideWindow { .. })
        ));
        assert_eq!(
            broken(|c| c.latch_max_wait_us = 0),
            Err(PresentTargetConfigError::ZeroLatchMaxWait)
        );
        assert_eq!(
            broken(|c| c.depth_reduction_streak_frames = 0),
            Err(PresentTargetConfigError::DepthReductionStreakOutOfRange { value: 0 })
        );
        assert_eq!(
            broken(|c| c.depth_reduction_min_interval_ms = MAX_DEPTH_REDUCTION_INTERVAL_MS + 1),
            Err(PresentTargetConfigError::DepthReductionIntervalOutOfRange {
                value_ms: MAX_DEPTH_REDUCTION_INTERVAL_MS + 1
            })
        );
    }

    #[test]
    fn depth_reduction_mode_is_read_by_name_and_skips_one_vsync_by_default() {
        assert_eq!(
            PresentTargetConfig::default().depth_reduction,
            DepthReductionMode::SkipOneVsync
        );
        let config: PresentTargetConfig =
            serde_json::from_str(r#"{"depth_reduction": "disabled", "depth_reduction_streak_frames": 5}"#)
                .expect("json");
        assert_eq!(config.depth_reduction, DepthReductionMode::Disabled);
        assert_eq!(config.depth_reduction_streak_frames, 5);
        assert!(serde_json::from_str::<PresentTargetConfig>(r#"{"depth_reduction": "always"}"#).is_err());
    }
}
