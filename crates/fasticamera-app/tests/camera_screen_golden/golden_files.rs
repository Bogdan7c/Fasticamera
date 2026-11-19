//! Эталоны экрана камеры: хранение, сверка с допуском, перегенерация и PNG для глаз.
//!
//! Размер. Кадр 1080 × 2412 в PAM 16 бит весил бы ~21 МБ на состояние. Поэтому эталон хранится уменьшенным в
//! `DOWNSCALE` раз усреднением блоков (180 × 402) и в 8 бит на канал (PAM `RGB`, ~0,22 МБ). Рендер и все
//! смысловые проверки пикселей идут в полном разрешении; уменьшение — только формат хранения golden.

use std::path::{Path, PathBuf};

use crate::driver::{HEIGHT_PX, WIDTH_PX};
use crate::render_rig::Snapshot;

// Кодек PNG для превью — общий с golden рендера UI (свой кодек без тяжёлых зависимостей, правило проекта).
#[allow(dead_code, reason = "из общего модуля нужен только encode_png")]
#[path = "../../../ui-render-vulkan/tests/support/image.rs"]
mod image;

/// Во сколько раз эталон меньше кадра по каждой оси. 6: 1080 / 6 = 180, 2412 / 6 = 402 — делится нацело.
const DOWNSCALE: u32 = 6;
const GOLDEN_WIDTH: u32 = WIDTH_PX / DOWNSCALE;
const GOLDEN_HEIGHT: u32 = HEIGHT_PX / DOWNSCALE;
/// Допуск на канал: 6/255. Восьмибитное квантование даёт до 0,5/255, остальное — запас на пересборки Mesa
/// (порядок FMA/округления в smoothstep). Заметная ошибка рисунка (сдвиг элемента, не тот цвет, пропавший текст)
/// на блоке 6 × 6 даёт на порядок больше.
const TOLERANCE: f32 = 6.0 / 255.0;
const UPDATE_ENV: &str = "FASTICAMERA_UPDATE_GOLDEN";
const PREVIEW_ENV: &str = "FASTICAMERA_GOLDEN_PREVIEW_DIR";
const TEST_COMMAND: &str = "cargo test -p fasticamera-app --test camera_screen_golden";

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-assets/camera-screen/golden")
}

/// Усреднение блоков `DOWNSCALE × DOWNSCALE` (линейное в закодированных значениях — достаточно для сверки).
fn downscale(snapshot: &Snapshot) -> Vec<[f32; 3]> {
    let mut result = Vec::with_capacity((GOLDEN_WIDTH * GOLDEN_HEIGHT) as usize);
    let block_pixels = (DOWNSCALE * DOWNSCALE) as f32;
    for block_y in 0..GOLDEN_HEIGHT {
        for block_x in 0..GOLDEN_WIDTH {
            let mut sum = [0.0_f32; 3];
            for dy in 0..DOWNSCALE {
                for dx in 0..DOWNSCALE {
                    let pixel = snapshot.at(block_x * DOWNSCALE + dx, block_y * DOWNSCALE + dy);
                    for channel in 0..3 {
                        sum[channel] += pixel[channel];
                    }
                }
            }
            result.push(sum.map(|value| value / block_pixels));
        }
    }
    result
}

/// PAM `RGB` 8 бит, без сжатия.
fn encode_pam_rgb8(pixels: &[[f32; 3]]) -> Vec<u8> {
    let mut bytes =
        format!("P7\nWIDTH {GOLDEN_WIDTH}\nHEIGHT {GOLDEN_HEIGHT}\nDEPTH 3\nMAXVAL 255\nTUPLTYPE RGB\nENDHDR\n")
            .into_bytes();
    for pixel in pixels {
        bytes.extend(
            pixel
                .iter()
                .map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8),
        );
    }
    bytes
}

/// Разбор PAM, записанного `encode_pam_rgb8`. Другой заголовок или размер — `Err` с причиной.
fn decode_pam_rgb8(bytes: &[u8]) -> Result<Vec<[f32; 3]>, String> {
    let header =
        format!("P7\nWIDTH {GOLDEN_WIDTH}\nHEIGHT {GOLDEN_HEIGHT}\nDEPTH 3\nMAXVAL 255\nTUPLTYPE RGB\nENDHDR\n");
    let body = bytes
        .strip_prefix(header.as_bytes())
        .ok_or_else(|| format!("заголовок не совпадает с ожидаемым {GOLDEN_WIDTH}x{GOLDEN_HEIGHT} RGB8"))?;
    let expected_len = (GOLDEN_WIDTH * GOLDEN_HEIGHT * 3) as usize;
    if body.len() != expected_len {
        return Err(format!("тело {} байт, нужно {expected_len}", body.len()));
    }
    Ok(body
        .chunks_exact(3)
        .map(|chunk| [chunk[0], chunk[1], chunk[2]].map(|value| f32::from(value) / 255.0))
        .collect())
}

/// Худшее расхождение по каналам и индекс уменьшенного пикселя, где оно случилось.
fn worst_difference(actual: &[[f32; 3]], expected: &[[f32; 3]]) -> (f32, usize) {
    let mut worst = (0.0_f32, 0_usize);
    for (index, (a, e)) in actual.iter().zip(expected).enumerate() {
        for channel in 0..3 {
            let difference = (a[channel] - e[channel]).abs();
            if difference > worst.0 {
                worst = (difference, index);
            }
        }
    }
    worst
}

/// Пишет PNG-превью полного кадра в `FASTICAMERA_GOLDEN_PREVIEW_DIR` (если задан).
fn write_preview(name: &str, snapshot: &Snapshot) {
    let Ok(directory) = std::env::var(PREVIEW_ENV) else {
        return;
    };
    std::fs::create_dir_all(&directory).expect("каталог превью");
    let png = image::encode_png(WIDTH_PX, HEIGHT_PX, &snapshot.pixels);
    std::fs::write(Path::new(&directory).join(format!("{name}.png")), png).expect("запись превью");
}

/// Идёт ли перегенерация эталонов (`FASTICAMERA_UPDATE_GOLDEN=1`): тогда «сверка» пишет файлы, а не сравнивает.
pub fn is_update_mode() -> bool {
    std::env::var(UPDATE_ENV).as_deref() == Ok("1")
}

/// Записывает (`FASTICAMERA_UPDATE_GOLDEN=1`) или сверяет эталон `name` с допуском.
pub fn check_golden(name: &str, snapshot: &Snapshot) {
    write_preview(name, snapshot);
    let actual = downscale(snapshot);
    let path = golden_dir().join(format!("{name}.pam"));
    if is_update_mode() {
        std::fs::create_dir_all(golden_dir()).expect("каталог эталонов");
        std::fs::write(&path, encode_pam_rgb8(&actual)).expect("запись эталона");
        eprintln!("эталон записан: {}", path.display());
        return;
    }
    let bytes = std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "нет эталона {} ({error}). Сгенерируйте: {UPDATE_ENV}=1 {TEST_COMMAND}",
            path.display()
        )
    });
    let expected =
        decode_pam_rgb8(&bytes).unwrap_or_else(|error| panic!("эталон {} повреждён: {error}", path.display()));
    let (worst, index) = worst_difference(&actual, &expected);
    assert!(
        worst <= TOLERANCE,
        "{name}: расхождение {worst:.4} (допуск {TOLERANCE:.4}) в блоке ({}, {}) уменьшенного кадра. \
         Если изменение намеренное: {UPDATE_ENV}=1 {TEST_COMMAND}",
        index as u32 % GOLDEN_WIDTH,
        index as u32 / GOLDEN_WIDTH
    );
}
