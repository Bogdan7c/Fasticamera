//! Эталон цветовых приборов Fasticamera на CPU: waveform, RGB parade, vectorscope (решения 183, 188–190).
//!
//! Прибор = форма сетки счётчиков + правило попадания ([`ScopeRule`]) + правило показа ([`render`]).
//! Новый прибор — новая реализация [`ScopeRule`] и вариант [`ResolvedScope`]; [`accumulate`], счётчики и
//! показ не меняются.
//!
//! # Контракт «в форме GPU» (паритет счётчиков БИТ В БИТ)
//! Compute-проход `isp-vulkan` обязан давать те же целые счётчики. Поэтому правило попадания собрано
//! только из операций, одинаковых на любом GPU Vulkan:
//! - целая арифметика `i32`/`u32`; `>>` над знаковым числом арифметический (как `>>` над `int` в GLSL);
//! - f32 `+`, `−`, `×` (OpFAdd/OpFSub/OpFMul округляются точно; в шейдере они помечаются `precise` =
//!   NoContraction: ни слияния в FMA, ни перестановок). На CPU `mul_add` не применяется никогда;
//! - `floor` и сравнения; перевод в целое — только после проверки диапазона;
//! - НЕТ деления f32, `pow`, `log`, `exp`, `sqrt`, `cbrt` из библиотек. Кубический корень OKLab —
//!   [`cube_root::cube_root`]: целочисленное начальное приближение и три шага Ньютона на `×`/`−`;
//! - денормали исключены: ниже `10⁻³⁰` значение зануляется до нелинейных шагов.
//!
//! Все коэффициенты (веса проекций, матрицы, таблица декодирования, масштабы) считаются ОДИН раз на CPU в
//! f64 и округляются один раз; GPU получает ровно эти числа через аксессоры правил. Каждая функция попадания
//! написана так, чтобы переписываться в GLSL один в один, с тем же порядком операций (см. docblock модулей
//! [`projection`], [`trace_rules`], [`vectorscope_ycbcr`], [`oklab_rule`], [`cube_root`]).
//!
//! Битно точны только СЧЁТЧИКИ. Картинка прибора ([`render`]) использует `log2` и деление и сверяется с
//! допуском.
//!
//! # Вход
//! Целые коды 0…1023 показанной картинки ([`ShownCodes`]); округление значений экрана в коды —
//! единственное правило [`quantize_display_value`], общее с GPU.

pub mod codes;
pub mod cube_root;
pub mod graticule;
mod graticule_layout;
pub mod grid;
pub mod oklab_rule;
pub mod projection;
pub mod render;
pub mod render_settings;
pub mod resolved;
pub mod rule;
pub mod sampling;
pub mod settings;
pub mod trace_rules;
pub mod vectorscope_geometry;
pub mod vectorscope_ycbcr;

pub use codes::{HorizontalOrder, MAX_DISPLAY_CODE, ShownCodes, ShownCodesError, quantize_display_value};
pub use graticule::{GraticulePrimitive, MAX_GRATICULE_PRIMITIVES};
pub use grid::{GridError, GridShape, ScopeCounters};
pub use render::{RenderError, ScopePicture, render, trace_intensity};
pub use render_settings::{RenderSettings, RenderSettingsError};
pub use resolved::{ResolveError, ResolvedScope};
pub use rule::{AccumulateError, Hit, SamplePosition, ScopeRule, accumulate, hits_of_code};
pub use sampling::{SampleStride, SampleStrideError};
pub use settings::{ScopeModelSettings, ScopeSettingsError};
