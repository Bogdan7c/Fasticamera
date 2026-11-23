//! Профиль выхода (решение 184): последний шаг цепочки цвета, вынесенный в отдельную сменную часть.
//!
//! Цепочка 14a (решение 176): `… → основа тона → вид → [LUT] → W → цвета выхода → мягкое сжатие охвата → кодирование`.
//! Рабочее пространство `W` и формирование картинки (основа тона, вид) ОБЩИЕ для всех выходов; профиль выхода
//! описывает только то, что идёт ПОСЛЕ формирования: в каких основных цветах, какой кривой передачи и с какой
//! разрядностью отдавать результат, и какого «рендеринга» от картинки ждёт приёмник.
//!
//! Сейчас поддержаны два профиля: SDR sRGB (снимок) и SDR Display P3 (видоискатель на экране телефона). Остальные
//! варианты ([`OutputRendering::SceneLog`], [`OutputRendering::Hdr`], 10 бит) — ЗАДЕЛЫ под план `video`: тип
//! уже их знает, но [`OutputProfile::check_supported`] честно отвергает их различимой ошибкой
//! [`UnsupportedOutputProfile`], а `IspParams::resolve` и GPU-пути не дают им попасть в расчёт.
//!
//! # Где профиль влияет
//! * охват выхода — [`OutputProfile::gamut`] (матрица `W → цвета выхода`, выходное сжатие охвата);
//! * кодирование — [`OutputTransfer::encode`] (CPU, единственная точка выбора кривой) и
//!   `color_tail_encode_output` в `shaders/color_tail.glsl` (GPU, зеркало);
//! * поддержка — [`OutputProfile::check_supported`] (разрешение параметров и проверки GPU-путей).
//!
//! # Как добавить профиль выхода (например, Log или HDR из плана `video`)
//! 1. **Охват.** Log и HDR работают в основных цветах Rec. 2020. Добавьте вариант в [`crate::image::DisplayGamut`]
//!    (+ `Primaries::REC_2020` в `color::chromaticity`; рабочее пространство `W` у AgX уже Rec. 2020, поэтому
//!    матрица `W → выход` для него единичная) и конструктор профиля вместо `with_rendering` в тестах.
//! 2. **Кривая.** Новый вариант [`OutputTransfer`] (Log-кривая камеры, PQ или HLG) с ветками в
//!    [`OutputTransfer::encode`] и [`OutputTransfer::decode`] — компилятор укажет все `match`. На GPU — функция в
//!    `color_tail.glsl` и выбор кривой в `color_tail_encode_output` (по числу из uniform или константе
//!    специализации: смена профиля — смена варианта пайплайна, а не пересборка шейдера на лету). Паритет GPU с
//!    CPU — тестом по сетке значений, как у sRGB.
//! 3. **Рендеринг.** Для `SceneLog` картинка НЕ сжимается основой тона в 0…1 экрана (кривая сохраняет динамический
//!    диапазон сцены), для `Hdr` — сжимается к пику яркости экрана (нити, а не 0…1). Это решение формирования, а не
//!    кодирования: оно меняет ветку основы тона, поэтому требует отдельной записи решения владельца.
//! 4. **Разрядность.** 10 бит — это формат цели (кодер видео, буфер кадра), а не формула: профиль лишь несёт её,
//!    а кодер обязан принимать указанную разрядность и тегировать поток (основные цвета, кривая, диапазон).
//! 5. **Разрешение поддержки.** Уберите соответствующую ветку из [`OutputProfile::check_supported`] только когда
//!    CPU-эталон, GPU и кодер вместе умеют профиль; до этого он отвергается.
//! 6. **Тесты.** Поддержанный профиль больше не отвергается; сетка значений кривой совпадает между CPU и GPU; кадр
//!    в sRGB-профиле не меняется бит в бит.

use crate::image::DisplayGamut;
use crate::transfer::{srgb_decode, srgb_encode};

/// Какого «рендеринга» от картинки ждёт приёмник.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputRendering {
    /// Картинка для SDR-экрана: основа тона сжимает сцену в 0…1 (снимок, видоискатель). Поддержан.
    DisplayReferredSdr,
    /// Log-запись: динамический диапазон сцены сохраняется кривой камеры, без «вида» экрана. ЗАДЕЛ (план `video`).
    SceneLog,
    /// HDR (PQ или HLG): сжатие к пику яркости экрана. ЗАДЕЛ (план `video`).
    Hdr,
}

/// Кривая передачи (кодирование света в сигнал 0…1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputTransfer {
    /// sRGB (IEC 61966-2-1); у Display P3 та же кривая (см. [`crate::transfer`]).
    Srgb,
}

