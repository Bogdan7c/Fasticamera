//! [`PhotoDeveloper`] — стадия «проявление» снимка по полосам.
//!
//! # Протокол (рендер вызывает между кадрами видоискателя)
//! ```text
//! begin(request)                                  — выделяет картинку снимка, uniform, привязки (CPU, без GPU)
//! повторять:
//!   record_next_slice(command_buffer) → Recorded  — одна полоса в командный буфер кадра
//!   [рендер отправляет буфер с fence]
//!   slice_completed()                             — после fence этого сабмита
//! record_next_slice → Finished                    — полос больше нет
//! finish() → DevelopedPhoto                       — после fence последней полосы
//! ```
//! Полосы разных сабмитов упорядочены очередью (один GPU), поэтому рендер может записывать следующую полосу,
//! не дожидаясь fence предыдущей; `slice_completed` вызывается по числу подтверждённых fence-ей сабмитов.
//!
//! # Что внутри полосы
//! Область полосы (crop-ширина + ореол по ширине × строки полосы + ореол по высоте) проявляется как
//! самостоятельная мини-картинка: уровни+баланс из буфера захвата → Menon (8 проходов стенда V4) → выход
//! (матрицы, охват, тон, OETF, квантование) в окно картинки снимка. Геометрия и ореол — [`super::slicing`].
//!
//! # Память
//! Плоскости f32 — семь штук размером «область полосы» (а не кадра): для 4096×3072 при полосе 256 строк это
//! 4096 × 282 × 4 Б × 7 = 32,3 МБ = 30,8 МиБ (кадр целиком — 7 × 48 МиБ = 336 МиБ). Остальное — выход (картинка снимка
//! 4096×3072×4 = 48 МиБ на GPU, по запросу столько же в памяти хоста) и буфер захвата (24 МиБ).
//!
//! # Уничтожение
//! `PhotoDeveloper` с неподтверждёнными полосами (записаны, fence не подтверждён) при уничтожении
//! ТЕРЯЕТСЯ вместе со всеми объектами Vulkan (лучше утечка, чем уничтожение работающего GPU, соглашение крейта).

use std::sync::Arc;

use ash::vk;
use camera_contract::geometry::{PixelRect, PixelSize};
use gpu_vulkan::compute::{BindingKind, BindingWrite};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::device_buffer::record_compute_pass_dependency;
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};
use gpu_vulkan::host_readback::HostReadbackBuffer;
use gpu_vulkan::one_shot_queue::{CompletedWork, SubmittedWork};
use gpu_vulkan::rgba8_image::Rgba8StorageImage;
use still_contract::shot::Rgba8Image;

use super::capture::RawCapture;
use super::config::PhotoConfig;
use super::developed::{DevelopRequest, DevelopedPhoto, DevelopmentStart, RgbaReadback, SliceOutcome};
use super::error::PhotoError;
use super::pixels::{GpuPixels, seal_or_leak};
use super::slicing::{DevelopmentPlan, SlicePlan, max_region_rows, plan_development};
use crate::quality::demosaic::{MenonPass, create_pass_pipelines};
use crate::quality::frame_params::{
    CfaCode, FRAME_PARAMETERS_BYTES, SourceAddressing, check_supported, frame_parameter_bytes,
};
use crate::quality::pass::{PassPushConstants, QualityPass};
use crate::quality::planes::{LinearRgbPlanes, QualityPlanes};
use crate::shaders::blobs;

/// Единственный слот наборов дескрипторов: проявление одно за раз.
const SLOT: usize = 0;

/// Привязки шейдера уровней: сырые отсчёты (чтение), uniform кадра, мозаика (запись).
const LEVELS_BINDINGS: [BindingKind; 3] = [
    BindingKind::ReadOnlyStorageBuffer,
    BindingKind::UniformBuffer,
    BindingKind::ReadWriteStorageBuffer,
];

