//! Кэш текстур картинок UI (сессия 14g): загрузка без ожиданий, вытеснение, безопасное освобождение.
//!
//! # Владение и жизненный цикл
//! Кэш владеет текстурами (ключ — [`PictureVersion`]), текстурой-заглушкой 1×1 для неиспользуемых
//! привязок и «отставкой» — объектами, которые GPU ещё может читать. Рендер вызывает кэш в таком порядке:
//! 1. [`PictureCache::stage_missing`] — до первой команды: выделяет память и пишет пиксели в staging-буферы
//!    для картинок, которых нет в кэше (ошибка здесь безопасна: команд нет);
//! 2. [`PictureCache::record_staged_uploads`] — копии ЗАПИСЫВАЮТСЯ в командный буфер кадра (вне проходов:
//!    динамический рендеринг запрещает копии внутри). Ничего не ждёт, `SampledTexture::upload` не зовётся;
//! 3. [`PictureCache::finish_frame`] — кадр записан успешно: считаем кадр, вытесняем давно не нужные картинки,
//!    отдаём staging в «отставку»; либо [`PictureCache::abort_frame`], если кадр упал после записи копий.
//!
//! # Когда объекты можно уничтожить
//! Staging читает GPU при копии; вытесненную текстуру могут читать кадры в полёте. Поэтому такие объекты
//! ждут в «отставке» (`waiting_slots`) и уничтожаются, когда ВСЕ слоты, бывшие в полёте при отставке, прошли
//! fence ([`PictureCache::release_slot_after_fence`]) — тот же приём, что у `RetiredPyramid`.
//!
//! # Инвариант «запись ≠ отправка»
//! Загрузка картинки считается состоявшейся только после fence слота, в чей кадр записана копия: до этого
//! кэш помнит её в `uploads_awaiting_fence`. Два пути, где копия может не выполниться:
//! * кадр упал после записи копий (`UiRenderError::Gpu`) — буфер на выброс: [`PictureCache::abort_frame`];
//! * кадр записан успешно, но его буфер НЕ отправлен (сбой сабмита, смена поколения swapchain) — вызывающий
//!   сообщает об этом [`PictureCache::frame_not_submitted`]. Без этого следующий кадр взял бы из кэша текстуру,
//!   в которую копия так и не легла, и рисовал бы мусор, пока картинка не сменится (ревью 14g).
//!
//! В обоих случаях текстуры выбрасываются из кэша, следующий кадр загрузит их заново.

use std::collections::HashMap;

use ash::vk;
use gpu_vulkan::compute::DescriptorResource;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};
use gpu_vulkan::sampled_texture::{PendingUploadTexture, SampledTexture, TextureDesc, TextureFormat};
use ui_draw::picture::{PictureVersion, UiPicture};

use crate::config::{MAX_IMAGE_BINDINGS, PictureCacheConfig};
use crate::plan::SamplerSlot;

/// Картинка в видеопамяти и когда её последний раз рисовали.
struct CachedPicture {
    texture: SampledTexture,
    /// Номер кадра кэша (`frame_serial`), в котором картинку использовали последний раз.
    last_used_frame: u64,
}

/// Объекты, которые GPU ещё может читать: живут, пока не освобождены слоты, бывшие тогда в полёте.
struct Retired {
    _texture: Option<SampledTexture>,
    _staging: Vec<HostBuffer>,
    waiting_slots: Vec<usize>,
}

/// Картинка, выделенная и заполненная в staging, но ещё без записанной копии.
pub(crate) struct StagedUpload {
    version: PictureVersion,
    pending: PendingUploadTexture,
    staging: HostBuffer,
}

/// Кэш текстур картинок.
pub(crate) struct PictureCache {
    config: PictureCacheConfig,
    entries: HashMap<PictureVersion, CachedPicture>,
    /// Заглушка для привязок без картинки кадра: дескрипторы обязаны быть валидными.
    placeholder: SampledTexture,
    retired: Vec<Retired>,
    /// Staging копий, записанных в текущий кадр (отправка ещё не подтверждена).
    frame_staging: Vec<HostBuffer>,
    /// Картинки, загруженные текущим кадром: при отказе кадра выбрасываются.
    frame_fresh: Vec<PictureVersion>,
    /// Картинки, чьи копии записаны в успешно записанный кадр слота, но fence этого слота ещё не пройден:
    /// если буфер кадра так и не отправят, они выбрасываются ([`Self::frame_not_submitted`]).
    uploads_awaiting_fence: HashMap<usize, Vec<PictureVersion>>,
    /// Сколько кадров записано успешно.
    frame_serial: u64,
}

