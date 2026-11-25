//! Словарь вспышки (сессия 14d, решения 153, 158, 159): что ходит между экраном, движком камеры,
//! бэкендом Camera2 и потоком рендера, когда снимок делается со вспышкой.
//!
//! Как снимается кадр со вспышкой (решение 158, факты зонда `flash` 14d):
//! 1. Движок камеры (`camera-core`) на несколько кадров включает в ПОВТОРЯЮЩЕМСЯ запросе фонарик
//!    ([`FlashControl::Torch`]) со своей экспозицией предвспышки. Это «предвспышка».
//! 2. Поток рендера мерит кадры без фонарика и с фонариком и кладёт линейные замеры
//!    ([`PreflashSample`]) в ящик. Какой кадр чем освещён, он узнаёт по результату САМОГО кадра
//!    ([`FlashResult::light`]), а не по времени.
//! 3. Движок считает сцену под вспышкой: свет без вспышки плюс R × вклад фонарика (разовая вспышка
//!    на этом телефоне в 1,87 раза ярче фонарика). Под неё выбираются выдержка и ISO разового
//!    запроса [`FlashControl::Single`], опознаваемого по [`FlashShotId`].
//! 4. Рендер находит кадр этого запроса в кольце снимков по его метке запроса и проявляет его
//!    с подтяжкой из [`FlashFramePlan`] и белым кадра с фонариком (тот же светодиод).
//!
//! Здесь только данные и их инварианты; формулу и последовательность держит `camera-core`.

use serde::Serialize;

use crate::auto_exposure::{CameraTag, ExposureProduct, MeasurementQuality};
use crate::capture_result::SensorTimestamp;
use crate::control::WhiteBalanceSetting;

/// Есть ли у камеры вспышка (`android.flash.info.available`). Нет вспышки — экран её не
/// показывает (инвариант 6 README), движок отказывает в снимке со вспышкой.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlashUnit {
    /// Вспышки нет или камера о ней не сообщила.
    #[default]
    Absent,
    /// Вспышка есть.
    Present,
}

impl FlashUnit {
    /// Можно ли снимать со вспышкой на этой камере.
    pub const fn is_present(self) -> bool {
        matches!(self, FlashUnit::Present)
    }
}

/// Что светодиод делает в ОДНОМ запросе (`android.flash.mode`).
///
/// Инвариант: значение пишется в КАЖДЫЙ запрос явно, включая `Off`. Ключи HAL «липкие»
/// (`mem:conventions`, 3A): запрос без ключа может унаследовать фонарик прошлого запроса.
/// Работает только при `AE_MODE_OFF`/`ON` (документация `CaptureRequest.FLASH_MODE`); у нас AE
/// всегда `OFF` (решение 63).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlashControl {
    /// Светодиод не горит.
    #[default]
    Off,
    /// Фонарик: горит на каждом кадре этого (повторяющегося) запроса — предвспышка.
    Torch,
    /// Разовая вспышка на кадре этого запроса.
    Single,
}

/// Номер снимка со вспышкой. По нему кадр разового запроса опознаётся в кольце снимков
/// ([`crate::three_a::RequestOrigin::FlashStill`]) — по запросу, а не по времени.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct FlashShotId(pub u64);

/// Режим вспышки, который HAL сообщил в результате кадра (эхо `android.flash.mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlashModeEcho {
    Off,
    Single,
    Torch,
}

/// Состояние вспышки в результате кадра (`android.flash.state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlashState {
    /// У камеры нет вспышки.
    Unavailable,
    /// Заряжается, выстрелить сейчас не может.
    Charging,
    /// Готова, на этом кадре не горела.
    Ready,
    /// Горела на этом кадре (разовая вспышка или фонарик).
    Fired,
    /// Горела только на части кадра.
    Partial,
}

/// Что HAL сообщил о вспышке на одном кадре. `None` — ключ не пришёл.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
pub struct FlashResult {
    pub mode: Option<FlashModeEcho>,
    pub state: Option<FlashState>,
}

impl FlashResult {
    /// HAL о вспышке ничего не сообщил.
    pub const UNREPORTED: FlashResult = FlashResult {
        mode: None,
        state: None,
    };

