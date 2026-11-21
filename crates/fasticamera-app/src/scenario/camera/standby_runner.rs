//! Исполнитель шагов сценария `standby` (13b): чистая машина «тик наблюдения → какие намерения объектива публиковать».
//!
//! Исполнитель ничего не вызывает сам: на каждом тике он получает [`StandbyObservation`] и возвращает [`StandbyTick`] —
//! намерения для ящика движка и состояние прогона. Публикует их сценарий-обвязка (`standby.rs`), поэтому все правила
//! проверяются тестами на подставных наблюдениях.
//!
//! # Ход (экран камеры в нём не участвует: намерения кладёт сам сценарий)
//! 1. **Дом.** Задняя камера на 1× (опорный объектив шкалы), тёплых нет.
//! 2. **Одна камера.** Окно замера частоты без тёплой.
//! 3. **Тёплая.** Намерение «1×, плавный жест, греть соседа» → ждём готовую ячейку; её камера и есть «сосед»
//!    (решает движок по каталогу, а не сценарий). Записывается время до готовности.
//! 4. **Обе камеры.** Окно замера: частота показываемой и тёплой эпох.
//! 5. **Обмен туда.** Коэффициент соседа (плавный жест, спрос сохранён) → ждём `LensSwitched` на роль соседа; метод
//!    обязан быть повышением тёплой. После — окно замера (бывшая основная теперь тёплая).
//! 6. **Обмен обратно.** Так же, к «дому».
//! 7. **Отпуск.** Намерение 1× без спроса → ждём, пока ни одна ячейка не держит камеру (одна открытая камера).
//!
//! Одна шкала времени: сроки и «действие → событие» — по часам маршрутизатора (`Instant`).

use std::time::{Duration, Instant};

use camera_contract::ids::{CameraId, LensFacing, LensRole};
use camera_contract::zoom::{LensIntent, StandbyDemand, ZoomGesture, ZoomRatio};
use camera_core::CameraEvent;
use camera_core::lens_status::LensSwitchMethod;
use raw_frame_stream::RawStreamCounters;
use serde::Serialize;

use super::standby_config::StandbyScenarioConfig;
use super::standby_plan::{back_home, ratio_for_continuous_switch, window_fps};
pub use super::standby_views::StandbyEngineView;

/// Один тик наблюдения.
pub struct StandbyObservation<'a> {
    /// Часы маршрутизатора.
    pub now: Instant,
    pub engine: &'a StandbyEngineView,
    /// События движка, пришедшие с прошлого тика (исполнитель — единственный, кто их разбирает).
    pub events: &'a [CameraEvent],
    /// Снимок счётчиков потока кадров.
    pub stream: &'a RawStreamCounters,
}

/// Состояние прогона после тика.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerState {
    Running,
    AllDone,
    Failed(String),
}

/// Итог тика.
#[derive(Debug, Clone, PartialEq)]
pub struct StandbyTick {
    /// Намерения объектива, которые надо положить в ящик движка (по порядку).
    pub intents: Vec<LensIntent>,
    pub state: RunnerState,
}

/// Куда идёт обмен.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Leg {
    /// Основная → соседняя (она стримила тёплой).
    ToNeighbour,
    /// Обратно к дому (бывшая основная стримила тёплой).
    BackHome,
}

/// Один обмен.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SwapRecord {
    pub leg: Leg,
    /// Роль, на которую ждали смену.
    pub to_role: &'static str,
    /// Как движок исполнил смену; обязан быть повышением тёплой.
    pub method: LensSwitchMethod,
    /// От публикации намерения до события движка, мс (часы маршрутизатора).
    pub observed_ms: u64,
    /// Время смены по часам движка (от намерения до первого кадра новой камеры), мс.
    pub engine_elapsed_ms: u64,
}

/// Окно замера частоты.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FpsRecord {
    pub name: &'static str,
    pub window_ms: u64,
    /// Показываемая эпоха, к/с.
    pub primary_fps: f64,
    /// Тёплые эпохи, к/с (в окне «одна камера» их нет — ноль).
    pub standby_fps: f64,
    /// Окно, где обе камеры обязаны держать порог частоты.
    pub both_cameras_running: bool,
}