fn rgba_desc(picture: &UiPicture) -> TextureDesc {
    let size = picture.size();
    TextureDesc {
        extent: vk::Extent2D {
            width: size.width,
            height: size.height,
        },
        format: TextureFormat::Rgba8Unorm,
        label: "ui-picture",
    }
}

impl PictureCache {
    /// Пустой кэш; заглушка загружается блокирующей отправкой (путь создания рендера, не кадра).
    pub(crate) fn new(context: &GpuContext, config: PictureCacheConfig) -> Result<Self, GpuError> {
        let placeholder = SampledTexture::upload(
            context,
            &TextureDesc {
                extent: vk::Extent2D { width: 1, height: 1 },
                format: TextureFormat::Rgba8Unorm,
                label: "ui-picture-placeholder",
            },
            &[0, 0, 0, 0],
        )?;
        Ok(Self {
            config,
            entries: HashMap::new(),
            placeholder,
            retired: Vec::new(),
            frame_staging: Vec::new(),
            frame_fresh: Vec::new(),
            uploads_awaiting_fence: HashMap::new(),
            frame_serial: 0,
        })
    }

    /// Сколько картинок сейчас в видеопамяти (без заглушки).
    pub(crate) fn cached_pictures(&self) -> usize {
        self.entries.len()
    }

    /// Ресурсы привязок сэмплеров кадра по слотам плана: картинка из памяти — текстура кэша, живая картинка —
    /// её представление напрямую (кэш её не знает: ни загрузки, ни вытеснения, ни отставки), остальное —
    /// заглушка. Всегда `MAX_IMAGE_BINDINGS` штук; картинки из памяти обязаны быть в кэше
    /// (после `record_staged_uploads`).
    pub(crate) fn bindings_for(&self, slots: &[SamplerSlot]) -> Vec<DescriptorResource> {
        (0..MAX_IMAGE_BINDINGS as usize)
            .map(|index| match slots.get(index) {
                Some(SamplerSlot::Live { view, .. }) => DescriptorResource::ReadOnlyImage { view: *view },
                Some(SamplerSlot::Picture(picture)) => self.entries.get(&picture.version()).map_or_else(
                    || self.placeholder.read_only_binding(),
                    |cached| cached.texture.read_only_binding(),
                ),
                None => self.placeholder.read_only_binding(),
            })
            .collect()
    }

