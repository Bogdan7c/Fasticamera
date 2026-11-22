//! План снимка (механизм М4): что снять и какие файлы получить.
//!
//! План собирает приложение из нажатия на экране и передаёт рендеру. Всё, что меняется во времени
//! (зум, соотношение, поворот телефона), фиксируется в плане НА МОМЕНТ НАЖАТИЯ: снимок не должен
//! зависеть от того, что пользователь сделал после (решение 3 сессии 14a — тот же принцип для вида).

use camera_contract::zoom::ZoomFraming;
use serde::Serialize;
use shell_contract::clock::MonotonicNanos;
use shell_contract::orientation::DeviceOrientation;

/// Номер снимка. Сквозной на процесс: его выдаёт сервис снимка, а не экран
/// (номера экрана начинаются с 1 у каждого нового экрана).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct StillId(pub u64);

/// Нажатие затвора: момент по часам касаний (`MONOTONIC`).
///
/// Метка сенсора живёт на другой шкале (`BOOTTIME`), поэтому кадр выбирается не арифметикой часов,
/// а по записям показа рендера: какой кадр стоял на экране в этот момент.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ShutterPress {
    pub at: MonotonicNanos,
}

/// Как выбрать кадр. В v1 — один кадр «что было на экране»; серия, брекетинг и кадр со вспышкой (14d) —
/// будущие варианты этого же перечисления, а не новые сервисы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameSelection {
    /// Кадр сенсора, который был на экране в момент нажатия (без задержки затвора).
    ShownAtPress,
}

/// Формат основной картинки (решение 11: по умолчанию HEIF, JPEG — выбор в настройках).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrimaryFormat {
    /// HEIF через аппаратный кодер; рендер рисует снимок прямо в его поверхность (решение 142).
    Heif,
    /// JPEG: картинка читается с GPU и сжимается платформой.
    Jpeg,
}

impl PrimaryFormat {
    /// MIME-тип файла для хранилища.
    pub fn mime_type(self) -> &'static str {
        match self {
            Self::Heif => "image/heic",
            Self::Jpeg => "image/jpeg",
        }
    }

    /// Расширение имени файла (без точки).
    pub fn file_extension(self) -> &'static str {
        match self {
            Self::Heif => "HEIC",
            Self::Jpeg => "JPG",
        }
    }
}

/// Нужен ли сырой файл рядом со снимком (кнопка RAW, решение 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RawOutput {
    /// Только основная картинка.
    Skip,
    /// Ещё DNG рядом (своя запись по спецификации DNG 1.7).
    Dng,
}

/// Набор выходов снимка. В v1 — «основной + DNG по кнопке»; «спутники» (видео Live, глубина) — позже.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct StillOutputs {
    pub primary: PrimaryFormat,
    pub raw: RawOutput,
}

impl StillOutputs {
    /// Нужны ли сырые данные кадра на CPU (для DNG).
    pub fn needs_raw_samples(&self) -> bool {
        self.raw == RawOutput::Dng
    }
}

/// План одного снимка.
#[derive(Debug, Clone, PartialEq)]
pub struct StillPlan {
    pub id: StillId,
    pub press: ShutterPress,
    pub selection: FrameSelection,
    /// Кадрирование на момент нажатия: соотношение сторон, зум и шкала камер — та же формула,
    /// что у видоискателя (`ZoomFraming::crop_for_camera`), поэтому снимок = то, что было в кадре.
    pub framing: ZoomFraming,
    /// Как держали телефон в момент нажатия: определяет тег поворота файлов (пиксели не вращаются).
    pub orientation: DeviceOrientation,
    pub outputs: StillOutputs,
}
