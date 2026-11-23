//! Общие помощники GPU-тестов цветовых приборов: контекст, входные картинки с заданными значениями, сцены,
//! прогон кадра приборов в одной блокирующей отправке, эталон на CPU из ЧТЕНИЯ той же картинки.
//!
//! Политика «нет GPU» — как у остальных тестов: тест ПАДАЕТ, если только не задано `FASTICAMERA_SKIP_GPU_TESTS=1`.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "помощник тестов: каждый бинарь берёт свою часть; падение — способ сообщить о провале"
)]

use ash::vk;
use gpu_vulkan::compute::{BindingKind, BindingWrite, ComputePipeline, ComputePipelineDesc, LocalSize, WorkgroupCount};
use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};
use gpu_vulkan::kit_test_support::{read_storage_image_rgba, run_one_shot_blocking};
use gpu_vulkan::source_window::{HorizontalMirror, SourceWindow};
use gpu_vulkan::storage_image::{
    StorageFormatPolicy, StorageImage, StorageImageDesc, StorageImageFormat, StorageImagePreviousUse, StorageImageUsage,
};
use gpu_vulkan::validation::ValidationMode;
use isp_model::image::DisplayGamut;
use isp_vulkan::scopes::{
    ColorScopes, RecordedScope, ScopeError, ScopeFrame, ScopeInput, ScopePassCounters, ScopeSlotReport,
};
use isp_vulkan::shaders::blobs;
use scope_contract::{ScopeImageSize, ScopeKind, ScopeRequest, VectorscopeAxes};
use scope_model::{
    HorizontalOrder, ResolvedScope, ScopeCounters, ScopeModelSettings, ScopeRule, ShownCodes, accumulate,
};

const SKIP_ENV: &str = "FASTICAMERA_SKIP_GPU_TESTS";

/// Контекст `Headless` с полной validation или `None` при осознанном пропуске.
pub fn gpu_context_or_skip() -> Option<GpuContext> {
    let config = GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::FULL);
    match GpuContext::new(&config) {
        Ok(context) => Some(context),
        Err(error) => {
            if std::env::var(SKIP_ENV).as_deref() == Ok("1") {
                eprintln!("ПРОПУСК GPU-теста: {error} ({SKIP_ENV}=1)");
                None
            } else {
                panic!("GPU недоступен: {error}. Установите {SKIP_ENV}=1, чтобы пропустить GPU-тесты осознанно");
            }
        }
    }
}

/// Финальная проверка: GPU простаивает, validation не сообщила ни одной ошибки.
pub fn assert_no_validation_errors(context: &GpuContext) {
    context.wait_idle().expect("wait_idle в конце теста");
    let report = context.validation_report();
    assert_eq!(report.errors, 0, "validation нашла ошибки: {report:?}");
}

/// Маленькие сетки: тесты не гоняют CPU-эталон по 256×256×3 на каждый случай, но размеры нарочно не степени двойки.
pub fn small_settings() -> ScopeModelSettings {
    ScopeModelSettings {
        trace_columns: 97,
        trace_levels: 128,
        vectorscope_size: 128,
        ..ScopeModelSettings::default()
    }
}

/// Четыре эталонных прибора + оба вектороскопа: (название, запрос, гамма).
pub fn rule_cases(settings: &ScopeModelSettings) -> Vec<(&'static str, ResolvedScope)> {
    let size = ScopeImageSize::try_new(160, 96).unwrap();
    let request = |kind, vectorscope_axes| ScopeRequest {
        kind,
        vectorscope_axes,
        image_size: size,
    };
    let resolve = |request, gamut| ResolvedScope::resolve(&request, settings, gamut).unwrap();
    vec![
        (
            "waveform",
            resolve(
                request(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709),
                DisplayGamut::Srgb,
            ),
        ),
        (
            "parade",
            resolve(
                request(ScopeKind::RgbParade, VectorscopeAxes::YCbCrBt709),
                DisplayGamut::Srgb,
            ),
        ),
        (
            "vectorscope-ycbcr",
            resolve(
                request(ScopeKind::Vectorscope, VectorscopeAxes::YCbCrBt709),
                DisplayGamut::Srgb,
            ),
        ),
        (
            "vectorscope-oklab-srgb",
            resolve(
                request(ScopeKind::Vectorscope, VectorscopeAxes::OklabAb),
                DisplayGamut::Srgb,
            ),
        ),
        (
            "vectorscope-oklab-p3",
            resolve(
                request(ScopeKind::Vectorscope, VectorscopeAxes::OklabAb),
                DisplayGamut::DisplayP3,
            ),
        ),
    ]
}