/// Что исполнитель записал за прогон.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct StandbyRecord {
    pub home_camera: Option<String>,
    pub home_role: Option<&'static str>,
    pub neighbour_camera: Option<String>,
    pub neighbour_role: Option<&'static str>,
    /// От просьбы о тёплой до готовой ячейки, мс.
    pub time_to_ready_ms: Option<u64>,
    pub swaps: Vec<SwapRecord>,
    pub windows: Vec<FpsRecord>,
    /// От «не нужно» до закрытия всех тёплых, мс.
    pub release_ms: Option<u64>,
}

/// Объективы прогона: «дом» и сосед, плюс коэффициенты обменов.
#[derive(Debug, Clone)]
struct Targets {
    home_role: LensRole,
    neighbour_role: LensRole,
    ratio_to_neighbour: ZoomRatio,
    ratio_back_home: ZoomRatio,
}

#[derive(Debug, Clone)]
enum Stage {
    Home {
        entered: Instant,
        published: bool,
    },
    Alone {
        entered: Instant,
        start: RawStreamCounters,
    },
    Warm {
        entered: Instant,
        published: bool,
    },
    Both {
        entered: Instant,
        start: RawStreamCounters,
    },
    Swap {
        leg: Leg,
        entered: Instant,
        published: bool,
    },
    After {
        leg: Leg,
        entered: Instant,
        start: RawStreamCounters,
    },
    Release {
        entered: Instant,
        published: bool,
    },
}

/// Исполнитель шагов.
pub struct StandbyRunner {
    config: StandbyScenarioConfig,
    stage: Option<Stage>,
    targets: Option<Targets>,
    record: StandbyRecord,
    /// Итог прогона, когда он известен; дальше исполнитель только повторяет его.
    outcome: Option<RunnerState>,
}

impl StandbyRunner {
    pub fn new(config: StandbyScenarioConfig, now: Instant) -> Self {
        Self {
            config,
            stage: Some(Stage::Home {
                entered: now,
                published: false,
            }),
            targets: None,
            record: StandbyRecord::default(),
            outcome: None,
        }
    }

    pub fn record(&self) -> &StandbyRecord {
        &self.record
    }