/// Привязки выходного шейдера: R, G, B (чтение), uniform кадра, картинка снимка (запись).
const OUTPUT_BINDINGS: [BindingKind; 5] = [
    BindingKind::ReadOnlyStorageBuffer,
    BindingKind::ReadOnlyStorageBuffer,
    BindingKind::ReadOnlyStorageBuffer,
    BindingKind::UniformBuffer,
    BindingKind::StorageImageWrite,
];

/// Проявитель снимка. Живёт долго (пайплайны и плоскости создаются один раз); проявляет одну картинку за раз.
pub struct PhotoDeveloper {
    /// `None` только внутри `Drop` (см. «Уничтожение» в документации модуля).
    parts: Option<Box<DeveloperParts>>,
}

impl std::fmt::Debug for PhotoDeveloper {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.parts {
            Some(parts) => write!(
                formatter,
                "PhotoDeveloper({:?}, busy: {})",
                parts.planes,
                parts.job.is_some()
            ),
            None => write!(formatter, "PhotoDeveloper(released)"),
        }
    }
}

impl PhotoDeveloper {
    /// Создаёт проявитель. `max_valid_width` — наибольшая ширина полезной области кадра (чётная): плоскости
    /// выделяются под область полосы такой ширины. Больше памяти, чем `max_region_rows × ширина × 28 байт`,
    /// проявитель не держит.
    pub fn new(context: &GpuContext, config: &PhotoConfig, max_valid_width: u32) -> Result<Self, PhotoError> {
        config.validate()?;
        let local = config.local_size()?;
        local.validate_for_device(context)?;
        let planes = QualityPlanes::new(
            context,
            vk::Extent2D {
                width: max_valid_width,
                height: max_region_rows(config.slice_rows),
            },
        )?;
        let menon = create_pass_pipelines(context, &planes, local, 1)?;
        let levels = QualityPass::new(
            context,
            "isp-photo-levels",
            blobs::PHOTO_LEVELS_COMP,
            &LEVELS_BINDINGS,
            local,
            1,
        )?;
        let output = QualityPass::new(
            context,
            "isp-photo-output-rgba8",
            blobs::PHOTO_OUTPUT_RGBA8_COMP,
            &OUTPUT_BINDINGS,
            local,
            1,
        )?;
        Ok(Self {
            parts: Some(Box::new(DeveloperParts {
                job: None,
                output,
                menon,
                levels,
                planes,
                config: *config,
                max_valid_width,
                context: context.clone(),
            })),
        })
    }

    /// Начинает проявление: проверяет вход, строит план полос, выделяет картинку снимка (и буфер для JPEG по
    /// запросу), привязывает ресурсы. GPU не трогает и команд не пишет.
    ///
    /// Ошибка не оставляет активной проявки. Пока проявка не завершена ([`Self::finish`]) или не отменена
    /// ([`Self::cancel`]), повторный `begin` — `Busy`.
    pub fn begin(&mut self, request: &DevelopRequest<'_>) -> Result<DevelopmentStart, PhotoError> {
        self.parts_mut()?.begin(request)
    }

    /// Записывает следующую полосу в `command_buffer` (состояние записи, вне рендеринга). Все проверки — до
    /// первой команды: при ошибке в буфер ничего не записано и состояние не изменилось.
    ///
    /// Нет активной проявки — `OutOfOrder`. Все полосы записаны — [`SliceOutcome::Finished`] (буфер не тронут).
    pub fn record_next_slice(&mut self, command_buffer: vk::CommandBuffer) -> Result<SliceOutcome, PhotoError> {
        self.parts_mut()?.record_next_slice(command_buffer)
    }

    /// Сообщает проявителю, какой отправленной работой записана очередная полоса (сразу после
    /// `OneShotSubmitter::submit` с `record_next_slice`, вернувшим `Recorded`). Копия в буфер чтения хоста
    /// записана в ПОСЛЕДНЕЙ полосе; последняя отметка и есть связь «буфер ↔ работа», по которой `finish`
    /// проверяет доказательство. Пропущенная отметка последней полосы — отказ `finish` (`SealRefused`).
    pub fn note_slice_submitted(&mut self, work: &SubmittedWork) -> Result<(), PhotoError> {
        self.parts_mut()?.note_slice_submitted(work)
    }

