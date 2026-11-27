//! Вход кадра приборов и ёмкость ресурсов: проверка показанной картинки, push-константы прохода накопления и размеры
//! наибольших сеток настроек (по ним при создании выделяются буфер счётчиков и промежуточный слой показа).
//!
//! Чистые функции без GPU-объектов; владелец — [`super::ColorScopes`]. Вынесены из `color_scopes.rs`, чтобы модуль
//! владельца не рос за предел размера (правило модулей 700–800 строк).

use ash::vk;
use gpu_vulkan::source_window::HorizontalMirror;
use scope_model::codes::{MAX_SHOWN_PIXELS, MAX_SHOWN_SIDE};
use scope_model::grid::{MAX_GRID_CELLS, MAX_GRID_SIDE};
use scope_model::vectorscope_geometry::MAX_VECTORSCOPE_SIZE;
use scope_model::{SampleCount, ScopeModelSettings};

use super::accumulators::{ACCUMULATE_BINDINGS, SAMPLING_PUSH_BYTES};
use super::error::ScopeError;
use super::frame::ScopeInput;
use super::parameter_blocks::MAX_PLANES;

/// Проверенный вход: размер содержимого и зеркало.
pub(super) struct CheckedInput {
    /// Размер содержимого окна.
    pub content: vk::Extent2D,
    /// Зеркальна ли картинка на экране.
    pub mirrored: bool,
}

/// Проверяет вход: формат, окно внутри картинки, предел размера показанной картинки.
pub(super) fn check_input(input: &ScopeInput) -> Result<CheckedInput, ScopeError> {
    match input.view.format {
        vk::Format::A2B10G10R10_UNORM_PACK32 | vk::Format::R16G16B16A16_SFLOAT => {}
        format => return Err(ScopeError::UnsupportedInputFormat { format }),
    }
    input
        .window
        .validate_within(input.view.extent)
        .map_err(ScopeError::InputWindow)?;
    let content = input.window.content();
    if content.width > MAX_SHOWN_SIDE
        || content.height > MAX_SHOWN_SIDE
        || u64::from(content.width) * u64::from(content.height) > MAX_SHOWN_PIXELS
    {
        return Err(ScopeError::InputTooLarge {
            width: content.width,
            height: content.height,
        });
    }
    Ok(CheckedInput {
        content,
        mirrored: input.window.mirror() == HorizontalMirror::On,
    })
}

/// Push-константы прохода накопления (`Sampling` в `scope_accumulate_common.glsl`, 9 слов): окно, шаг и смещение
/// первого отсчёта по x и по y (свои у каждой оси, решение 329), число отсчётов по осям, зеркало. Шейдер берёт
/// координаты отсчётов так же, как `SampleStride::coordinate` эталона: `смещение + номер · шаг`.
pub(super) fn sampling_push_bytes(input: &CheckedInput, samples: SampleCount) -> [u8; SAMPLING_PUSH_BYTES as usize] {
    let steps = samples.steps();
    let words = [
        input.content.width,
        input.content.height,
        steps.across().get(),
        steps.down().get(),
        steps.across().offset(),
        steps.down().offset(),
        samples.across(),
        samples.down(),
        u32::from(input.mirrored),
    ];
    let mut bytes = [0_u8; SAMPLING_PUSH_BYTES as usize];
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(words) {
        chunk.copy_from_slice(&word.to_ne_bytes());
    }
    bytes
}

/// Ячеек в наибольшей сетке настроек: waveform/parade (`потолок столбцов × уровни × 3`) или вектороскоп (`сторона²`).
pub(super) fn largest_grid_cells(settings: &ScopeModelSettings) -> usize {
    let trace = settings.trace_max_columns as usize * settings.trace_levels as usize * MAX_PLANES;
    let vectorscope = settings.vectorscope_size as usize * settings.vectorscope_size as usize;
    trace.max(vectorscope)
}

/// Строк в наибольшей сетке настроек: уровни waveform/parade или сторона вектороскопа.
pub(super) fn largest_grid_rows(settings: &ScopeModelSettings) -> u32 {
    settings.trace_levels.max(settings.vectorscope_size)
}

// Пределы эталона, на которые опирается шейдер, закреплены на этапе компиляции: если эталон их увеличит,
// сборка остановится, а не переполнит `uint` шейдера молча.
const _: () = {
    // x · columns < 2^32 (положение отсчёта в waveform/parade).
    assert!(MAX_SHOWN_SIDE as u64 * MAX_GRID_SIDE as u64 <= u32::MAX as u64);
    // Индекс ячейки помещается в uint.
    assert!(MAX_GRID_CELLS <= u32::MAX as u64);
    assert!(MAX_VECTORSCOPE_SIZE <= MAX_GRID_SIDE);
    assert!(ACCUMULATE_BINDINGS.len() == 3);
};
