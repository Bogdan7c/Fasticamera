//! Цветовые приборы на GPU (сессия 14h): waveform, RGB parade, вектороскоп (Y'CbCr и OKLab) по выходу
//! видоискателя. Эталон на CPU — crate `scope-model`; счётчики совпадают с ним БИТ В БИТ, картинка — с
//! допуском fp16 (решения 183, 188–190).
//!
//! # Владение и швы
//! * [`ColorScopes`] владеет пайплайнами всех приборов, буфером счётчиков, параметрами по слотам и картинкой
//!   прибора; создаётся при открытии приборов, бросается при закрытии (закрытые приборы ничего не стоят);
//! * прибор — ДАННЫЕ ([`GpuScopeRule`]): идентификатор шейдера накопления, форма сетки, слова параметров,
//!   способ рисования следа с данными (`TraceStyle`), примитивы сетки. Для приборов `scope-model` перевод — `impl GpuScopeRule for ResolvedScope`;
//!   новый прибор = своя реализация + свой шейдер на общей части `scope_accumulate_common.glsl`, зарегистрированный
//!   в [`ColorScopes::with_extra_accumulators`]; соседние приборы и проходы не правятся;
//! * вход — [`ScopeInput`] (выход ISP + окно содержимого и зеркала), выход — [`RecordedScope`] (картинка,
//!   готовая к чтению фрагментным шейдером).
//!
//! # Допущения побитового паритета счётчиков (ревью сессии 14h)
//! Счётчики GPU совпадают с эталоном БИТ В БИТ только пока выполнены два допущения об арифметике f32 шейдера. Они не
//! проверяются кодом и не навязываются устройству (режимы `RoundingModeRTE`/`DenormPreserve` шейдерам НЕ выставляются):
//! 1. округление f32 по умолчанию — к ближайшему чётному (RTE). Спецификация Vulkan умолчание не фиксирует;
//!    `VkPhysicalDeviceFloatControlsProperties::shaderRoundingModeRTEFloat32` лишь говорит, что RTE можно потребовать.
//!    Драйверы настольных и мобильных GPU по умолчанию округляют к ближайшему чётному, на это и опираемся;
//! 2. в арифметике правил нет денормализованных f32: коды экрана декодируются таблицей из 1024 значений (наименьшее
//!    ненулевое около 7,6·10⁻⁵ — на 33 порядка выше наименьшего нормализованного f32), а кубический корень OKLab
//!    возвращает 0 для |x| < 1e-30 и NaN (`CUBE_ROOT_ZERO_THRESHOLD` эталона и шейдера), так что денормалей не
//!    возникает и сброс денормалей в ноль (FTZ) ничего не меняет.
//!
//! Факт устройства (`shaderRoundingModeRTEFloat32`, `shaderDenormPreserveFloat32`, `shaderDenormFlushToZeroFloat32`)
//! читает `gpu-vulkan` (`GpuCapabilities::float32_controls`), а render-loop показывает его в диагностике приборов
//! (`ScopeDiagnostics::float32_controls`): расхождение паритета на новом устройстве объясняется, а не угадывается.
//!
//! Шейдеры: `scope_accumulate_common.glsl` + правила `scope_accumulate_integer_rule.glsl` (waveform, parade, Y'CbCr) и
//! `scope_accumulate_oklab_rule.glsl`; шейдеры накопления `scope_accumulate_integer.comp`, `scope_accumulate_oklab.comp`
//! и их варианты `*_merged.comp` со слиянием одинаковых ячеек в subgroup (вектороскопы: соседние пиксели падают в одни
//! ячейки, и `atomicAdd` в них упирается в конкуренцию; без баллота subgroup на устройстве берётся запасной шейдер
//! без слияния, [`AccumulatorId::fallback`]); `scope_render.comp` (показ с отбором примитивов сетки по плитке).

mod accumulators;
mod color_scopes;
mod error;
mod frame;
mod frame_input;
mod output_digest;
mod parameter_blocks;
mod picture;
mod resample_pass;
mod resolved_rule;
mod rule;
#[cfg(test)]
mod shader_literals_tests;
#[cfg(feature = "test-support")]
mod test_support;

pub use self::accumulators::{AccumulatorShader, merged_accumulators, standard_accumulators};
pub use self::color_scopes::{ColorScopes, ColorScopesConfig};
pub use self::error::ScopeError;
pub use self::frame::{RecordedScope, ScopeFrame, ScopeInput, ScopePassCounters, ScopeSlotReport};
pub use self::output_digest::ScopeOutputDigest;
pub use self::picture::PictureChange;
pub use self::rule::{
    AccumulateParameters, AccumulatorId, DECODE_TABLE_LEN, GpuScopeRule, RULE_WORD_CAPACITY, RuleWords,
};
#[cfg(feature = "test-support")]
pub use self::test_support::PictureReadback;