    /// Подтверждает: сабмит, куда записана очередная неподтверждённая полоса, завершён. Звать по разу на
    /// каждую записанную полосу, по порядку записи; `proof` — доказательство завершения именно этого сабмита
    /// (`OneShotSubmitter::poll → Completed`). Доказательство ПОСЛЕДНЕЙ полосы запечатывает буфер чтения снимка.
    pub fn slice_completed(&mut self, proof: &CompletedWork) -> Result<(), PhotoError> {
        self.parts_mut()?.slice_completed(proof)
    }

    /// Завершает проявку: все полосы записаны и подтверждены. Освобождает наборы дескрипторов и отдаёт снимок.
    pub fn finish(&mut self) -> Result<DevelopedPhoto, PhotoError> {
        self.parts_mut()?.finish()
    }

    /// Отменяет проявку (снимок отброшен), освобождая ресурсы. Если есть неподтверждённые полосы — `Busy`:
    /// сначала подтвердите fence-и (GPU ещё пишет в картинку).
    pub fn cancel(&mut self) -> Result<(), PhotoError> {
        self.parts_mut()?.cancel()
    }

    /// Есть ли активная проявка (между `begin` и `finish`/`cancel`).
    pub fn is_developing(&self) -> bool {
        self.parts.as_ref().is_some_and(|parts| parts.job.is_some())
    }

    /// Плоскости линейного RGB камеры ПОСЛЕДНЕЙ записанной полосы (диагностика и тесты паритета): плотные строки
    /// шириной области проявления, строк столько, сколько в области полосы. Содержимое определено после fence
    /// сабмита этой полосы и до записи следующей. Нет проявителя (уничтожается) — `None`.
    pub fn linear_rgb_planes(&self) -> Option<LinearRgbPlanes<'_>> {
        self.parts.as_ref().map(|parts| parts.planes.linear_rgb())
    }

    /// Память плоскостей, байт (7 плоскостей f32 под область полосы): для отчётов и диагностики.
    pub fn plane_memory_bytes(&self) -> u64 {
        self.parts.as_ref().map_or(0, |parts| {
            let planes = parts.planes.extent();
            u64::from(planes.width) * u64::from(planes.height) * 4 * 7
        })
    }

    fn parts_mut(&mut self) -> Result<&mut DeveloperParts, PhotoError> {
        self.parts.as_deref_mut().ok_or(PhotoError::OutOfOrder {
            what: "проявитель уже уничтожается",
        })
    }
}

impl Drop for PhotoDeveloper {
    fn drop(&mut self) {
        let Some(parts) = self.parts.take() else { return };
        if parts.has_unconfirmed_slices() {
            tracing::error!(
                target: "fasticamera::isp",
                "проявитель снимка уничтожается с неподтверждёнными полосами: объекты Vulkan намеренно утекают"
            );
            std::mem::forget(parts);
        }
    }
}

/// Состояние и объекты проявителя. Порядок полей = порядок уничтожения: ресурсы проявки (на них ссылаются
/// наборы дескрипторов) → проходы → плоскости.
struct DeveloperParts {
    job: Option<Job>,
    output: QualityPass,
    menon: Vec<(MenonPass, QualityPass)>,
    levels: QualityPass,
    planes: QualityPlanes,
    config: PhotoConfig,
    max_valid_width: u32,
    context: GpuContext,
}

/// Активная проявка.
struct Job {
    plan: DevelopmentPlan,
    raw: RawCapture,
    cfa: CfaCode,
    image: Arc<Rgba8StorageImage>,
    readback: Option<HostReadbackBuffer>,
    /// Uniform-параметры кадра: на них ссылается набор дескрипторов, пока GPU не закончит.
    _parameters: HostBuffer,
    /// Сколько полос записано (индекс следующей).
    recorded: usize,
    /// Сколько записанных полос ещё не подтверждено fence-ом.
    unconfirmed: usize,
    /// Доказательство завершения последней подтверждённой полосы. Копия в буфер чтения записана в ПОСЛЕДНЕЙ
    /// полосе, а полосы подтверждаются по порядку записи, поэтому к `finish` (все подтверждены) здесь лежит
    /// доказательство именно сабмита с копией.
    last_proof: Option<CompletedWork>,
}

