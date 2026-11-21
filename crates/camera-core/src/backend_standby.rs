//! Граница «тёплой» камеры (13b): вторая (и далее) камера открыта и стримит параллельно основной,
//! её кадры не показываются; при переходе порога зума тёплая **повышается** до основной обменом
//! ролей без закрытия и открытия камер.
//!
//! Владелец границы — движок: он выдаёт номера ячеек ([`StandbySlotId`]) и поколения устройств
//! и сессий (из тех же счётчиков, что у основной камеры, поэтому поколения уникальны), а бэкенд
//! хранит свои ресурсы по номеру ячейки. Здесь нет слов «основная» и «ширик»: какие камеры греть,
//! решает правило каталога зума (`zoom::standby_candidates`), а сколько — конфиг и
//! [`StandbyCamera::capacity`].

use camera_contract::control::CaptureIntent;
use camera_contract::ids::CameraId;
use camera_contract::stream::{ConfiguredStream, ResolvedStream};
use serde::Serialize;

use crate::backend::{ConfigureFailure, DeviceGeneration, OpenFailure, RequestFailure, SessionGeneration};
use crate::request_controls::RequestControls;

/// Номер ячейки тёплой камеры (`0..ёмкость`). Выдаёт движок; бэкенд хранит ячейки по номеру.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct StandbySlotId(u8);

impl StandbySlotId {
    /// Ячейка с этим номером.
    pub const fn new(index: u8) -> Self {
        Self(index)
    }

    /// Номер ячейки для индексации массивов.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// Поддерживает ли бэкенд тёплую камеру. Отсутствие — честный вариант, а не `bool`.
pub enum StandbySupport<'a> {
    Available(&'a mut dyn StandbyCamera),
    Unsupported,
}

/// Почему обмен ролей не состоялся. Ничего не изменено: движок идёт обычным путём
/// (закрыть основную, открыть нужную).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StandbySwapFailure {
    /// У тёплой нет идущего потока (ещё не готова или уже закрыта).
    #[error("у тёплой камеры нет идущего потока")]
    StandbyNotStreaming,
    /// У основной нет сессии: обмен бессмыслен.
    #[error("у основной камеры нет сессии")]
    PrimaryNotStreaming,
}

/// Тёплая камера: открыта и стримит параллельно основной, её кадры не показываются.
///
/// Контракт: те же правила потока камеры, что у `CameraBackend`; ни один метод не паникует.
/// Результаты кадров тёплой бэкенд **не доставляет** (`CaptureCompleted`/`CaptureFailed` сессии,
/// которая сейчас тёплая): вместо этого один раз на сессию шлёт `BackendEvent::StandbyStreaming`.
/// После обмена ролей результаты новой основной идут как обычно, бывшей основной — больше нет.
pub trait StandbyCamera {
    /// Сколько ячеек бэкенд держит одновременно (с учётом ресурса HAL).
    fn capacity(&self) -> usize;

    /// Открыть устройство тёплой (основная остаётся открытой). События — с `generation`.
    fn open_standby_device(
        &mut self,
        slot: StandbySlotId,
        camera: &CameraId,
        generation: DeviceGeneration,
    ) -> Result<(), OpenFailure>;

    /// Сессия тёплой: RAW-поток с **полным** пулом основной (после обмена любая станет
    /// основной) и пустой 3A-поток.
    fn configure_standby_session(
        &mut self,
        slot: StandbySlotId,
        stream: &ResolvedStream,
        generation: SessionGeneration,
    ) -> Result<ConfiguredStream, ConfigureFailure>;

    /// Повторяющийся запрос тёплой (экспозицию считает движок; фокус — непрерывный AF).
    fn set_standby_repeating_request(
        &mut self,
        slot: StandbySlotId,
        intent: CaptureIntent,
        controls: &RequestControls,
    ) -> Result<(), RequestFailure>;

    /// Закрыть тёплую (сессию и устройство). Идемпотентно.
    fn close_standby(&mut self, slot: StandbySlotId);

    /// Обмен ролей: тёплая ячейки `slot` становится основной, основная — тёплой **в этой же
    /// ячейке** (обе продолжают стримить). Атомарно и для потока кадров. При отказе ничего
    /// не меняется.
    fn swap_standby_with_primary(&mut self, slot: StandbySlotId) -> Result<(), StandbySwapFailure>;
}