    /// Имя шага, на котором сейчас прогон (для провала и отчёта).
    pub fn stage_name(&self) -> &'static str {
        match self.stage {
            Some(Stage::Home { .. }) => "home",
            Some(Stage::Alone { .. }) => "alone_window",
            Some(Stage::Warm { .. }) => "warm",
            Some(Stage::Both { .. }) => "both_window",
            Some(Stage::Swap {
                leg: Leg::ToNeighbour, ..
            }) => "swap_to_neighbour",
            Some(Stage::Swap { leg: Leg::BackHome, .. }) => "swap_back_home",
            Some(Stage::After {
                leg: Leg::ToNeighbour, ..
            }) => "after_to_neighbour",
            Some(Stage::After { leg: Leg::BackHome, .. }) => "after_back_home",
            Some(Stage::Release { .. }) => "release",
            None => "done",
        }
    }

    /// Прогон дошёл до конца (все шаги выполнены).
    pub fn is_done(&self) -> bool {
        self.outcome == Some(RunnerState::AllDone)
    }

    /// Причина провала, если исполнитель провалился сам.
    pub fn outcome_failure(&self) -> Option<String> {
        match &self.outcome {
            Some(RunnerState::Failed(reason)) => Some(reason.clone()),
            _ => None,
        }
    }

    /// Один тик наблюдения.
    pub fn tick(&mut self, observation: &StandbyObservation<'_>) -> StandbyTick {
        if let Some(outcome) = &self.outcome {
            return StandbyTick {
                intents: Vec::new(),
                state: outcome.clone(),
            };
        }
        let mut intents = Vec::new();
        let state = match self.advance(observation, &mut intents) {
            Ok(()) if self.stage.is_none() => RunnerState::AllDone,
            Ok(()) => RunnerState::Running,
            Err(reason) => RunnerState::Failed(format!("{}: {reason}", self.stage_name())),
        };
        if state != RunnerState::Running {
            self.outcome = Some(state.clone());
        }
        StandbyTick { intents, state }
    }

    fn advance(&mut self, observation: &StandbyObservation<'_>, intents: &mut Vec<LensIntent>) -> Result<(), String> {
        if let Some(failure) = observation.events.iter().find_map(switch_failure_text) {
            return Err(failure);
        }
        let Some(stage) = self.stage.clone() else {
            return Ok(());
        };
        // Отказ тёплой после того, как её попросили, — провал прогона: «любой отказ тёплой» из вердикта.
        let standby_requested = self.targets.is_some() || matches!(stage, Stage::Warm { .. });
        if standby_requested && let Some(failed) = observation.engine.failed_slot() {
            return Err(format!("тёплая отказала: {failed}"));
        }
        match stage {
            Stage::Home { entered, published } => self.step_home(observation, intents, entered, published),
            Stage::Alone { entered, start } => self.step_alone(observation, entered, start),
            Stage::Warm { entered, published } => self.step_warm(observation, intents, entered, published),
            Stage::Both { entered, start } => self.step_both(observation, entered, start),
            Stage::Swap {
                leg,
                entered,
                published,
            } => self.step_swap(observation, intents, leg, entered, published),
            Stage::After { leg, entered, start } => self.step_after(observation, leg, entered, start),
            Stage::Release { entered, published } => self.step_release(observation, intents, entered, published),
        }
    }

    // ---- шаги --------------------------------------------------------------------------------------------------

    fn step_home(
        &mut self,
        observation: &StandbyObservation<'_>,
        intents: &mut Vec<LensIntent>,
        entered: Instant,
        published: bool,
    ) -> Result<(), String> {
        let engine = observation.engine;
        let Some((_, home)) = back_home(&engine.catalog) else {
            return self.wait_or_time_out(
                observation.now,
                entered,
                self.config.home_timeout_ms,
                "каталог зума без задних камер",
            );
        };
        if !published {
            intents.push(home_intent(StandbyDemand::NotNeeded, ZoomGesture::Discrete));
            self.stage = Some(Stage::Home {
                entered,
                published: true,
            });
        }
        let at_home = engine.streaming
            && engine
                .active
                .as_ref()
                .is_some_and(|active| active.facing == LensFacing::Back && active.role == home.role)
            && engine.steady_role == Some(home.role);
        if at_home && !engine.any_slot_holds_camera() {
            self.record.home_camera = Some(home.camera.to_string());
            self.record.home_role = Some(home.role.name());
            self.stage = Some(Stage::Alone {
                entered: observation.now,
                start: *observation.stream,
            });
            return Ok(());
        }
        self.wait_or_time_out(
            observation.now,
            entered,
            self.config.home_timeout_ms,
            "задняя основная камера на 1× без тёплых не наступила",
        )
    }

    fn step_alone(
        &mut self,
        observation: &StandbyObservation<'_>,
        entered: Instant,
        start: RawStreamCounters,
    ) -> Result<(), String> {
        let elapsed = observation.now.saturating_duration_since(entered);
        if elapsed < Duration::from_millis(self.config.alone_window_ms) {
            return Ok(());
        }
        self.record_window("alone", &start, observation, elapsed, false)?;
        self.stage = Some(Stage::Warm {
            entered: observation.now,
            published: false,
        });
        Ok(())
    }

    fn step_warm(
        &mut self,
        observation: &StandbyObservation<'_>,
        intents: &mut Vec<LensIntent>,
        entered: Instant,
        published: bool,
    ) -> Result<(), String> {
        if !published {
            intents.push(home_intent(StandbyDemand::KeepNeighbourWarm, ZoomGesture::Continuous));
            self.stage = Some(Stage::Warm {
                entered,
                published: true,
            });
        }
        let engine = observation.engine;
        let Some(camera) = engine.ready_standby_camera() else {
            return self.wait_or_time_out(
                observation.now,
                entered,
                self.config.warm_timeout_ms,
                &format!("тёплая не стала готовой, ячейки: {}", slots_text(engine)),
            );
        };
        let targets = self.targets_for_neighbour(engine, camera)?;
        self.record.neighbour_camera = Some(camera.to_string());
        self.record.neighbour_role = Some(targets.neighbour_role.name());
        self.record.time_to_ready_ms = Some(millis_since(entered, observation.now));
        self.targets = Some(targets);
        self.stage = Some(Stage::Both {
            entered: observation.now,
            start: *observation.stream,
        });
        Ok(())
    }

    fn step_both(
        &mut self,
        observation: &StandbyObservation<'_>,
        entered: Instant,
        start: RawStreamCounters,
    ) -> Result<(), String> {
        let elapsed = observation.now.saturating_duration_since(entered);
        if elapsed < Duration::from_millis(self.config.both_window_ms) {
            return Ok(());
        }
        self.record_window("both_running", &start, observation, elapsed, true)?;
        self.stage = Some(Stage::Swap {
            leg: Leg::ToNeighbour,
            entered: observation.now,
            published: false,
        });
        Ok(())
    }

    fn step_swap(
        &mut self,
        observation: &StandbyObservation<'_>,
        intents: &mut Vec<LensIntent>,
        leg: Leg,
        entered: Instant,
        published: bool,
    ) -> Result<(), String> {
        let Some(targets) = self.targets.clone() else {
            return Err("нет целей обмена: тёплая не была готова".to_owned());
        };
        let (ratio, expected_role) = match leg {
            Leg::ToNeighbour => (targets.ratio_to_neighbour, targets.neighbour_role),
            Leg::BackHome => (targets.ratio_back_home, targets.home_role),
        };
        if !published {
            intents.push(intent(ratio, ZoomGesture::Continuous, StandbyDemand::KeepNeighbourWarm));
            self.stage = Some(Stage::Swap {
                leg,
                entered,
                published: true,
            });
        }
        let switched = observation.events.iter().find_map(|event| match event {
            CameraEvent::LensSwitched {
                to, elapsed, method, ..
            } if *to == expected_role => Some((*elapsed, *method)),
            _ => None,
        });
        if let Some((engine_elapsed, method)) = switched {
            self.record.swaps.push(SwapRecord {
                leg,
                to_role: expected_role.name(),
                method,
                observed_ms: millis_since(entered, observation.now),
                engine_elapsed_ms: u64::try_from(engine_elapsed.as_millis()).unwrap_or(u64::MAX),
            });
            self.stage = Some(Stage::After {
                leg,
                entered: observation.now,
                start: *observation.stream,
            });
            return Ok(());
        }
        self.wait_or_time_out(
            observation.now,
            entered,
            self.config.swap_timeout_ms,
            &format!("событие LensSwitched на роль {} не пришло", expected_role.name()),
        )
    }

    fn step_after(
        &mut self,
        observation: &StandbyObservation<'_>,
        leg: Leg,
        entered: Instant,
        start: RawStreamCounters,
    ) -> Result<(), String> {
        let elapsed = observation.now.saturating_duration_since(entered);
        if elapsed < Duration::from_millis(self.config.both_window_ms) {
            return Ok(());
        }
        let (name, expected_role, next) = match leg {
            Leg::ToNeighbour => (
                "after_to_neighbour",
                self.neighbour_role()?,
                next_swap(Leg::BackHome, observation.now),
            ),
            Leg::BackHome => (
                "after_back_home",
                self.home_role()?,
                Stage::Release {
                    entered: observation.now,
                    published: false,
                },
            ),
        };
        let active_role = observation.engine.active.as_ref().map(|active| active.role);
        if active_role != Some(expected_role) {
            return Err(format!(
                "после обмена активна роль {active_role:?}, ожидалась {}",
                expected_role.name()
            ));
        }
        self.record_window(name, &start, observation, elapsed, true)?;
        self.stage = Some(next);
        Ok(())
    }

    fn step_release(
        &mut self,
        observation: &StandbyObservation<'_>,
        intents: &mut Vec<LensIntent>,
        entered: Instant,
        published: bool,
    ) -> Result<(), String> {
        if !published {
            intents.push(home_intent(StandbyDemand::NotNeeded, ZoomGesture::Discrete));
            self.stage = Some(Stage::Release {
                entered,
                published: true,
            });
        }
        let engine = observation.engine;
        let home_role = self.home_role()?;
        if !engine.any_slot_holds_camera() && engine.steady_role == Some(home_role) {
            self.record.release_ms = Some(millis_since(entered, observation.now));
            self.stage = None;
            return Ok(());
        }
        self.wait_or_time_out(
            observation.now,
            entered,
            self.config.release_timeout_ms,
            &format!("тёплые не закрылись, ячейки: {}", slots_text(engine)),
        )
    }

    // ---- общее -------------------------------------------------------------------------------------------------

    /// Сосед по камере готовой ячейки и коэффициенты обменов — по каталогу зума.
    fn targets_for_neighbour(&self, engine: &StandbyEngineView, camera: &CameraId) -> Result<Targets, String> {
        let (group, home) = back_home(&engine.catalog).ok_or("каталог зума потерял заднюю группу")?;
        let neighbour = group
            .lens_for_camera(camera)
            .ok_or_else(|| format!("тёплая камера {camera} — не из задней шкалы зума"))?;
        if neighbour.role == home.role {
            return Err(format!("тёплая камера {camera} совпала с «домом»: греть нечего"));
        }
        Ok(Targets {
            home_role: home.role,
            neighbour_role: neighbour.role,
            ratio_to_neighbour: ratio_for_continuous_switch(group, home, neighbour, self.config.overshoot_ratio),
            ratio_back_home: ratio_for_continuous_switch(group, neighbour, home, self.config.overshoot_ratio),
        })
    }

    fn home_role(&self) -> Result<LensRole, String> {
        self.targets
            .as_ref()
            .map(|targets| targets.home_role)
            .ok_or_else(|| "роль «дома» неизвестна: тёплая не была готова".to_owned())
    }

    fn neighbour_role(&self) -> Result<LensRole, String> {
        self.targets
            .as_ref()
            .map(|targets| targets.neighbour_role)
            .ok_or_else(|| "роль соседа неизвестна: тёплая не была готова".to_owned())
    }

    fn record_window(
        &mut self,
        name: &'static str,
        start: &RawStreamCounters,
        observation: &StandbyObservation<'_>,
        elapsed: Duration,
        both_cameras_running: bool,
    ) -> Result<(), String> {
        let fps = window_fps(start, observation.stream, elapsed).ok_or("окно замера нулевой длины")?;
        self.record.windows.push(FpsRecord {
            name,
            window_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            primary_fps: fps.primary,
            standby_fps: fps.standby,
            both_cameras_running,
        });
        Ok(())
    }

    /// Ждать дальше, пока не вышел срок шага; вышел — причина провала.
    fn wait_or_time_out(
        &self,
        now: Instant,
        entered: Instant,
        timeout_ms: u64,
        waiting_for: &str,
    ) -> Result<(), String> {
        let waited = now.saturating_duration_since(entered);
        if waited >= Duration::from_millis(timeout_ms) {
            return Err(format!("не дождались за {} мс: {waiting_for}", waited.as_millis()));
        }
        Ok(())
    }
}

