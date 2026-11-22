//! Настройки съёмки, которыми владеет экран: вспышка, RAW, таймер, соотношение сторон, сетка, уровень.
//!
//! Это «желаемое пользователем», а не факты камеры: экран хранит выбор и передаёт его в снимок
//! (`ShotRequest`). Предустановленные значения (`#[default]`) — обычное состояние первого запуска;
//! сохранение между запусками («Preserve Settings») — сессия 16.

use camera_contract::viewfinder_crop::CropAspect;

/// Режим вспышки.
///
/// `Auto` входит в v1 (решение 153): экран хранит и передаёт в снимок именно выбор пользователя.
/// Решение «стрелять ли вспышкой» для `Auto` принимает приложение/движок по оценке AE, а не экран
/// (решение 159). Режим по умолчанию при запуске — `Off` (решение 160).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FlashMode {
    /// Вспышка выключена.
    #[default]
    Off,
    /// Вспышка включена.
    On,
    /// Автовспышка: сработает ли она, решает оценка AE (не экран).
    Auto,
}

impl FlashMode {
    /// Следующий режим по тапу на значок над кадром: Off → Auto → On → Off (iOS 26,
    /// `docs/ios-camera/behavior.md`).
    pub const fn next_in_cycle(self) -> FlashMode {
        match self {
            FlashMode::Off => FlashMode::Auto,
            FlashMode::Auto => FlashMode::On,
            FlashMode::On => FlashMode::Off,
        }
    }

    /// Цветом «включено» (жёлтый) подсвечивается только `On`: `Auto` — «может сработать», значок
    /// остаётся нейтральным (белым) `[estimate]`, как у `Off`; жёлтый у iOS — «вспышка будет».
    pub const fn is_highlighted(self) -> bool {
        matches!(self, FlashMode::On)
    }
}

/// Снимать ли RAW (DNG) рядом с основным файлом.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RawCapture {
    #[default]
    Off,
    On,
}

impl RawCapture {
    /// Противоположное значение (тап по значку RAW).
    pub const fn toggled(self) -> RawCapture {
        match self {
            RawCapture::Off => RawCapture::On,
            RawCapture::On => RawCapture::Off,
        }
    }
}

/// Таймер затвора. Имена вариантов — «слоты» iOS (3, 5, 10 с); сколько секунд в каждом слоте,
/// решает конфиг ([`crate::config::TimerConfig`]), чтобы длительность не была «магическим» числом.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TimerSetting {
    /// Таймер выключен: снимок сразу.
    #[default]
    Off,
    /// Короткий слот (по умолчанию 3 с).
    Seconds3,
    /// Средний слот (по умолчанию 5 с).
    Seconds5,
    /// Длинный слот (по умолчанию 10 с).
    Seconds10,
}

impl TimerSetting {
    /// Включён ли таймер (значок таймера над кадром виден только тогда, `behavior.md` 2.2).
    pub const fn is_active(self) -> bool {
        !matches!(self, TimerSetting::Off)
    }
}

/// Соотношение сторон кадра (маска видоискателя и кадрирование снимка).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AspectRatio {
    /// 4:3 — родной кадр сенсора, режим по умолчанию.
    #[default]
    FourByThree,
    /// 16:9 — кадрирование по высоте.
    SixteenByNine,
    /// 1:1 — квадрат по короткой стороне.
    Square,
}

impl AspectRatio {
    /// Все соотношения в порядке меню.
    pub const ALL: [AspectRatio; 3] = [
        AspectRatio::FourByThree,
        AspectRatio::SixteenByNine,
        AspectRatio::Square,
    ];

    /// То же соотношение на языке кадрирования сенсора (`camera-contract`).
    pub const fn crop_aspect(self) -> CropAspect {
        match self {
            AspectRatio::FourByThree => CropAspect::FOUR_BY_THREE,
            AspectRatio::SixteenByNine => CropAspect::SIXTEEN_BY_NINE,
            AspectRatio::Square => CropAspect::SQUARE,
        }
    }
}

/// Сетка поверх кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GridSetting {
    #[default]
    Off,
    On,
}

/// Уровень (линия горизонта по наклону телефона).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LevelSetting {
    #[default]
    Off,
    On,
}

/// Все настройки съёмки одним значением.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CaptureSettings {
    pub flash: FlashMode,
    pub raw: RawCapture,
    pub timer: TimerSetting,
    pub aspect: AspectRatio,
    pub grid: GridSetting,
    pub level: LevelSetting,
}
