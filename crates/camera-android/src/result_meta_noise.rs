//! Разбор `ACAMERA_SENSOR_NOISE_PROFILE` из результата запроса в [`NoiseProfile`].
//!
//! Запись — `double[2 × число каналов CFA]`: пары `(S, O)` по каналам в порядке CFA
//! (документация NDK: «S — усиление сенсора, O — шум чтения», модель `N(x) = sqrt(S·x + O)`,
//! `x` нормирован в `[0, 1]`). Наш контракт хранит ровно четыре канала (Bayer 2×2), поэтому
//! запись обязана содержать ровно 8 чисел; монохромная камера (одна пара) сюда не относится.
//!
//! Модуль отдельный от `result_meta.rs`, чтобы не раздувать его, и потому что `entry.rs`
//! читает только `float`/`int`/`rational`, а тут нужен `double`.

use camera_contract::frame_passport::{NoiseModelCoefficients, NoiseProfile};

use crate::entry::{EntryKind, EntryLookup, EntryProblem, EntryValue, MetadataSource, Read};
use crate::tags;

/// Сколько чисел в записи для четырёх каналов CFA: пара `(S, O)` на канал.
const NOISE_PROFILE_VALUES: usize = 8;

/// Читает профиль шума. `Ok(None)` — HAL ключ не отдал; `Err` — ключ есть, но непригоден
/// (не `double`, не 8 чисел, нечисловое значение, отрицательный `S`).
pub(super) fn read_noise_profile(source: &dyn MetadataSource) -> Read<NoiseProfile> {
    let values = match source.entry(tags::NOISE_PROFILE) {
        EntryLookup::Absent => return Ok(None),
        EntryLookup::Unreadable { status } => return Err(EntryProblem::Unreadable { status }),
        EntryLookup::Present(EntryValue::Doubles(values)) => values,
        EntryLookup::Present(other) => {
            return Err(EntryProblem::WrongType {
                expected: EntryKind::Double,
                actual: other.kind(),
            });
        }
    };
    if values.len() < NOISE_PROFILE_VALUES {
        return Err(EntryProblem::TooFewElements {
            expected: NOISE_PROFILE_VALUES,
            actual: values.len(),
        });
    }
    // Лишние числа означают другое число каналов: молча обрезать нельзя, профиль был бы чужим.
    if values.len() != NOISE_PROFILE_VALUES {
        return Err(EntryProblem::OutOfRange);
    }
    let mut channels = [NoiseModelCoefficients {
        scale: 0.0,
        offset: 0.0,
    }; 4];
    for (channel, pair) in channels.iter_mut().zip(values.chunks_exact(2)) {
        let (scale, offset) = (pair[0], pair[1]);
        // `S` — дисперсия на единицу сигнала: отрицательной или нечисловой быть не может;
        // `O` может быть небольшим отрицательным (подгонка модели), но обязан быть конечным.
        if !scale.is_finite() || scale < 0.0 || !offset.is_finite() {
            return Err(EntryProblem::OutOfRange);
        }
        *channel = NoiseModelCoefficients { scale, offset };
    }
    Ok(Some(NoiseProfile(channels)))
}
