//! Поддельный GPU для тестов движка снимков: поведение и наблюдения управляются через общее состояние
//! ([`FakeGpuState`]), без Vulkan и без Android.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    dead_code,
    reason = "помощники тестов: падение — способ сообщить о провале"
)]

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::frame_passport::StreamPassport;
use camera_contract::geometry::PixelSize;
use gpu_vulkan::sync::GpuReleaseFence;
use raw_frame_stream::RawFrameLease;
use shell_contract::surface::NativeSurface;
use still_contract::shot::{OwnedPixels, RawSamples16, Rgba8Image};

use super::gpu::{
    AbandonReport, CapturePoll, DevelopProgress, DevelopStart, PresentProgress, PresentRefusal, StillGpu,
};
use super::test_support::FRAME_SIZE;

/// Поведение и наблюдения поддельного GPU: тест держит `Arc` и правит/читает.
#[derive(Default)]
pub struct FakeGpuState {
    /// Через сколько опросов копия готова (0 — готова на первом).
    pub capture_polls_needed: u32,
    pub capture_polls_done: u32,
    pub fail_capture_start: bool,
    pub fail_capture: bool,
    /// Сколько полос в проявлении и сколько отправлено.
    pub slices_total: u32,
    pub slices_sent: u32,
    pub fail_develop: bool,
    pub present_polls_needed: u32,
    pub present_polls_done: u32,
    pub fail_present_start: bool,
    /// Показ принят, но подключение цели срывается позже — на опросе (как отказ мастерской, решение 144).
    pub fail_present_on_poll: bool,
    pub present_forever: bool,
    pub abandon_idle: bool,
    /// Опрос fence копии не удаётся (исход на GPU неизвестен): `capture_poll` отвечает `Lost`.
    pub lose_capture: bool,
    /// То же для полос проявления.
    pub lose_develop: bool,
    /// То же для показа.
    pub lose_present: bool,
    /// Копия готова, но сырые отсчёты прочитать нельзя.
    pub fail_raw_samples: bool,
    /// Значение всех сырых отсчётов копии (`None` — каждый БАЙТ равен 7: отсчёт 0x0707). Для проверки света вспышки.
    pub raw_fill: Option<u16>,
    /// Копия никогда не завершается (зависший fence).
    pub capture_forever: bool,
    /// Проявление никогда не завершается (зависший fence).
    pub develop_forever: bool,
    /// Порядок значимых событий GPU и картинок: проверяет, что картинка не уничтожена под GPU.
    pub events: Vec<&'static str>,
    /// Сколько раз GPU объявили потерянным.
    pub lost_calls: u32,
    /// Лизинги, оставленные в утечку (не возвращены камере) при потере GPU.
    pub leases_leaked: u32,
    /// Метки захваченных кадров по порядку запуска копий.
    pub captured: Vec<SensorTimestamp>,
    /// Сколько полос отправил каждый вызов `develop_advance`.
    pub slices_per_call: Vec<u32>,
    pub developments_started: u32,
    pub presents_started: u32,
    pub abandon_calls: u32,
    pub released_unused: u32,
    /// Размеры картинок, отданных в показ.
    pub max_in_flight_developments: u32,
    pub flight: Flight,
    /// Лизинг копии: живёт, пока копия не готова.
    pub lease: Option<RawFrameLease>,
    pub readback_available: bool,
    /// Сколько «стоят» вызовы на часах движка: поддельный GPU сдвигает ручные часы внутри вызова.
    pub costs: CallCosts,
}

/// Цена вызовов поддельного GPU на ручных часах движка (замер `StillStepCpu`).
#[derive(Default)]
pub struct CallCosts {
    /// Часы движка; `None` — вызовы мгновенны.
    pub clock: Option<super::test_support::ManualClock>,
    pub develop_start: Duration,
    pub present_start: Duration,
}

impl CallCosts {
    fn spend(&self, cost: Duration) {
        if let Some(clock) = &self.clock {
            clock.advance(cost);
        }
    }
}