// ---------------------------------------------------------------------------------------------------------
// Сцены (коды 0…1023 на пиксель)
// ---------------------------------------------------------------------------------------------------------

/// Картинка в кодах: построчно.
#[derive(Debug, Clone)]
pub struct Scene {
    pub name: &'static str,
    pub width: u32,
    pub height: u32,
    pub codes: Vec<[u16; 3]>,
}

/// Генератор псевдослучайных чисел (xorshift32) с фиксированным зерном: тесты воспроизводимы.
pub struct Xorshift(pub u32);

impl Xorshift {
    pub fn next(&mut self) -> u32 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
        self.0 = value;
        value
    }
}

fn scene(name: &'static str, width: u32, height: u32, mut code: impl FnMut(u32, u32) -> [u16; 3]) -> Scene {
    let mut codes = Vec::with_capacity(width as usize * height as usize);
    for y in 0..height {
        for x in 0..width {
            codes.push(code(x, y));
        }
    }
    Scene {
        name,
        width,
        height,
        codes,
    }
}

/// Горизонтальный серый градиент: диагональ на waveform, точка в центре вектороскопа.
pub fn gradient_scene() -> Scene {
    scene("gradient", 61, 23, |x, _| {
        let level = (x * 1023 / 60) as u16;
        [level, level, level]
    })
}

/// Серая шкала по ВСЕМ 1024 кодам.
pub fn gray_ramp_scene() -> Scene {
    scene("gray-ramp-1024", 1024, 3, |x, _| [x as u16; 3])
}

/// Цветные полосы 75 %: белая, жёлтая, голубая, зелёная, пурпурная, красная, синяя, чёрная.
pub fn bars75_scene() -> Scene {
    const FULL: u16 = 767; // 0,75 · 1023 = 767,25 → код 767
    const BARS: [[u16; 3]; 8] = [
        [FULL, FULL, FULL],
        [FULL, FULL, 0],
        [0, FULL, FULL],
        [0, FULL, 0],
        [FULL, 0, FULL],
        [FULL, 0, 0],
        [0, 0, FULL],
        [0, 0, 0],
    ];
    scene("bars75", 64, 20, |x, _| BARS[(x / 8) as usize])
}

/// Случайные коды по всему кубу 0…1023.
pub fn random_scene(seed: u32) -> Scene {
    let mut random = Xorshift(seed);
    scene("random", 97, 61, move |_, _| {
        [0, 1, 2].map(|_| (random.next() % 1024) as u16)
    })
}

/// Значения, у которых код определяется правилом квантования (границы половины кода, насыщение).
pub fn edge_values_scene() -> Scene {
    let mut random = Xorshift(0x1234_5679);
    scene("code-edges", 33, 17, move |_, _| {
        [0, 1, 2].map(|_| {
            let pick = random.next() % 6;
            [0, 1, 511, 512, 1022, 1023][pick as usize]
        })
    })
}

/// Узкая сцена: ширина 13 с шагом 3 различает «шаг по показанному, потом зеркало» и «зеркало, потом шаг».
pub fn narrow_scene() -> Scene {
    let mut random = Xorshift(0x0BAD_CAFE);
    scene("narrow-13x9", 13, 9, move |_, _| {
        [0, 1, 2].map(|_| (random.next() % 1024) as u16)
    })
}

/// Все сцены паритета.
pub fn all_scenes() -> Vec<Scene> {
    vec![
        gradient_scene(),
        gray_ramp_scene(),
        bars75_scene(),
        random_scene(0x9E37_79B9),
        edge_values_scene(),
        narrow_scene(),
    ]
}

/// Код канала как значение экрана `c / 1023` (так хранит выход видоискателя).
pub fn display_value(code: u16) -> f32 {
    f32::from(code) / 1023.0
}