    /// Шаг 1 (до первой команды): готовит загрузки для картинок плана, которых нет в кэше. Возвращает
    /// подготовленное и число попаданий в кэш; попавшим обновляется «последнее использование».
    pub(crate) fn stage_missing<'a>(
        &mut self,
        context: &GpuContext,
        pictures: impl IntoIterator<Item = &'a UiPicture>,
    ) -> Result<(Vec<StagedUpload>, u32), GpuError> {
        let mut staged = Vec::new();
        let mut hits = 0_u32;
        for picture in pictures {
            if let Some(cached) = self.entries.get_mut(&picture.version()) {
                // Обновляем сразу: вытеснение в конце кадра не тронет картинку, нарисованную в нём.
                cached.last_used_frame = self.frame_serial;
                hits += 1;
                continue;
            }
            let desc = rgba_desc(picture);
            let mut staging = HostBuffer::new(context, desc.texel_bytes(), HostBufferUsage::ReadOnlyStorage)?;
            staging.write_bytes(0, picture.rgba8())?;
            let pending = SampledTexture::allocate_for_recorded_upload(context, &desc)?;
            staged.push(StagedUpload {
                version: picture.version(),
                pending,
                staging,
            });
        }
        Ok((staged, hits))
    }

    /// Шаг 2: записывает копии в `command_buffer` (вне проходов) и кладёт текстуры в кэш.
    pub(crate) fn record_staged_uploads(
        &mut self,
        command_buffer: vk::CommandBuffer,
        staged: Vec<StagedUpload>,
    ) -> Result<u32, GpuError> {
        let mut uploads = 0_u32;
        for upload in staged {
            let texture = upload.pending.record_upload_from(command_buffer, &upload.staging)?;
            self.entries.insert(
                upload.version,
                CachedPicture {
                    texture,
                    last_used_frame: self.frame_serial,
                },
            );
            self.frame_fresh.push(upload.version);
            self.frame_staging.push(upload.staging);
            uploads += 1;
        }
        Ok(uploads)
    }

    /// Шаг 3 (кадр записан): считаем кадр, отдаём staging в отставку (ждёт `slot`), вытесняем давно не
    /// нужные картинки (ждут все слоты из `slots_in_flight`, включая `slot`). Возвращает число вытесненных.
    pub(crate) fn finish_frame(&mut self, slot: usize, slots_in_flight: Vec<usize>) -> u32 {
        // Загрузки кадра ждут подтверждения отправки: fence слота (`release_slot_after_fence`) или отказ
        // (`frame_not_submitted`). Прежняя запись слота уже снята его fence — слот не выдаётся заново раньше.
        let fresh = std::mem::take(&mut self.frame_fresh);
        if !fresh.is_empty() {
            self.uploads_awaiting_fence.insert(slot, fresh);
        }
        let staging = std::mem::take(&mut self.frame_staging);
        if !staging.is_empty() {
            self.retired.push(Retired {
                _texture: None,
                _staging: staging,
                waiting_slots: vec![slot],
            });
        }
        let this_frame = self.frame_serial;
        self.frame_serial += 1;
        let limit = u64::from(self.config.idle_frames_before_eviction);
        let idle: Vec<PictureVersion> = self
            .entries
            .iter()
            .filter(|(_, cached)| this_frame - cached.last_used_frame > limit)
            .map(|(version, _)| *version)
            .collect();
        let mut evicted = 0_u32;
        for version in idle {
            if let Some(cached) = self.entries.remove(&version) {
                self.retire_texture(cached.texture, slots_in_flight.clone());
                evicted += 1;
            }
        }
        evicted
    }

    /// Кадр упал после записи копий (или до — тогда `frame_fresh` пуст): загруженные им текстуры могут не
    /// выполниться на GPU, из кэша они выбрасываются; буфер кадра мог попасть в отправку, поэтому объекты
    /// ждут все слоты из `slots_in_flight`.
    pub(crate) fn abort_frame(&mut self, slots_in_flight: &[usize]) {
        let staging = std::mem::take(&mut self.frame_staging);
        for version in std::mem::take(&mut self.frame_fresh) {
            if let Some(cached) = self.entries.remove(&version) {
                self.retire_texture(cached.texture, slots_in_flight.to_vec());
            }
        }
        if !staging.is_empty() {
            self.retired.push(Retired {
                _texture: None,
                _staging: staging,
                waiting_slots: slots_in_flight.to_vec(),
            });
        }
    }

    /// Успешно записанный кадр слота `slot` НЕ отправлен: копии его загрузок не выполнились, эти текстуры
    /// выбрасываются из кэша (следующий кадр загрузит их заново). GPU их не читал (буфер не отправлен), но
    /// кадры в полёте других слотов по правилу отставки тоже переждём — `slots_in_flight` без `slot`.
    /// Staging этого кадра уже в отставке и уйдёт с ближайшим `release_slot_after_fence(slot)`.
    /// Возвращает число выброшенных картинок.
    pub(crate) fn frame_not_submitted(&mut self, slot: usize, slots_in_flight: &[usize]) -> u32 {
        let mut discarded = 0_u32;
        for version in self.uploads_awaiting_fence.remove(&slot).unwrap_or_default() {
            if let Some(cached) = self.entries.remove(&version) {
                self.retire_texture(cached.texture, slots_in_flight.to_vec());
                discarded += 1;
            }
        }
        discarded
    }

    /// fence слота пройден: отставленные объекты, ждавшие только его (и уже освобождённые), уничтожаются.
    /// Загрузки, записанные в кадр этого слота, выполнены: подтверждение снимается.
    pub(crate) fn release_slot_after_fence(&mut self, slot: usize) {
        self.uploads_awaiting_fence.remove(&slot);
        for retired in &mut self.retired {
            retired.waiting_slots.retain(|waiting| *waiting != slot);
        }
        self.retired.retain(|retired| !retired.waiting_slots.is_empty());
    }

    fn retire_texture(&mut self, texture: SampledTexture, waiting_slots: Vec<usize>) {
        if waiting_slots.is_empty() {
            // Никто не мог читать: уничтожается сразу (drop).
            return;
        }
        self.retired.push(Retired {
            _texture: Some(texture),
            _staging: Vec::new(),
            waiting_slots,
        });
    }
}