    /// Чем освещён кадр — по его собственному результату.
    ///
    /// Правило (факты зонда 14d: `FIRED` бывает только у кадра с фонариком или у кадра разовой
    /// вспышки, у соседних — `READY`):
    /// - `PARTIAL` — частично освещён;
    /// - эхо `TORCH` — кадр предвспышки (состояние не пришло — всё равно фонарик: эхо режима
    ///   говорит, что светодиод был включён запросом);
    /// - `FIRED` без эха фонарика — кадр разовой вспышки;
    /// - эхо `SINGLE` без состояния — доказать, что вспышка была, нечем: [`FrameLight::Unknown`];
    /// - всё остальное, включая полное отсутствие ключей, — обычный свет сцены.
    pub const fn light(self) -> FrameLight {
        match (self.mode, self.state) {
            (_, Some(FlashState::Partial)) => FrameLight::PartiallyLit,
            (Some(FlashModeEcho::Torch), Some(FlashState::Fired) | None) => FrameLight::TorchLit,
            (_, Some(FlashState::Fired)) => FrameLight::FlashFired,
            (Some(FlashModeEcho::Single), None) => FrameLight::Unknown,
            _ => FrameLight::Ambient,
        }
    }
}

/// Чем освещён кадр по его собственному результату ([`FlashResult::light`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameLight {
    /// Обычный свет сцены, светодиод не горел.
    Ambient,
    /// Горел фонарик предвспышки.
    TorchLit,
    /// Горела разовая вспышка.
    FlashFired,
    /// Светодиод горел только на части кадра.
    PartiallyLit,
    /// Вспышку просили, но HAL не сообщил, горела ли она.
    Unknown,
}

impl FrameLight {
    /// Годится ли кадр для сглаживания AE и AWB видоискателя. Кадры, освещённые светодиодом,
    /// сдвинули бы экспозицию и белый видоискателя к свету предвспышки (`Unknown` — тоже нет:
    /// вспышку просили).
    pub const fn feeds_viewfinder_smoothing(self) -> bool {
        matches!(self, FrameLight::Ambient)
    }
}

/// Каким светом снят замер предвспышки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflashLight {
    /// Кадр без светодиода.
    Ambient,
    /// Кадр с фонариком.
    Torch,
}

/// Линейный замер одного кадра для расчёта вспышки (рендер → движок).
///
/// Отличие от [`crate::auto_exposure::ExposureProposal`]: предложение сглажено и защищает света,
/// а здесь — один конкретный кадр и ЛИНЕЙНАЯ величина, без защиты светов и без ограничения шага.
/// Только линейные замеры можно складывать как свет (решение 158).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PreflashSample {
    /// Кадр, по которому сделан замер (`SENSOR_TIMESTAMP`).
    pub frame: SensorTimestamp,
    /// Камера кадра: замер чужой камеры движок отвергает.
    pub measured_on: CameraTag,
    /// Свет кадра — по результату самого кадра ([`FlashResult::light`]).
    pub light: PreflashLight,
    /// Выдержка × ISO, с которыми снят кадр (из спаренного результата).
    pub frame_product: ExposureProduct,
    /// Выдержка × ISO, при которых среднее этого кадра встало бы ровно на средний серый
    /// при EV 0 — линейно: `frame_product × цель / среднее`.
    pub mid_gray_product: ExposureProduct,
    /// Годность замера. `ClippedHighlights` — среднее занижено пересветом (замер обманывает
    /// в сторону «темнее»); `TooDark` — сигнала мало, величина шумная.
    pub quality: MeasurementQuality,
}

/// Последние замеры предвспышки по видам света. Отдельные места для «без фонарика» и
/// «с фонариком»: в одном ящике «последнее побеждает» второй вид затирал бы первый.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct PreflashSamples {
    pub ambient: Option<PreflashSample>,
    pub torch: Option<PreflashSample>,
}

