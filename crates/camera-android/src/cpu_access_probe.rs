//! Отладочная проба «0 копий»: доказательство того, что счётчик CPU-копий потока живой.
//!
//! Конвейер RAW устроен так, что пиксели не должны проходить через CPU (инвариант 1 README
//! сессии 05). Счётчик `cpu_copies` потока, который всегда показывает 0, ничего не доказывает:
//! может быть, он просто не подключён. Проба нарочно делает настоящую CPU-копию (читает первую
//! строку буфера) и сообщает о ней, после чего счётчик **обязан** вырасти.
//!
//! Проба — рубильник на процесс: по умолчанию выключена, включает её только отладочный сценарий
//! приложения. Читается дважды: при создании reader-а (нужен usage `CPU_READ_RARELY`, иначе
//! `AHardwareBuffer_lock` откажет) и на каждый кадр.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use camera_contract::raw_format::RawFormat;

/// Режим пробы.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuProbeMode {
    /// Проба выключена: пиксели CPU не читает никто (рабочий режим).
    Disarmed,
    /// Каждый кадр: `lock` → копия первой строки в `Vec` → `unlock` → `note_cpu_copy`.
    CopyFirstRow,
}

impl CpuProbeMode {
    const fn to_raw(self) -> u8 {
        match self {
            Self::Disarmed => 0,
            Self::CopyFirstRow => 1,
        }
    }

    fn from_raw(raw: u8) -> Self {
        // Любое неизвестное значение трактуется как «выключено»: безопасная сторона.
        if raw == Self::CopyFirstRow.to_raw() {
            Self::CopyFirstRow
        } else {
            Self::Disarmed
        }
    }
}

/// Общая ручка пробы: клоны делят одно состояние (приложение, бэкенд, слушатель reader-а).
#[derive(Debug, Clone)]
pub struct CpuAccessProbe {
    mode: Arc<AtomicU8>,
}

impl Default for CpuAccessProbe {
    fn default() -> Self {
        Self::disarmed()
    }
}

impl CpuAccessProbe {
    /// Выключенная проба (рабочее состояние по умолчанию).
    pub fn disarmed() -> Self {
        Self {
            mode: Arc::new(AtomicU8::new(CpuProbeMode::Disarmed.to_raw())),
        }
    }

    /// Включает копирование первой строки каждого кадра. Reader-ы, созданные **до** вызова,
    /// не имеют usage `CPU_READ_RARELY` и дадут сбои `lock` (они считаются, не молчат): включайте
    /// пробу до `configure_session`.
    pub fn arm_copy_first_row(&self) {
        self.mode.store(CpuProbeMode::CopyFirstRow.to_raw(), Ordering::Release);
    }

    /// Выключает пробу.
    pub fn disarm(&self) {
        self.mode.store(CpuProbeMode::Disarmed.to_raw(), Ordering::Release);
    }

    pub fn mode(&self) -> CpuProbeMode {
        CpuProbeMode::from_raw(self.mode.load(Ordering::Acquire))
    }

    pub fn is_armed(&self) -> bool {
        self.mode() != CpuProbeMode::Disarmed
    }
}

/// Сколько байт занимает первая строка буфера.
///
/// `stride` из `AHardwareBuffer_describe` у RAW10 измеряется в **байтах** (факт 00 C4), у RAW16 —
/// в пикселях по 2 байта. Результат ограничен `max_bytes`: проба не должна читать за пределами
/// буфера, даже если драйвер отдал странный stride.
pub fn first_row_bytes(format: RawFormat, stride: u32, max_bytes: usize) -> usize {
    let bytes_per_stride_unit: usize = match format {
        RawFormat::Raw10 => 1,
        RawFormat::Raw16 => 2,
    };
    usize::try_from(stride)
        .unwrap_or(usize::MAX)
        .saturating_mul(bytes_per_stride_unit)
        .min(max_bytes)
}

#[cfg(test)]
#[path = "cpu_access_probe_tests.rs"]
mod tests;
