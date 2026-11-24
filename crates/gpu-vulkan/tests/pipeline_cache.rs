//! Кэш пайплайнов контекста (I6-02/I6-03, сессия 16a): файл с нашей обёрткой сохраняется, загружается в новый контекст,
//! а повреждённые, чужие и слишком большие файлы не ломают запуск и не доходят до драйвера. Реальный GPU (RADV), полная
//! validation. Обрезки и порча проверяются НАШЕЙ обёрткой: на RADV это ничего не говорит о драйвере Adreno.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod common;

use common::assert_no_validation_errors;
use gpu_vulkan::compute::{BindingKind, ComputePipeline, ComputePipelineDesc};
use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::pipeline_cache::{
    FILE_HEADER_BYTES, FORMAT_VERSION, FileDamage, PipelineCacheOrigin, seal_cache_file, unseal_cache_file,
};
use gpu_vulkan::shaders::blobs;
use gpu_vulkan::validation::ValidationMode;

/// Контекст с заданным файлом кэша и пределом или `None` при осознанном пропуске GPU-тестов.
fn context_with_file(file: Vec<u8>, max_bytes: usize) -> Option<GpuContext> {
    let mut config = GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::FULL);
    config.pipeline_cache_data = file;
    config.pipeline_cache_max_bytes = max_bytes;
    match GpuContext::new(&config) {
        Ok(context) => Some(context),
        Err(error) => {
            if std::env::var("FASTICAMERA_SKIP_GPU_TESTS").as_deref() == Ok("1") {
                eprintln!("ПРОПУСК GPU-теста: {error}");
                None
            } else {
                panic!("GPU недоступен: {error}");
            }
        }
    }
}

fn context_with(file: Vec<u8>) -> Option<GpuContext> {
    context_with_file(file, usize::MAX)
}

/// Создаёт пайплайн из тестового шейдера: он обязан появиться при любом состоянии кэша.
fn create_test_pipeline(context: &GpuContext) -> ComputePipeline {
    ComputePipeline::new(
        context,
        &ComputePipelineDesc {
            label: "pipeline-cache-test",
            spirv: blobs::KIT_TEST_SAMPLE_COMP,
            bindings: &[BindingKind::CombinedImageSampler, BindingKind::ReadWriteStorageBuffer],
            push_constant_bytes: 8,
            specialization: &[],
            frame_slots: 1,
        },
    )
    .expect("пайплайн создаётся при любом состоянии кэша")
}

/// Настоящий файл кэша с одним пайплайном.
fn real_file() -> Option<Vec<u8>> {
    let donor = common::gpu_context_or_skip()?;
    let _pipeline = create_test_pipeline(&donor);
    Some(donor.pipeline_cache_data().unwrap())
}

#[test]
fn a_sealed_file_grows_after_creating_a_pipeline_and_a_new_context_really_uses_it() {
    let Some(first) = common::gpu_context_or_skip() else {
        return;
    };
    assert_eq!(first.pipeline_cache_origin(), PipelineCacheOrigin::Empty);
    let empty_size = first.pipeline_cache_initial_file_bytes();
    let _pipeline = create_test_pipeline(&first);
    let saved = first.pipeline_cache_data().unwrap();
    assert!(
        saved.len() > empty_size,
        "кэш с пайплайном больше пустого: {} против {empty_size}",
        saved.len()
    );
    assert!(unseal_cache_file(&saved).is_ok(), "файл запечатан нашей обёрткой");
    assert_no_validation_errors(&first);

    let Some(second) = context_with(saved.clone()) else {
        return;
    };
    assert!(matches!(
        second.pipeline_cache_origin(),
        PipelineCacheOrigin::Loaded { .. }
    ));
    // «Действительно использует»: сразу после создания драйвер держит данные, а не пустой кэш.
    assert!(
        second.pipeline_cache_initial_file_bytes() > empty_size
            && second.pipeline_cache_initial_file_bytes() * 2 >= saved.len(),
        "после загрузки кэш содержит данные: {} при файле {}",
        second.pipeline_cache_initial_file_bytes(),
        saved.len()
    );
    let _pipeline = create_test_pipeline(&second);
    assert_no_validation_errors(&second);
}