impl DeveloperParts {
    fn has_unconfirmed_slices(&self) -> bool {
        self.job.as_ref().is_some_and(|job| job.unconfirmed > 0)
    }

    fn begin(&mut self, request: &DevelopRequest<'_>) -> Result<DevelopmentStart, PhotoError> {
        if self.job.is_some() {
            return Err(PhotoError::Busy {
                what: "предыдущая проявка не завершена (finish или cancel)",
            });
        }
        check_supported(request.params)?;
        let valid = request.raw.valid_area();
        // Область снимка — в координатах буфера кадра; план считается в координатах полезной области.
        let crop_in_valid = PixelRect {
            left: request
                .crop
                .left
                .checked_sub(valid.left)
                .ok_or_else(|| crop_outside(request.crop, valid.left))?,
            top: request
                .crop
                .top
                .checked_sub(valid.top)
                .ok_or_else(|| crop_outside(request.crop, valid.top))?,
            size: request.crop.size,
        };
        let plan = plan_development(request.raw.size(), crop_in_valid, self.config.slice_rows)?;
        if plan.region_width > self.max_valid_width {
            return Err(PhotoError::InvalidInput {
                what: format!(
                    "ширина области проявления {} больше ширины, под которую выделены плоскости ({})",
                    plan.region_width, self.max_valid_width
                ),
            });
        }
        let cfa = CfaCode::from_pattern(request.params.pattern);
        // Левый/верхний край адресации не нужен: области полос задаются push-константами; шаг строки — только
        // для порядка (шейдер уровней берёт ширину буфера захвата из push-констант).
        let source = SourceAddressing {
            left: 0,
            top: 0,
            row_stride_bytes: valid.width.saturating_mul(2),
        };
        let mut parameters = HostBuffer::new(
            &self.context,
            FRAME_PARAMETERS_BYTES,
            HostBufferUsage::UniformParameters,
        )?;
        parameters.write_bytes(0, &frame_parameter_bytes(request.params, source))?;
        let image = Arc::new(Rgba8StorageImage::new(
            &self.context,
            vk::Extent2D {
                width: plan.crop_size.width,
                height: plan.crop_size.height,
            },
        )?);
        let readback = match request.readback {
            RgbaReadback::NotNeeded => None,
            RgbaReadback::ForJpeg => Some(HostReadbackBuffer::new(&self.context, image.packed_size_bytes())?),
        };
        if let Err(error) = self.bind_job_resources(request.raw, &parameters, &image) {
            // Ресурсы проявки (картинка, uniform) сейчас уничтожатся, а наборы могли запомнить их хэндлы: сброс
            // запомненного обязателен (см. `DescriptorRing::release_after_fence`). Наборы ни в одном
            // отправленном буфере не использовались, поэтому освободить их можно сразу.
            self.release_descriptor_slots()?;
            return Err(error);
        }

        let start = DevelopmentStart {
            slice_count: plan.slices.len(),
            image_size: plan.crop_size,
            widest_region: PixelSize::new(
                plan.region_width,
                plan.slices.iter().map(|slice| slice.region_rows).max().unwrap_or(0),
            ),
        };
        self.job = Some(Job {
            plan,
            raw: request.raw.clone(),
            cfa,
            image,
            readback,
            _parameters: parameters,
            recorded: 0,
            unconfirmed: 0,
            last_proof: None,
        });
        Ok(start)
    }