#[derive(Default, Clone, Copy)]
pub struct Flight {
    pub capture: bool,
    pub develop: bool,
    pub present: bool,
}

pub type SharedState = Arc<Mutex<FakeGpuState>>;

pub fn gpu_state() -> SharedState {
    Arc::new(Mutex::new(FakeGpuState {
        slices_total: 3,
        abandon_idle: true,
        readback_available: true,
        ..FakeGpuState::default()
    }))
}

#[derive(Clone)]
pub struct FakeRaw {
    pub marker: Arc<Vec<u8>>,
}

pub struct FakeImage {
    pub readback: Option<Rgba8Image>,
    state: SharedState,
}

impl Drop for FakeImage {
    /// Уничтожение картинки пишется в журнал событий: тест сверяет порядок «GPU потерян → картинка не тронута».
    fn drop(&mut self) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .events
            .push("image_dropped");
    }
}

pub struct FakeGpu {
    pub state: SharedState,
    /// Размер полезной области, который GPU «обслуживает».
    pub served: PixelSize,
}

impl FakeGpu {
    fn lock(&self) -> std::sync::MutexGuard<'_, FakeGpuState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl StillGpu for FakeGpu {
    type Raw = FakeRaw;
    type Image = FakeImage;

    fn serves(&self, stream: &StreamPassport) -> bool {
        stream.valid_area.size.width <= self.served.width && stream.valid_area.size.height <= self.served.height
    }

    fn capture_in_flight(&self) -> bool {
        self.lock().flight.capture
    }

    fn develop_in_flight(&self) -> bool {
        self.lock().flight.develop
    }

    fn present_in_flight(&self) -> bool {
        self.lock().flight.present
    }

    fn capture_start(&mut self, lease: RawFrameLease) -> Result<(), String> {
        let mut state = self.lock();
        assert!(!state.flight.capture, "движок запустил вторую копию при занятой первой");
        if state.fail_capture_start {
            lease.release_unused();
            state.released_unused += 1;
            return Err("копия отвергнута".to_owned());
        }
        state.captured.push(lease.passport().sensor_timestamp);
        state.capture_polls_done = 0;
        state.flight.capture = true;
        state.lease = Some(lease);
        Ok(())
    }

    fn capture_poll(&mut self) -> CapturePoll<FakeRaw> {
        let mut state = self.lock();
        if !state.flight.capture {
            return CapturePoll::NothingInFlight;
        }
        if state.lose_capture {
            state.flight.capture = false;
            // Исход неизвестен: лизинг НЕ возвращается камере (учтённая утечка), как у настоящей реализации.
            if let Some(lease) = state.lease.take() {
                drop(lease);
                state.leases_leaked += 1;
            }
            return CapturePoll::Lost("опрос копии не удался".to_owned());
        }
        if state.capture_forever {
            return CapturePoll::Pending;
        }
        if state.capture_polls_done < state.capture_polls_needed {
            state.capture_polls_done += 1;
            return CapturePoll::Pending;
        }
        state.flight.capture = false;
        if let Some(lease) = state.lease.take() {
            lease.release_after_gpu_fence(GpuReleaseFence::already_signaled());
        }
        if state.fail_capture {
            CapturePoll::Failed("копия сорвалась".to_owned())
        } else {
            CapturePoll::Ready(FakeRaw {
                marker: Arc::new(vec![0; 4]),
            })
        }
    }

    fn raw_samples(&self, _raw: &FakeRaw) -> Result<RawSamples16, String> {
        if self.lock().fail_raw_samples {
            return Err("отсчёты не читаются".to_owned());
        }
        let bytes = FRAME_SIZE.width as usize * FRAME_SIZE.height as usize * 2;
        RawSamples16::new(
            FRAME_SIZE,
            Arc::new(OwnedPixels(fill_bytes(self.lock().raw_fill, bytes))),
        )
        .map_err(|error| error.to_string())
    }

