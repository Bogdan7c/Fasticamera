//! Доведение ячеек до спроса: кого открыть, кого закрыть, чей запрос перепоставить.
//!
//! Зовётся хвостом `reconcile` — **после** основной камеры, поэтому тёплые никогда не задерживают
//! её доведение и не меняют её состояние. Подъём одной ячейки идёт теми же шагами, что у основной
//! (`open → configure → request`), синхронными вызовами бэкенда; отказ любого шага уводит ячейку
//! в `Failed` с причиной, основная не затрагивается.

use std::time::Instant;

use camera_contract::ids::CameraId;
use camera_contract::stream::ResolvedStream;

use super::request::{IssuedRequest, build_standby_request, needs_reissue};
use super::{Phase, Slot};
use crate::backend::{ConfigureFailure, OpenFailure, UnsupportedStream};
use crate::backend_standby::{StandbyCamera, StandbySlotId, StandbySupport};
use crate::engine::CameraEngine;
use crate::engine::{LOG_TARGET, LiveSession, OpenDevice};
use crate::standby_status::StandbyFailureReason;
use crate::zoom::standby_candidates;

impl CameraEngine {
    /// Бэкенд тёплых; `None` — бэкенд их не поддерживает.
    fn standby_backend(&mut self) -> Option<&mut dyn StandbyCamera> {
        match self.backend.standby() {
            StandbySupport::Available(camera) => Some(camera),
            StandbySupport::Unsupported => None,
        }
    }

    /// Рабочая ёмкость: меньшее из конфига и бэкенда (0, если бэкенд тёплых не поддерживает).
    fn standby_capacity(&mut self) -> usize {
        let configured = self.config.standby.max_cameras;
        self.standby_backend()
            .map_or(0, |backend| backend.capacity().min(configured))
    }

    /// Основная камера стримит: только тогда тёплые имеют смысл и смену можно ускорить.
    pub(in crate::engine) fn primary_is_streaming(&self) -> bool {
        self.device.is_some() && self.session.as_ref().is_some_and(|session| session.repeating.is_some())
    }

    /// Довести тёплые до спроса. Идемпотентно: нечего делать — ни одного вызова бэкенда.
    pub(in crate::engine) fn sync_standby(&mut self, now: Instant) {
        if self.reconcile_is_blocked() {
            return;
        }
        if self.standby.release_is_due(now) {
            self.close_standby_resources();
            self.standby.end_demand();
        }
        let capacity = self.standby_capacity();
        if capacity == 0 || !self.standby.has_demand() || !self.primary_is_streaming() {
            // Нет ёмкости, спроса или основной, которой тёплая нужна рядом: открытых тёплых быть не должно.
            self.close_standby_resources();
            return;
        }
        self.standby.ensure_slots(capacity);
        self.close_outdated_standby();
        if self.standby.wants_neighbours() {
            self.close_unwanted_standby(capacity);
            self.open_missing_standby(capacity, now);
        }
        self.reissue_standby_requests_if_needed(now);
    }

    /// Закрыть все открытые ячейки (ресурсы бэкенда и состояние); `Failed` остаётся до конца спроса.
    pub(in crate::engine) fn close_standby_resources(&mut self) {
        let open_slots: Vec<StandbySlotId> = self
            .standby
            .slots
            .iter()
            .filter(|slot| slot.holds_resources())
            .map(|slot| slot.id)
            .collect();
        for slot in open_slots {
            self.close_standby_slot(slot);
        }
    }

    /// Закрыть одну ячейку: бэкенду — `close_standby` (идемпотентно), состоянию — «свободна».
    fn close_standby_slot(&mut self, slot: StandbySlotId) {
        if let Some(backend) = self.standby_backend() {
            backend.close_standby(slot);
        }
        if let Some(state) = self.standby.slot_mut(slot) {
            state.become_idle();
        }
    }

    /// Ячейка отказала: закрыть её ресурсы и записать причину. Основная не затрагивается.
    pub(in crate::engine) fn fail_standby_slot(&mut self, slot: StandbySlotId, reason: StandbyFailureReason) {
        tracing::warn!(target: LOG_TARGET, slot = slot.index(), ?reason, "тёплая камера отказала");
        if let Some(backend) = self.standby_backend() {
            backend.close_standby(slot);
        }
        self.standby.counters.failures += 1;
        if let Some(state) = self.standby.slot_mut(slot) {
            let camera = state.camera.take();
            state.become_idle();
            state.camera = camera;
            state.phase = Phase::Failed(reason);
        }
    }