fn next_swap(leg: Leg, now: Instant) -> Stage {
    Stage::Swap {
        leg,
        entered: now,
        published: false,
    }
}

fn millis_since(earlier: Instant, now: Instant) -> u64 {
    u64::try_from(now.saturating_duration_since(earlier).as_millis()).unwrap_or(u64::MAX)
}

/// Намерение задней камеры на заданном коэффициенте.
fn intent(ratio: ZoomRatio, gesture: ZoomGesture, standby: StandbyDemand) -> LensIntent {
    LensIntent::without_standby(LensFacing::Back, ratio, gesture).with_standby(standby)
}

/// Намерение «дома»: 1× (опорный коэффициент шкалы).
fn home_intent(standby: StandbyDemand, gesture: ZoomGesture) -> LensIntent {
    intent(ZoomRatio::ONE, gesture, standby)
}

/// Провал смены объектива из события движка (вина новой камеры): прогон такого не прощает.
fn switch_failure_text(event: &CameraEvent) -> Option<String> {
    match event {
        CameraEvent::LensSwitchFailed { from, to, reason } => Some(format!(
            "смена {}→{} не удалась: {}",
            from.name(),
            to.name(),
            serde_json::to_string(reason).unwrap_or_else(|_| "причина не сериализована".to_owned())
        )),
        _ => None,
    }
}

/// Состояния ячеек одной строкой (для текста провала).
fn slots_text(engine: &StandbyEngineView) -> String {
    if engine.slots.is_empty() {
        return "ячеек нет".to_owned();
    }
    engine
        .slots
        .iter()
        .map(|slot| {
            format!(
                "#{} {} {}",
                slot.slot.index(),
                slot.camera.as_ref().map_or_else(|| "-".to_owned(), ToString::to_string),
                serde_json::to_string(&slot.state).unwrap_or_else(|_| "?".to_owned())
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[path = "standby_runner_tests.rs"]
mod tests;