#[test]
fn every_kind_of_file_damage_is_caught_before_the_driver_and_startup_survives() {
    let Some(real) = real_file() else { return };
    let mut bit_flipped = real.clone();
    let middle = FILE_HEADER_BYTES + (real.len() - FILE_HEADER_BYTES) / 2;
    bit_flipped[middle] ^= 0x10;
    let mut with_tail = real.clone();
    with_tail.extend_from_slice(&[0xCD; 9]);
    let mut foreign_magic = real.clone();
    foreign_magic[0] ^= 0xFF;
    let mut old_version = real.clone();
    old_version[8..12].copy_from_slice(&(FORMAT_VERSION - 1).to_le_bytes());
    let cases: Vec<(&str, Vec<u8>, FileDamage)> = vec![
        (
            "обрезок в заголовке",
            real[..FILE_HEADER_BYTES - 1].to_vec(),
            FileDamage::TooShort,
        ),
        (
            "обрезок: ровно заголовок",
            real[..FILE_HEADER_BYTES].to_vec(),
            FileDamage::LengthMismatch,
        ),
        (
            "обрезок: половина",
            real[..real.len() / 2].to_vec(),
            FileDamage::LengthMismatch,
        ),
        (
            "обрезок: на байт короче",
            real[..real.len() - 1].to_vec(),
            FileDamage::LengthMismatch,
        ),
        ("перевёрнутый бит в теле", bit_flipped, FileDamage::ChecksumMismatch),
        ("хвост мусора", with_tail, FileDamage::LengthMismatch),
        ("чужое магическое число", foreign_magic, FileDamage::WrongMagic),
        (
            "старая версия формата",
            old_version,
            FileDamage::WrongVersion {
                found: FORMAT_VERSION - 1,
            },
        ),
        (
            "сырые данные без обёртки",
            unseal_cache_file(&real).unwrap().to_vec(),
            FileDamage::WrongMagic,
        ),
    ];
    for (name, file, expected) in cases {
        let Some(context) = context_with(file) else { return };
        assert_eq!(
            context.pipeline_cache_origin(),
            PipelineCacheOrigin::DamagedFile(expected),
            "{name}"
        );
        // Драйвер блока не видел: кэш пуст, и пайплайн создаётся с нуля.
        let empty_size = context.pipeline_cache_initial_file_bytes();
        let _pipeline = create_test_pipeline(&context);
        assert!(
            context.pipeline_cache_data().unwrap().len() > empty_size,
            "{name}: кэш снова растёт"
        );
        assert_no_validation_errors(&context);
    }
}

#[test]
fn a_file_over_the_limit_starts_empty_and_is_reported_separately() {
    let Some(real) = real_file() else { return };
    let Some(context) = context_with_file(real.clone(), real.len() - 1) else {
        return;
    };
    assert_eq!(
        context.pipeline_cache_origin(),
        PipelineCacheOrigin::OversizedFile { bytes: real.len() }
    );
    let _pipeline = create_test_pipeline(&context);
    // Ровно по пределу — берётся.
    let Some(at_limit) = context_with_file(real.clone(), real.len()) else {
        return;
    };
    assert!(matches!(
        at_limit.pipeline_cache_origin(),
        PipelineCacheOrigin::Loaded { .. }
    ));
}

#[test]
fn a_valid_wrapper_around_a_foreign_driver_block_never_looks_like_a_warm_cache() {
    // Обёртка верна, но внутри блок другого устройства (чужой UUID в заголовке Vulkan) или просто мусор.
    let Some(real) = real_file() else { return };
    let mut foreign = unseal_cache_file(&real).unwrap().to_vec();
    for byte in &mut foreign[16..32] {
        *byte ^= 0xFF;
    }
    for (name, payload) in [
        ("чужой UUID", foreign),
        (
            "мусор внутри обёртки",
            (0..4096_u32).map(|index| (index * 31 % 251) as u8).collect(),
        ),
    ] {
        let file = seal_cache_file(&payload);
        let Some(context) = context_with(file) else { return };
        let origin = context.pipeline_cache_origin();
        assert!(
            matches!(
                origin,
                PipelineCacheOrigin::IgnoredByDriver { .. }
                    | PipelineCacheOrigin::RejectedByDriver
                    | PipelineCacheOrigin::Loaded { .. }
            ),
            "{name}: {origin:?}"
        );
        // Главное: база сравнения — фактическое состояние кэша. Первый же пайплайн даёт рост относительно НЕЁ.
        let baseline = context.pipeline_cache_initial_file_bytes();
        let _pipeline = create_test_pipeline(&context);
        assert!(
            context.pipeline_cache_data().unwrap().len() > baseline,
            "{name}: рост относительно фактического состояния кэша ({baseline}) виден"
        );
        assert_no_validation_errors(&context);
    }
}