    /// Сессия ячейки не совпадает с потоком, который сейчас дал бы режим для её камеры
    /// (сменили режим или частоту): такую тёплую повышать нельзя, закрываем.
    fn close_outdated_standby(&mut self) {
        let outdated: Vec<StandbySlotId> = self
            .standby
            .slots
            .iter()
            .filter(|slot| slot.holds_resources())
            .filter(|slot| {
                let wanted = slot
                    .camera
                    .as_ref()
                    .and_then(|camera| self.catalog.camera(camera))
                    .and_then(|descriptor| self.resolved_stream_for(descriptor));
                let current = slot.session.as_ref().map(|session| &session.configured.stream);
                // Сессии ещё нет (`Opening`) — сравнивать не с чем.
                current.is_some() && current != wanted.as_ref()
            })
            .map(|slot| slot.id)
            .collect();
        for slot in outdated {
            self.close_standby_slot(slot);
        }
    }

    /// Соседи основной по правилу каталога зума, ближайшие к коэффициенту экрана первыми.
    fn standby_candidate_cameras(&self, capacity: usize) -> Vec<CameraId> {
        let Some(primary_camera) = self.session.as_ref().map(|session| &session.configured.stream.camera) else {
            return Vec::new();
        };
        let zoom_catalog = self.zoom_catalog_handle();
        let Some((group, lens)) = zoom_catalog.lens_for_camera(primary_camera) else {
            return Vec::new();
        };
        // Коэффициент экрана — из последнего намерения того же направления; иначе собственный.
        let ratio = self.screen_ratio_for(group.facing).unwrap_or(lens.native_ratio);
        standby_candidates(&zoom_catalog, group.facing, lens.role, ratio, capacity)
            .into_iter()
            .map(|candidate| candidate.camera.clone())
            .collect()
    }

    /// Закрыть открытые ячейки, чьей камеры больше нет среди кандидатов (колесо ушло к другому соседу).
    fn close_unwanted_standby(&mut self, capacity: usize) {
        let candidates = self.standby_candidate_cameras(capacity);
        let unwanted: Vec<StandbySlotId> = self
            .standby
            .slots
            .iter()
            .filter(|slot| slot.holds_resources())
            .filter(|slot| slot.camera.as_ref().is_none_or(|camera| !candidates.contains(camera)))
            .map(|slot| slot.id)
            .collect();
        for slot in unwanted {
            self.close_standby_slot(slot);
        }
    }

    /// Поднять кандидатов, у которых ещё нет ячейки, пока есть свободные ячейки.
    fn open_missing_standby(&mut self, capacity: usize, now: Instant) {
        for camera in self.standby_candidate_cameras(capacity) {
            if self.standby.slot_holding(&camera).is_some() {
                continue;
            }
            let Some(slot) = self.standby.free_slot() else {
                break;
            };
            self.bring_up_standby(slot, &camera, now);
        }
    }

    /// Подъём одной ячейки: открыть, сконфигурировать, поставить запрос. Первый отказ — `Failed`.
    fn bring_up_standby(&mut self, slot: StandbySlotId, camera: &CameraId, now: Instant) {
        let Some(descriptor) = self.catalog.camera(camera).cloned() else {
            return;
        };
        let Some(stream) = self.resolved_stream_for(&descriptor) else {
            // Желаемого режима нет (или формата у камеры нет): греть нечем и незачем.
            self.record_unstreamable_standby(slot, camera);
            return;
        };
        if let Some(state) = self.standby.slot_mut(slot) {
            state.camera = Some(camera.clone());
        }
        if !self.open_standby_device(slot, camera) || !self.configure_standby_session(slot, &stream) {
            return;
        }
        self.issue_standby_request(slot, &stream, now);
    }

    /// Камеры нет под желаемый поток: ячейка получает честный отказ, а не молчаливый пропуск.
    fn record_unstreamable_standby(&mut self, slot: StandbySlotId, camera: &CameraId) {
        let Some(mode) = self.desired.mode else {
            return;
        };
        if let Some(state) = self.standby.slot_mut(slot) {
            state.camera = Some(camera.clone());
        }
        let failure = ConfigureFailure::Unsupported(UnsupportedStream::FormatNotOffered {
            format: mode.raw_format,
        });
        self.fail_standby_slot(slot, StandbyFailureReason::ConfigureFailed { failure });
    }