impl OutputTransfer {
    /// Свет 0…1 → сигнал 0…1 (вход обрезается в 0…1, NaN → 0). ЕДИНСТВЕННАЯ точка выбора кривой на CPU.
    pub fn encode(self, linear: f32) -> f32 {
        match self {
            Self::Srgb => srgb_encode(linear),
        }
    }

    /// Обратная кривая (EOTF): сигнал 0…1 → свет; нужна метрикам и сверке с чтением экрана.
    pub fn decode(self, encoded: f32) -> f32 {
        match self {
            Self::Srgb => srgb_decode(encoded),
        }
    }
}

/// Разрядность сигнала на выходе.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputBitDepth {
    /// 8 бит на канал (снимок JPEG/HEIF, кадр экрана). Поддержана.
    Eight,
    /// 10 бит на канал (видео HDR/Log). ЗАДЕЛ (план `video`): цель 10 бит появится вместе с кодером.
    Ten,
}

/// Профиль выхода не поддержан: какая именно часть профиля ещё не умеет считаться (различимо для вызывающего).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UnsupportedOutputProfile {
    /// Рендеринг (Log, HDR) ещё не реализован.
    #[error("профиль выхода не поддерживается: рендеринг {0:?} ещё не реализован (план video)")]
    Rendering(OutputRendering),
    /// Разрядность ещё не реализована.
    #[error("профиль выхода не поддерживается: разрядность {0:?} ещё не реализована (план video)")]
    BitDepth(OutputBitDepth),
}

/// Профиль выхода: рендеринг, основные цвета, кривая передачи и разрядность. Поля закрыты: профиль собирается
/// конструкторами ([`Self::srgb`], [`Self::display_p3`], [`Self::sdr`]) и «заделами» ([`Self::with_rendering`],
/// [`Self::with_bit_depth`]), а читается акцессорами.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutputProfile {
    rendering: OutputRendering,
    primaries: DisplayGamut,
    transfer: OutputTransfer,
    bit_depth: OutputBitDepth,
}

impl OutputProfile {
    /// SDR sRGB, 8 бит: профиль снимка.
    pub const fn srgb() -> Self {
        Self::sdr(DisplayGamut::Srgb)
    }

    /// SDR Display P3, 8 бит: профиль видоискателя на экране телефона.
    pub const fn display_p3() -> Self {
        Self::sdr(DisplayGamut::DisplayP3)
    }

    /// SDR с основными цветами `gamut`, кривой sRGB (одна для обеих гамм) и 8 битами.
    pub const fn sdr(gamut: DisplayGamut) -> Self {
        Self {
            rendering: OutputRendering::DisplayReferredSdr,
            primaries: gamut,
            transfer: OutputTransfer::Srgb,
            bit_depth: OutputBitDepth::Eight,
        }
    }

    /// Тот же профиль с другим рендерингом (заделы Log/HDR; такой профиль [`Self::check_supported`] отвергает).
    #[must_use]
    pub const fn with_rendering(self, rendering: OutputRendering) -> Self {
        Self { rendering, ..self }
    }

    /// Тот же профиль с другой разрядностью (задел 10 бит; такой профиль [`Self::check_supported`] отвергает).
    #[must_use]
    pub const fn with_bit_depth(self, bit_depth: OutputBitDepth) -> Self {
        Self { bit_depth, ..self }
    }

    /// Какого рендеринга ждёт приёмник.
    pub const fn rendering(&self) -> OutputRendering {
        self.rendering
    }

    /// Основные цвета выхода (белая точка D65): из них выводятся матрица `W → выход` и выходное сжатие охвата.
    pub const fn gamut(&self) -> DisplayGamut {
        self.primaries
    }

    /// Кривая передачи выхода.
    pub const fn transfer(&self) -> OutputTransfer {
        self.transfer
    }

    /// Разрядность сигнала.
    pub const fn bit_depth(&self) -> OutputBitDepth {
        self.bit_depth
    }

    /// Умеет ли расчёт этот профиль (CPU-эталон и GPU-пути). Ветки явные, без `_ =>`: новый вариант не пройдёт молча.
    pub fn check_supported(&self) -> Result<(), UnsupportedOutputProfile> {
        match self.rendering {
            OutputRendering::DisplayReferredSdr => {}
            OutputRendering::SceneLog | OutputRendering::Hdr => {
                return Err(UnsupportedOutputProfile::Rendering(self.rendering));
            }
        }
        match self.bit_depth {
            OutputBitDepth::Eight => {}
            OutputBitDepth::Ten => return Err(UnsupportedOutputProfile::BitDepth(self.bit_depth)),
        }
        match self.transfer {
            OutputTransfer::Srgb => Ok(()),
        }
    }
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