    fn develop_start(&mut self, _request: DevelopStart<'_, FakeRaw>) -> Result<(), String> {
        let mut state = self.lock();
        state.costs.spend(state.costs.develop_start);
        assert!(
            !state.flight.develop,
            "движок начал второе проявление при идущем первом"
        );
        state.flight.develop = true;
        state.slices_sent = 0;
        state.developments_started += 1;
        Ok(())
    }

    fn develop_advance(&mut self, max_new_slices: u32) -> DevelopProgress<FakeImage> {
        let mut state = self.lock();
        if !state.flight.develop {
            return DevelopProgress::Idle;
        }
        if state.lose_develop {
            state.flight.develop = false;
            return DevelopProgress::Lost("опрос полосы не удался".to_owned());
        }
        if state.develop_forever {
            return DevelopProgress::InFlight { submitted_now: 0 };
        }
        if state.fail_develop {
            state.flight.develop = false;
            return DevelopProgress::Failed("проявление сорвалось".to_owned());
        }
        let remaining = state.slices_total - state.slices_sent;
        let submit = remaining.min(max_new_slices);
        state.slices_sent += submit;
        state.slices_per_call.push(submit);
        if state.slices_sent >= state.slices_total && submit == 0 {
            state.flight.develop = false;
            let readback = state.readback_available.then(|| {
                let bytes = FRAME_SIZE.width as usize * FRAME_SIZE.height as usize * 4;
                Rgba8Image::new(FRAME_SIZE, Arc::new(OwnedPixels(vec![9; bytes]))).expect("размер картинки")
            });
            return DevelopProgress::Done(FakeImage {
                readback,
                state: Arc::clone(&self.state),
            });
        }
        DevelopProgress::InFlight { submitted_now: submit }
    }

    fn take_readback(&self, image: &mut FakeImage) -> Option<Rgba8Image> {
        image.readback.take()
    }

    fn present_start(&mut self, _image: &FakeImage, _surface: NativeSurface) -> Result<(), PresentRefusal> {
        let mut state = self.lock();
        state.costs.spend(state.costs.present_start);
        if state.fail_present_start {
            return Err(PresentRefusal("поверхность кодера отвергнута".to_owned()));
        }
        state.flight.present = true;
        state.present_polls_done = 0;
        state.presents_started += 1;
        Ok(())
    }

    fn present_poll(&mut self) -> PresentProgress {
        let mut state = self.lock();
        if !state.flight.present {
            return PresentProgress::Idle;
        }
        if state.lose_present {
            state.flight.present = false;
            return PresentProgress::Lost("опрос показа не удался".to_owned());
        }
        if state.fail_present_on_poll {
            state.flight.present = false;
            return PresentProgress::Failed("поверхность кодера: подключение отвергнуто".to_owned());
        }
        if state.present_forever || state.present_polls_done < state.present_polls_needed {
            state.present_polls_done += 1;
            return PresentProgress::Pending;
        }
        state.flight.present = false;
        PresentProgress::Done
    }

    fn declare_lost(&mut self) {
        let mut state = self.lock();
        state.lost_calls += 1;
        state.events.push("gpu_lost");
        // Лизинг не возвращается камере: GPU мог ещё читать кадр (учтённая утечка).
        if let Some(lease) = state.lease.take() {
            drop(lease);
            state.leases_leaked += 1;
        }
        state.flight = Flight::default();
    }

    fn abandon(&mut self) -> AbandonReport {
        let mut state = self.lock();
        state.abandon_calls += 1;
        state.events.push("abandon");
        if let Some(lease) = state.lease.take() {
            lease.release_unused();
            state.released_unused += 1;
        }
        state.flight = Flight::default();
        AbandonReport {
            gpu_idle: state.abandon_idle,
        }
    }
}

/// Байты отсчётов: `bytes / 2` значений `value` (little-endian); без значения — каждый байт 7 (как до вспышки).
fn fill_bytes(value: Option<u16>, bytes: usize) -> Vec<u8> {
    match value {
        Some(value) => value.to_le_bytes().iter().copied().cycle().take(bytes).collect(),
        None => vec![7; bytes],
    }
}