    /// Привязывает ресурсы проявки к наборам уровней и выхода (плоскости Menon привязаны при создании).
    fn bind_job_resources(
        &mut self,
        raw: &RawCapture,
        parameters: &HostBuffer,
        image: &Rgba8StorageImage,
    ) -> Result<(), PhotoError> {
        self.levels.bind(
            SLOT,
            &[
                BindingWrite {
                    binding: 0,
                    resource: raw.read_only_binding(),
                },
                BindingWrite {
                    binding: 1,
                    resource: parameters.binding(),
                },
                BindingWrite {
                    binding: 2,
                    resource: self.planes.mosaic().read_write_binding(),
                },
            ],
        )?;
        let linear = self.planes.linear_rgb();
        let resources = [
            linear.red.read_only_binding(),
            linear.green.read_only_binding(),
            linear.blue.read_only_binding(),
            parameters.binding(),
            image.storage_binding(),
        ];
        let writes: Vec<BindingWrite> = (0_u32..)
            .zip(resources)
            .map(|(binding, resource)| BindingWrite { binding, resource })
            .collect();
        self.output.bind(SLOT, &writes)?;
        Ok(())
    }

    fn record_next_slice(&mut self, command_buffer: vk::CommandBuffer) -> Result<SliceOutcome, PhotoError> {
        let Some(job) = self.job.as_ref() else {
            return Err(PhotoError::OutOfOrder {
                what: "record_next_slice без активной проявки (begin)",
            });
        };
        let Some(slice) = job.plan.slices.get(job.recorded).copied() else {
            return Ok(SliceOutcome::Finished);
        };
        let pushes = SlicePushes::new(&job.plan, &slice, job.cfa, job.raw.size().width);
        self.check_slice(&pushes)?;

        // --- Команды (все проверки пройдены) ---
        let is_first = job.recorded == 0;
        let is_last = job.recorded + 1 == job.plan.slices.len();
        if is_first {
            job.image.record_prepare_for_compute_write(command_buffer);
        }
        // Начало полосы: прошлые полосы (в том числе в прошлых сабмитах) и захват закончили писать плоскости.
        record_compute_pass_dependency(&self.context, command_buffer);
        self.levels.record(command_buffer, SLOT, &pushes.levels)?;
        for (_, pass) in &self.menon {
            record_compute_pass_dependency(&self.context, command_buffer);
            pass.record(command_buffer, SLOT, &pushes.geometry)?;
        }
        record_compute_pass_dependency(&self.context, command_buffer);
        self.output.record(command_buffer, SLOT, &pushes.output)?;
        if is_last {
            // Картинка готова: отдаём копирующим командам (surface кодера HEIF, буфер JPEG).
            job.image.record_compute_write_to_transfer_read(command_buffer);
            if let Some(readback) = &job.readback {
                job.image.record_copy_to_host_readback(command_buffer, readback)?;
            }
        }

        let Some(job) = self.job.as_mut() else {
            return Err(PhotoError::OutOfOrder {
                what: "проявка пропала во время записи полосы",
            });
        };
        job.recorded += 1;
        job.unconfirmed += 1;
        Ok(SliceOutcome::Recorded {
            remaining: job.plan.slices.len() - job.recorded,
        })
    }

    /// Проверки dispatch всех проходов полосы без записи команд.
    fn check_slice(&self, pushes: &SlicePushes) -> Result<(), PhotoError> {
        self.levels.check(SLOT, &pushes.levels)?;
        for (_, pass) in &self.menon {
            pass.check(SLOT, &pushes.geometry)?;
        }
        self.output.check(SLOT, &pushes.output)?;
        Ok(())
    }

    fn note_slice_submitted(&mut self, work: &SubmittedWork) -> Result<(), PhotoError> {
        let Some(job) = self.job.as_mut() else {
            return Err(PhotoError::OutOfOrder {
                what: "note_slice_submitted без активной проявки",
            });
        };
        if let Some(readback) = job.readback.as_mut() {
            readback.mark_written_by(work);
        }
        Ok(())
    }