// ---------------------------------------------------------------------------------------------------------
// Входные картинки
// ---------------------------------------------------------------------------------------------------------

/// Картинка-вход приборов: storage-картинка ёмкости с заданным содержимым в левом верхнем прямоугольнике.
pub struct InputImage {
    pub image: StorageImage,
    pub content: vk::Extent2D,
}

impl InputImage {
    /// Окно чтения содержимого с зеркалом `mirror`.
    pub fn window(&self, mirror: HorizontalMirror) -> SourceWindow {
        SourceWindow::of_content(self.content).with_mirror(mirror)
    }

    /// Вход приборов: читать ТОЛЬКО содержимое.
    pub fn input(&self, mirror: HorizontalMirror) -> ScopeInput {
        ScopeInput {
            view: self.image.sampled_view(),
            window: self.window(mirror),
        }
    }

    /// Читает ВСЮ картинку (ёмкость) на CPU.
    pub fn read_all(&self) -> Vec<[f32; 4]> {
        read_storage_image_rgba(
            &self.image,
            StorageImagePreviousUse::SampledRead(ConsumerStage::Compute),
        )
        .unwrap()
    }

    /// Показанная картинка по ЧТЕНИЮ: левый верхний прямоугольник содержимого, с зеркалом или без.
    pub fn shown_codes(&self, mirror: HorizontalMirror) -> ShownCodes {
        shown_codes_of(&self.read_all(), self.image.extent().width, self.content, mirror)
    }
}

/// Показанные коды из чтения картинки ёмкости шириной `capacity_width`.
pub fn shown_codes_of(
    texels: &[[f32; 4]],
    capacity_width: u32,
    content: vk::Extent2D,
    mirror: HorizontalMirror,
) -> ShownCodes {
    let values: Vec<[f32; 4]> = (0..content.height)
        .flat_map(|y| (0..content.width).map(move |x| (y * capacity_width + x) as usize))
        .map(|index| texels[index])
        .collect();
    let order = match mirror {
        HorizontalMirror::On => HorizontalOrder::Mirrored,
        HorizontalMirror::Off => HorizontalOrder::AsStored,
    };
    ShownCodes::from_display_rgba(content.width, content.height, &values, order).unwrap()
}

/// Создаёт картинку ёмкости `capacity` формата `policy` и заполняет её texel-ами `texels` (построчно, ёмкость целиком)
/// компьютным шейдером. После заполнения картинка читаема стадией COMPUTE (`SHADER_READ_ONLY_OPTIMAL`).
pub fn upload_input(
    context: &GpuContext,
    capacity: vk::Extent2D,
    content: vk::Extent2D,
    policy: StorageFormatPolicy,
    texels: &[[f32; 4]],
) -> InputImage {
    assert_eq!(texels.len(), (capacity.width * capacity.height) as usize);
    let image = StorageImage::new(
        context,
        &StorageImageDesc {
            extent: capacity,
            format_policy: policy,
            usage: StorageImageUsage::ComputeWriteSampledRead,
        },
    )
    .unwrap();
    let spirv = match image.format() {
        StorageImageFormat::A2B10G10R10Unorm => blobs::TEST_SCOPE_FILL_RGB10A2_COMP,
        StorageImageFormat::Rgba16Float => blobs::TEST_SCOPE_FILL_RGBA16F_COMP,
    };
    let mut pipeline = ComputePipeline::new(
        context,
        &ComputePipelineDesc {
            label: "test-scope-fill",
            spirv,
            bindings: &[BindingKind::StorageImageWrite, BindingKind::ReadOnlyStorageBuffer],
            push_constant_bytes: 8,
            specialization: &[],
            frame_slots: 1,
        },
    )
    .unwrap();
    let bytes: Vec<u8> = texels.iter().flatten().flat_map(|value| value.to_ne_bytes()).collect();
    let mut buffer = HostBuffer::new(context, bytes.len() as u64, HostBufferUsage::ReadOnlyStorage).unwrap();
    buffer.write_bytes(0, &bytes).unwrap();
    pipeline
        .bind(
            0,
            &[
                BindingWrite {
                    binding: 0,
                    resource: image.storage_binding(),
                },
                BindingWrite {
                    binding: 1,
                    resource: buffer.binding(),
                },
            ],
        )
        .unwrap();
    let local = LocalSize::new(16, 8).unwrap();
    let groups = WorkgroupCount::covering(capacity.width, capacity.height, local);
    let mut push = [0_u8; 8];
    push[..4].copy_from_slice(&capacity.width.to_ne_bytes());
    push[4..].copy_from_slice(&capacity.height.to_ne_bytes());
    run_one_shot_blocking(context, |command_buffer| {
        image.record_prepare_for_compute_write(command_buffer, StorageImagePreviousUse::NotUsedYet);
        pipeline.record_dispatch(command_buffer, 0, &push, groups)?;
        image.record_compute_write_to_sampled_read(command_buffer, ConsumerStage::Compute);
        Ok(())
    })
    .unwrap();
    InputImage { image, content }
}

