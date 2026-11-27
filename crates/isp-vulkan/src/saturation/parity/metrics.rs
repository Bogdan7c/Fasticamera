//! Числа сверки картинки GPU с эталоном: max |Δ|, доля пикселей хуже 1 LSB и ошибка математики сверх шага хранения.

use gpu_vulkan::storage_image::StorageImageFormat;

use super::allowance::ONE_LSB;
use super::ruler::ParityError;

/// Паритет GPU против эталона (числа, без вердикта).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Parity {
    /// max |Δ| по всем каналам.
    pub max_abs: f32,
    /// Доля пикселей, где хоть один канал отличается больше 1 LSB 10-битного выхода.
    pub over_one_lsb: f64,
    /// max(|Δ| − шаг хранения) — то, что НЕ объясняется форматом картинки: ошибка математики GPU.
    /// Шаг хранения — см. [`storage_step`].
    pub math_excess: f32,
}

/// Шаг хранения значения `value` в формате картинки: fp16 — один ULP fp16 у значения эталона (драйвер вправе
/// округлять к нулю при записи в картинку), 10 бит UNORM — полшага 1/2046 (округление к ближайшему).
pub fn storage_step(format: StorageImageFormat, value: f32) -> f32 {
    match format {
        StorageImageFormat::Rgba16Float => {
            let magnitude = value.abs().max(f32::from_bits(0x3880_0000)); // 2^-14: нормальные fp16
            // ULP fp16 = 2^(экспонента − 10).
            2.0_f32.powi(magnitude.log2().floor() as i32 - 10)
        }
        StorageImageFormat::A2B10G10R10Unorm => 0.5 / 1023.0,
    }
}

/// Сверяет пиксели GPU (`gpu_pixels`, RGBA, построчно) с эталоном `reference` (RGB, те же размеры).
///
/// Размеры обязаны совпасть: иначе числа были бы про разные картинки, и это явная ошибка, а не «паритет плохой».
pub fn compare_with_reference(
    gpu_pixels: &[[f32; 4]],
    gpu_size: (u32, u32),
    format: StorageImageFormat,
    reference: &[[f32; 3]],
    reference_size: (u32, u32),
) -> Result<Parity, ParityError> {
    if gpu_size != reference_size || gpu_pixels.len() != reference.len() {
        return Err(ParityError::SizeMismatch {
            gpu: gpu_size,
            reference: reference_size,
        });
    }
    let mut max_abs = 0.0_f32;
    let mut math_excess = 0.0_f32;
    let mut over = 0_usize;
    for (gpu_pixel, reference_pixel) in gpu_pixels.iter().zip(reference) {
        let mut worst = 0.0_f32;
        for channel in 0..3 {
            let difference = (gpu_pixel[channel] - reference_pixel[channel]).abs();
            worst = worst.max(difference);
            let step = storage_step(format, reference_pixel[channel]);
            math_excess = math_excess.max(difference - step);
        }
        max_abs = max_abs.max(worst);
        if worst > ONE_LSB {
            over += 1;
        }
    }
    Ok(Parity {
        max_abs,
        over_one_lsb: over as f64 / reference.len().max(1) as f64,
        math_excess: math_excess.max(0.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_bit_step_is_half_a_code_and_fp16_step_is_one_ulp_of_the_reference_value() {
        assert_eq!(storage_step(StorageImageFormat::A2B10G10R10Unorm, 0.7), 0.5 / 1023.0);
        // 0,5 лежит в [0,5; 1): ULP fp16 = 2^(−1−10).
        assert_eq!(storage_step(StorageImageFormat::Rgba16Float, 0.5), 2.0_f32.powi(-11));
        // Ниже нормальных fp16 шаг не мельчает (нижняя граница 2^-14).
        assert_eq!(storage_step(StorageImageFormat::Rgba16Float, 0.0), 2.0_f32.powi(-24));
    }

    #[test]
    fn difference_within_the_storage_step_is_no_math_excess_and_a_larger_one_is_measured() {
        let reference = [[0.5_f32, 0.5, 0.5]];
        let step = storage_step(StorageImageFormat::A2B10G10R10Unorm, 0.5);
        let within = [[0.5 + step, 0.5, 0.5, 1.0]];
        let parity = compare_with_reference(
            &within,
            (1, 1),
            StorageImageFormat::A2B10G10R10Unorm,
            &reference,
            (1, 1),
        )
        .unwrap();
        assert_eq!(
            parity.math_excess, 0.0,
            "разница ровно в шаг хранения — не ошибка математики"
        );
        assert!((parity.max_abs - step).abs() < 1e-9);

        let beyond = [[0.5 + step + 3.0e-5, 0.5, 0.5, 1.0]];
        let parity = compare_with_reference(
            &beyond,
            (1, 1),
            StorageImageFormat::A2B10G10R10Unorm,
            &reference,
            (1, 1),
        )
        .unwrap();
        assert!((parity.math_excess - 3.0e-5).abs() < 1e-7, "{parity:?}");
    }

    #[test]
    fn share_over_one_lsb_counts_pixels_not_channels() {
        let reference = [[0.2_f32; 3]; 4];
        let mut gpu = [[0.2_f32, 0.2, 0.2, 1.0]; 4];
        gpu[0][0] += 3.0 * ONE_LSB;
        gpu[0][1] += 3.0 * ONE_LSB;
        let parity = compare_with_reference(&gpu, (2, 2), StorageImageFormat::Rgba16Float, &reference, (2, 2)).unwrap();
        assert_eq!(parity.over_one_lsb, 0.25);
    }

    #[test]
    fn different_sizes_are_an_error_not_a_bad_parity() {
        let result = compare_with_reference(
            &[[0.0; 4]],
            (1, 1),
            StorageImageFormat::Rgba16Float,
            &[[0.0; 3]; 2],
            (2, 1),
        );
        assert!(matches!(result, Err(ParityError::SizeMismatch { .. })), "{result:?}");
    }
}