    /// Шаг «открыть устройство». `true` — открыто.
    fn open_standby_device(&mut self, slot: StandbySlotId, camera: &CameraId) -> bool {
        self.counters.devices_opened += 1;
        let generation = crate::backend::DeviceGeneration(self.counters.devices_opened);
        let Some(backend) = self.standby_backend() else {
            return false;
        };
        match backend.open_standby_device(slot, camera, generation) {
            Ok(()) => {
                self.standby.counters.opened += 1;
                if let Some(state) = self.standby.slot_mut(slot) {
                    state.device = Some(OpenDevice {
                        camera: camera.clone(),
                        generation,
                    });
                    state.phase = Phase::Opening;
                }
                true
            }
            Err(failure @ (OpenFailure::Unavailable { .. } | OpenFailure::Backend(_))) => {
                self.fail_standby_slot(slot, StandbyFailureReason::OpenFailed { failure });
                false
            }
        }
    }

    /// Шаг «сконфигурировать сессию». `true` — сконфигурирована.
    fn configure_standby_session(&mut self, slot: StandbySlotId, stream: &ResolvedStream) -> bool {
        self.counters.sessions_configured += 1;
        let generation = crate::backend::SessionGeneration(self.counters.sessions_configured);
        let Some(backend) = self.standby_backend() else {
            return false;
        };
        match backend.configure_standby_session(slot, stream, generation) {
            Ok(configured) => {
                if let Some(state) = self.standby.slot_mut(slot) {
                    state.session = Some(LiveSession {
                        generation,
                        configured,
                        repeating: None,
                    });
                    state.phase = Phase::Configured;
                }
                true
            }
            Err(failure) => {
                self.fail_standby_slot(slot, StandbyFailureReason::ConfigureFailed { failure });
                false
            }
        }
    }

    /// Построить запрос тёплой по запросу основной; `None` — не из чего (основная не стримит).
    fn wanted_standby_request(&self, camera: &CameraId, stream: &ResolvedStream) -> Option<IssuedRequest> {
        let intent = self.desired.streaming?;
        let primary_controls = self.session.as_ref()?.repeating?.controls;
        let primary_camera = &self.device.as_ref()?.camera;
        let primary_caps = &self.catalog.camera(primary_camera)?.capabilities;
        let standby_caps = &self.catalog.camera(camera)?.capabilities;
        let (controls, source_product) = build_standby_request(
            &primary_controls,
            primary_caps,
            standby_caps,
            stream.frame_rate.frame_duration(),
        );
        Some(IssuedRequest {
            intent,
            controls,
            source_product,
        })
    }

    /// Шаг «поставить запрос» (первый или пересчёт). Отказ — `Failed`.
    fn issue_standby_request(&mut self, slot: StandbySlotId, stream: &ResolvedStream, _now: Instant) {
        let Some(wanted) = self.wanted_standby_request(&stream.camera, stream) else {
            return;
        };
        let Some(backend) = self.standby_backend() else {
            return;
        };
        match backend.set_standby_repeating_request(slot, wanted.intent, &wanted.controls) {
            Ok(()) => {
                self.standby.counters.requests_issued += 1;
                if let Some(state) = self.standby.slot_mut(slot) {
                    state.issued = Some(wanted);
                    // Запрос в первый раз: теперь ждём первого результата. Готовая ячейка
                    // (бывшая основная) готовой и остаётся.
                    if state.phase == Phase::Configured {
                        state.phase = Phase::Warming;
                    }
                }
            }
            Err(failure) => {
                self.fail_standby_slot(slot, StandbyFailureReason::RequestFailed { failure });
            }
        }
    }

    /// Перепоставить запросы тех, у кого экспозиция основной заметно сдвинулась (или запроса ещё нет).
    fn reissue_standby_requests_if_needed(&mut self, now: Instant) {
        let threshold_ev = f64::from(self.config.three_a.exposure.replan_min_change_ev);
        let pending: Vec<(StandbySlotId, ResolvedStream)> = self
            .standby
            .slots
            .iter()
            .filter_map(|slot| self.slot_needing_request(slot, threshold_ev))
            .collect();
        for (slot, stream) in pending {
            self.issue_standby_request(slot, &stream, now);
        }
    }

    /// Нужен ли ячейке новый запрос; если да — её поток.
    fn slot_needing_request(&self, slot: &Slot, threshold_ev: f64) -> Option<(StandbySlotId, ResolvedStream)> {
        if !matches!(slot.phase, Phase::Configured | Phase::Warming | Phase::Ready) {
            return None;
        }
        let stream = slot.session.as_ref()?.configured.stream.clone();
        let wanted = self.wanted_standby_request(&stream.camera, &stream)?;
        let stale = slot
            .issued
            .as_ref()
            .is_none_or(|issued| needs_reissue(issued, &wanted, threshold_ev));
        stale.then_some((slot.id, stream))
    }
}