/// Входная картинка из сцены: ёмкость больше содержимого на `(extra_width, extra_height)`, лишнее заполнено
/// «ядовитыми» пикселями (случайные коды): приборы обязаны читать только содержимое.
pub fn upload_scene(context: &GpuContext, scene: &Scene, policy: StorageFormatPolicy, extra: (u32, u32)) -> InputImage {
    let capacity = vk::Extent2D {
        width: scene.width + extra.0,
        height: scene.height + extra.1,
    };
    let mut poison = Xorshift(0xDEAD_BEEF);
    let texels: Vec<[f32; 4]> = (0..capacity.height)
        .flat_map(|y| (0..capacity.width).map(move |x| (x, y)))
        .map(|(x, y)| {
            if x < scene.width && y < scene.height {
                let code = scene.codes[(y * scene.width + x) as usize];
                [
                    display_value(code[0]),
                    display_value(code[1]),
                    display_value(code[2]),
                    1.0,
                ]
            } else {
                let mut channel = || display_value((poison.next() % 1024) as u16);
                [channel(), channel(), channel(), 1.0]
            }
        })
        .collect();
    upload_input(
        context,
        capacity,
        vk::Extent2D {
            width: scene.width,
            height: scene.height,
        },
        policy,
        &texels,
    )
}

// ---------------------------------------------------------------------------------------------------------
// Прогон кадра приборов
// ---------------------------------------------------------------------------------------------------------

/// Записывает один кадр приборов в одной блокирующей отправке и сразу подтверждает fence слота.
pub fn run_scope_frame(
    context: &GpuContext,
    scopes: &mut ColorScopes,
    frame: &ScopeFrame<'_>,
) -> Result<(RecordedScope, ScopeSlotReport), ScopeError> {
    let mut outcome = None;
    run_one_shot_blocking(context, |command_buffer| {
        outcome = Some(scopes.record(command_buffer, frame));
        Ok(())
    })
    .unwrap();
    let recorded = outcome.expect("замыкание отправки вызвано")?;
    let report = scopes.release_slot_after_fence(frame.frame_slot)?;
    Ok((recorded, report))
}

/// Эталонные счётчики CPU для показанной картинки.
pub fn cpu_counters(shown: &ShownCodes, settings: &ScopeModelSettings, rule: &dyn ScopeRule) -> ScopeCounters {
    accumulate(shown, settings.sample_stride, rule).unwrap()
}

/// Размер картинки прибора для тестов.
pub fn picture_size(width: u32, height: u32) -> ScopeImageSize {
    ScopeImageSize::try_new(width, height).unwrap()
}

/// Счётчики диагностики до/после.
pub fn counters_delta(before: ScopePassCounters, after: ScopePassCounters) -> ScopePassCounters {
    ScopePassCounters {
        recorded: after.recorded - before.recorded,
        accumulate_dispatches: after.accumulate_dispatches - before.accumulate_dispatches,
        render_dispatches: after.render_dispatches - before.render_dispatches,
        picture_recreations: after.picture_recreations - before.picture_recreations,
    }
}

pub fn mirror_name(mirror: HorizontalMirror) -> &'static str {
    match mirror {
        HorizontalMirror::On => "mirrored",
        HorizontalMirror::Off => "plain",
    }
}