impl PreflashSamples {
    /// Положить замер на его место по виду света.
    pub fn record(&mut self, sample: PreflashSample) {
        match sample.light {
            PreflashLight::Ambient => self.ambient = Some(sample),
            PreflashLight::Torch => self.torch = Some(sample),
        }
    }
}

/// Нужны ли рендеру замеры предвспышки (движок → рендер, часть указания AE).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PreflashMetering {
    /// Предвспышки нет: статистика по обычному расписанию.
    #[default]
    Inactive,
    /// Идёт предвспышка снимка `shot`: рендер мерит КАЖДЫЙ кадр и публикует замеры.
    Active { shot: FlashShotId },
}

/// Доля света вспышки в кадре со вспышкой, 0…1 (остальное — свет сцены).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct FlashShare(f64);

impl FlashShare {
    /// Доля из числа; нечисло и выход за 0…1 обрезаются до ближайшего края (нечисло — 0).
    pub fn new(share: f64) -> Self {
        if share.is_nan() {
            return FlashShare(0.0);
        }
        FlashShare(share.clamp(0.0, 1.0))
    }

    pub const fn value(self) -> f64 {
        self.0
    }
}

/// План кадра со вспышкой, который движок выбрал (движок → рендер).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct FlashFramePlan {
    pub shot: FlashShotId,
    /// Кадр без светодиода, по замеру которого движок посчитал план. Рендер проверяет «кадр освещён»
    /// по уровню света ИМЕННО этого кадра, а не «последнего без светодиода»: на части камер первый
    /// кадр с фонариком приходит ЧАСТИЧНО освещённым без эха `TORCH` и выглядит как обычный, и его
    /// уровень как «свет сцены» занизил бы вклад вспышки.
    pub ambient_frame: SensorTimestamp,
    /// Выдержка × ISO, при которых средний серый сцены под вспышкой ложится в цель с учётом EV
    /// пользователя. Рендер делит её на выдержку × ISO кадра и получает остаточную подтяжку
    /// проявления (как `k` видоискателя, решение 72).
    pub mid_gray_product: ExposureProduct,
    /// Доля вспышки в свете кадра (диагностика и будущее смешение белого).
    pub flash_share: FlashShare,
    /// Белая кадра со вспышкой: постоянный цвет светодиода активной камеры снимка (WB11, WB41). Рендер
    /// проявляет кадр с ней при ЛЮБОЙ белой видоискателя; оценок сцены и подстановок нет.
    pub led_white: WhiteBalanceSetting,
}

/// Часть указания AE про вспышку (движок → рендер).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct FlashDirective {
    pub preflash: PreflashMetering,
    /// Последний выбранный план кадра со вспышкой; рендер запоминает его по `shot`
    /// (следующая предвспышка может заменить указание раньше, чем кадр дойдёт до проявления).
    pub planned: Option<FlashFramePlan>,
}

impl FlashDirective {
    /// Вспышки нет.
    pub const INACTIVE: FlashDirective = FlashDirective {
        preflash: PreflashMetering::Inactive,
        planned: None,
    };
}

/// Почему снимок со вспышкой не снят (итог движка камеры, его видит и рендер, и экран).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlashStillFailure {
    /// У активной камеры нет вспышки.
    NoFlashUnit,
    /// Экспозицию ведёт не наш AE (вендорский A/B): посчитать вспышку нечем.
    NeedsOwnExposure,
    /// Камера не умеет ручной сенсор.
    NeedsManualSensor,
    /// Поток не идёт.
    NotStreaming,
    /// Уже идёт другой снимок со вспышкой.
    AlreadyInProgress,
    /// Предвспышка не дала годного замера в срок.
    PreflashNotMeasured,
    /// HAL отверг разовый запрос.
    SubmitFailed,
    /// Результат кадра со вспышкой не пришёл в срок.
    ResultTimedOut,
    /// Сессия камеры потеряна посреди снимка.
    SessionLost,
    /// Объектив сменился посреди снимка.
    LensChanged,
    /// Команда не дошла до движка: очередь команд полна или движок остановлен
    /// (`CommandRejected` ручки камеры).
    CommandNotDelivered,
}

#[cfg(test)]
#[path = "flash_tests.rs"]
mod tests;