    fn slice_completed(&mut self, proof: &CompletedWork) -> Result<(), PhotoError> {
        let Some(job) = self.job.as_mut() else {
            return Err(PhotoError::OutOfOrder {
                what: "slice_completed без активной проявки",
            });
        };
        if job.unconfirmed == 0 {
            return Err(PhotoError::OutOfOrder {
                what: "slice_completed: нет записанной полосы, ждущей подтверждения",
            });
        }
        job.unconfirmed -= 1;
        job.last_proof = Some(proof.clone());
        Ok(())
    }

    fn finish(&mut self) -> Result<DevelopedPhoto, PhotoError> {
        let Some(job) = self.job.as_ref() else {
            return Err(PhotoError::OutOfOrder {
                what: "finish без активной проявки",
            });
        };
        if job.recorded < job.plan.slices.len() {
            return Err(PhotoError::OutOfOrder {
                what: "finish: записаны не все полосы",
            });
        }
        if job.unconfirmed > 0 {
            return Err(PhotoError::OutOfOrder {
                what: "finish: есть полосы без подтверждения fence-ом",
            });
        }
        self.release_descriptor_slots()?;
        let Some(job) = self.job.take() else {
            return Err(PhotoError::OutOfOrder {
                what: "проявка пропала при завершении",
            });
        };
        let size = job.plan.crop_size;
        let host_rgba8 = match job.readback {
            Some(readback) => {
                // Копия записана в последней полосе, её завершение подтверждено (unconfirmed == 0) доказательством.
                let Some(proof) = job.last_proof.as_ref() else {
                    return Err(PhotoError::OutOfOrder {
                        what: "finish: нет доказательства завершения последней полосы",
                    });
                };
                let pixels = Arc::new(GpuPixels::new(seal_or_leak(readback, proof)?));
                Some(
                    Rgba8Image::new(size, pixels).map_err(|mismatch| PhotoError::InvalidInput {
                        what: mismatch.to_string(),
                    })?,
                )
            }
            None => None,
        };
        Ok(DevelopedPhoto {
            image: job.image,
            size,
            host_rgba8,
        })
    }

    fn cancel(&mut self) -> Result<(), PhotoError> {
        match &self.job {
            None => Ok(()),
            Some(job) if job.unconfirmed > 0 => Err(PhotoError::Busy {
                what: "есть полосы без подтверждения fence-ом: GPU ещё пишет в картинку снимка",
            }),
            Some(_) => {
                self.release_descriptor_slots()?;
                self.job = None;
                Ok(())
            }
        }
    }

    /// Fence всех сабмитов проявки подтверждены: наборы уровней и выхода можно привязывать к новым ресурсам.
    fn release_descriptor_slots(&self) -> Result<(), PhotoError> {
        self.levels.release_slot_after_fence(SLOT)?;
        self.output.release_slot_after_fence(SLOT)?;
        for (_, pass) in &self.menon {
            pass.release_slot_after_fence(SLOT)?;
        }
        Ok(())
    }
}

fn crop_outside(crop: PixelRect, edge: u32) -> PhotoError {
    PhotoError::InvalidInput {
        what: format!("область снимка {crop:?} левее/выше полезной области (край {edge})"),
    }
}

/// Push-константы трёх видов проходов одной полосы (раскладка `PlaneGeometry`; индексы < 2^24 точны в f32).
struct SlicePushes {
    levels: PassPushConstants,
    geometry: PassPushConstants,
    output: PassPushConstants,
}

impl SlicePushes {
    fn new(plan: &DevelopmentPlan, slice: &SlicePlan, cfa: CfaCode, source_width: u32) -> Self {
        let extent = vk::Extent2D {
            width: plan.region_width,
            height: slice.region_rows,
        };
        let geometry = PassPushConstants::geometry(extent, cfa);
        Self {
            levels: PassPushConstants {
                pass_parameters: [
                    plan.region_left as f32,
                    slice.region_top as f32,
                    source_width as f32,
                    0.0,
                ],
                ..geometry
            },
            output: PassPushConstants {
                pass_parameters: [
                    plan.window_left as f32,
                    slice.first_output_row as f32,
                    slice.output_rows as f32,
                    slice.destination_row as f32,
                ],
                ..geometry
            },
            geometry,
        }
    }
}
