//! Кэш пайплайнов устройства (`VkPipelineCache`): владелец, формат файла и обмен данными с диском.
//!
//! Зачем: компиляция одного варианта шейдера ISP на Adreno 740 — 145–180 мс, и драйвер между запусками приложения сам
//! ничего не помнит. Кэш, сохранённый в файл приложения и загруженный при следующем запуске, позволяет драйверу
//! пропускать компиляцию уже виденных шейдеров (замер I6: первый вариант 162 мс → 0,7 мс).
//!
//! # Формат файла (владелец — этот модуль; хранилище содержимого не знает)
//! Заголовок Vulkan (`VkPipelineCacheHeaderVersionOne`) не содержит ни длины, ни контрольной суммы полезной части, а
//! проверка чужих и обрезанных данных у драйверов разная (на Adreno она нам неизвестна). Поэтому драйверу отдаётся
//! ТОЛЬКО блок, который проверили мы сами. Файл = заголовок [`FILE_HEADER_BYTES`] байт + данные драйвера:
//!
//! | смещение | размер | что |
//! |---|---|---|
//! | 0 | 8 | магическое число `FSTIPCCH` |
//! | 8 | 4 | версия формата (LE), сейчас [`FORMAT_VERSION`] |
//! | 12 | 8 | длина данных драйвера (LE) |
//! | 20 | 8 | FNV-1a 64 данных драйвера (LE) |
//! | 28 | длина | данные `vkGetPipelineCacheData` как есть |
//!
//! Не сошлось что-то одно (обрезок, хвост мусора, перевёрнутый бит, чужой файл, другая версия формата) — блок драйверу
//! не отдаётся, кэш создаётся пустым, исход — [`PipelineCacheOrigin::DamagedFile`] с причиной [`FileDamage`].
//! Файл больше предела [`GpuContextConfig::pipeline_cache_max_bytes`](crate::context::GpuContextConfig) — тоже пустой кэш
//! ([`PipelineCacheOrigin::OversizedFile`]): кэш только растёт (каждая сборка с новыми шейдерами добавляет записи), и
//! без предела файл рос бы бесконечно. Следующее сохранение заменяет файл целиком.
//!
//! # Потокобезопасность (проверено по тексту спецификации Vulkan, docs.vulkan.org, refpages)
//! * `vkCreateComputePipelines` / `vkCreateGraphicsPipelines`, Valid Usage `VUID-...-pipelineCache-02873`: «If
//!   `pipelineCache` was created with `VK_PIPELINE_CACHE_CREATE_EXTERNALLY_SYNCHRONIZED_BIT`, host access to
//!   `pipelineCache` **must** be externally synchronized». Флаг `VkPipelineCacheCreateFlagBits`: «specifies that all
//!   commands that modify the created VkPipelineCache will be externally synchronized. When set, the implementation may
//!   skip any unnecessary processing needed to support simultaneous modification from multiple threads where allowed».
//!   Мы создаём кэш БЕЗ флага: одновременное создание пайплайнов из нескольких потоков (фон ISP и поток рендера)
//!   разрешено, замок не нужен.
//! * `vkGetPipelineCacheData`: в описании команды нет ни раздела Host Synchronization, ни требования внешней
//!   синхронизации; единственное условие — «Two calls to vkGetPipelineCacheData with the same parameters must retrieve
//!   the same data» ограничено отсутствием между вызовами команд, меняющих кэш. Значит, выгрузка данных одновременно с
//!   созданием пайплайна в другом потоке допустима, а выгрузка в момент изменения вернёт просто моментальный снимок.
//!   Замок у владельца кэша поэтому не добавлен.

use ash::{Device, vk};

use crate::checksum::fnv1a64;
use crate::error::GpuError;

/// Магическое число файла кэша.
const MAGIC: [u8; 8] = *b"FSTIPCCH";
/// Версия формата файла: меняется при любом изменении таблицы выше.
pub const FORMAT_VERSION: u32 = 1;
/// Размер заголовка файла, байт.
pub const FILE_HEADER_BYTES: usize = 28;
/// Предел размера файла кэша по умолчанию, байт. Сейчас кэш всего продукта ≈ 0,4 МБ (замер I6: 372 КБ с ISP, UI,
/// приборами и снимком); 8 МиБ — запас больше чем в двадцать раз для роста шейдеров и вариантов. Файл больше —
/// запуск с пустым кэшем (кэш копит записи от каждой сборки с изменёнными шейдерами, старые не уходят).
pub const DEFAULT_PIPELINE_CACHE_MAX_BYTES: usize = 8 * 1024 * 1024;

/// Чем плох файл кэша.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileDamage {
    /// Короче заголовка.
    TooShort,
    /// Другое магическое число (чужой файл).
    WrongMagic,
    /// Другая версия формата.
    WrongVersion {
        /// Версия из файла.
        found: u32,
    },
    /// Длина данных в заголовке не равна числу байт после заголовка (обрезок или хвост мусора).
    LengthMismatch,
    /// Контрольная сумма данных не сошлась (перевёрнутый бит и подобное).
    ChecksumMismatch,
}

/// Запечатывает данные драйвера в файл: заголовок + данные.
pub fn seal_cache_file(payload: &[u8]) -> Vec<u8> {
    let mut file = Vec::with_capacity(FILE_HEADER_BYTES + payload.len());
    file.extend_from_slice(&MAGIC);
    file.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    file.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    file.extend_from_slice(&fnv1a64(payload).to_le_bytes());
    file.extend_from_slice(payload);
    file
}

/// Проверяет файл и отдаёт данные драйвера. Любое расхождение — причина [`FileDamage`], данные не отдаются.
pub fn unseal_cache_file(file: &[u8]) -> Result<&[u8], FileDamage> {
    let header = file.get(..FILE_HEADER_BYTES).ok_or(FileDamage::TooShort)?;
    if header[..8] != MAGIC {
        return Err(FileDamage::WrongMagic);
    }
    let version = u32::from_le_bytes(header[8..12].try_into().map_err(|_| FileDamage::TooShort)?);
    if version != FORMAT_VERSION {
        return Err(FileDamage::WrongVersion { found: version });
    }
    let declared = u64::from_le_bytes(header[12..20].try_into().map_err(|_| FileDamage::TooShort)?);
    let checksum = u64::from_le_bytes(header[20..28].try_into().map_err(|_| FileDamage::TooShort)?);
    let payload = &file[FILE_HEADER_BYTES..];
    if declared != payload.len() as u64 {
        return Err(FileDamage::LengthMismatch);
    }
    if fnv1a64(payload) != checksum {
        return Err(FileDamage::ChecksumMismatch);
    }
    Ok(payload)
}

/// Владелец `VkPipelineCache`. Уничтожается раньше устройства (поле контекста объявлено перед устройством).
pub struct PipelineCacheOwner {
    device: Device,
    cache: vk::PipelineCache,
    /// Сколько байт данных отдаёт драйвер СРАЗУ после создания (фактическое состояние кэша, а не размер предложенного
    /// блока: драйвер мог молча проигнорировать блок).
    initial_payload_bytes: usize,
}

/// С чем создан кэш.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PipelineCacheOrigin {
    /// Файла не было.
    #[default]
    Empty,
    /// Драйвер принял блок: сразу после создания он отдаёт данные сопоставимого размера.
    Loaded {
        /// Размер данных драйвера в принятом блоке, байт.
        bytes: usize,
    },
    /// Драйвер вернул успех, но кэш после создания пуст: блок молча проигнорирован (чужой UUID после обновления ОС).
    IgnoredByDriver {
        /// Размер предложенного блока, байт.
        offered_bytes: usize,
    },
    /// Драйвер отверг блок ошибкой: кэш создан пустым.
    RejectedByDriver,
    /// Файл не прошёл нашу проверку (формат, длина, контрольная сумма): драйверу не отдавался.
    DamagedFile(FileDamage),
    /// Файл больше предела: драйверу не отдавался.
    OversizedFile {
        /// Размер файла, байт.
        bytes: usize,
    },
}

impl PipelineCacheOrigin {
    /// Короткое имя для отчётов сценариев (строка результата ограничена по размеру).
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Loaded { .. } => "loaded",
            Self::IgnoredByDriver { .. } => "ignored",
            Self::RejectedByDriver => "rejected",
            Self::DamagedFile(_) => "damaged",
            Self::OversizedFile { .. } => "oversized",
        }
    }
}

impl PipelineCacheOwner {
    /// Создаёт кэш из файла `file` (пусто — чистый кэш) и сообщает, с чем он создан.
    pub(crate) fn new(
        device: &Device,
        file: &[u8],
        max_file_bytes: usize,
    ) -> Result<(Self, PipelineCacheOrigin), vk::Result> {
        if file.is_empty() {
            return Ok((Self::create_empty(device)?, PipelineCacheOrigin::Empty));
        }
        if file.len() > max_file_bytes {
            tracing::warn!(
                target: "fasticamera::gpu",
                bytes = file.len(),
                limit = max_file_bytes,
                "файл кэша пайплайнов больше предела: запуск с пустым кэшем, следующее сохранение заменит файл"
            );
            return Ok((
                Self::create_empty(device)?,
                PipelineCacheOrigin::OversizedFile { bytes: file.len() },
            ));
        }
        let payload = match unseal_cache_file(file) {
            Ok(payload) => payload,
            Err(damage) => {
                tracing::warn!(target: "fasticamera::gpu", ?damage, "файл кэша пайплайнов повреждён: запуск с пустым кэшем");
                return Ok((Self::create_empty(device)?, PipelineCacheOrigin::DamagedFile(damage)));
            }
        };
        let info = vk::PipelineCacheCreateInfo::default().initial_data(payload);
        // SAFETY: устройство валидно; данные живут до конца вызова.
        match unsafe { device.create_pipeline_cache(&info, None) } {
            Ok(cache) => {
                let mut owner = Self {
                    device: device.clone(),
                    cache,
                    initial_payload_bytes: 0,
                };
                // Факт, а не обещание: сколько драйвер реально держит в кэше после создания.
                owner.initial_payload_bytes = owner.payload()?.len();
                let origin = if owner.initial_payload_bytes.saturating_mul(2) < payload.len() {
                    tracing::warn!(
                        target: "fasticamera::gpu",
                        offered = payload.len(),
                        actual = owner.initial_payload_bytes,
                        "драйвер молча проигнорировал сохранённый кэш пайплайнов (другое устройство или драйвер)"
                    );
                    PipelineCacheOrigin::IgnoredByDriver {
                        offered_bytes: payload.len(),
                    }
                } else {
                    PipelineCacheOrigin::Loaded {
                        bytes: owner.initial_payload_bytes,
                    }
                };
                Ok((owner, origin))
            }
            Err(result) => {
                tracing::warn!(target: "fasticamera::gpu", ?result, "драйвер отверг сохранённый кэш пайплайнов: начинаем с пустого");
                Ok((Self::create_empty(device)?, PipelineCacheOrigin::RejectedByDriver))
            }
        }
    }

    fn create_empty(device: &Device) -> Result<Self, vk::Result> {
        let info = vk::PipelineCacheCreateInfo::default();
        // SAFETY: устройство валидно.
        let cache = unsafe { device.create_pipeline_cache(&info, None) }?;
        let mut owner = Self {
            device: device.clone(),
            cache,
            initial_payload_bytes: 0,
        };
        owner.initial_payload_bytes = owner.payload()?.len();
        Ok(owner)
    }

    /// Дескриптор для `vkCreate*Pipelines`.
    pub(crate) fn handle(&self) -> vk::PipelineCache {
        self.cache
    }

    /// Сколько байт данных драйвера в кэше сразу после создания.
    pub(crate) fn initial_payload_bytes(&self) -> usize {
        self.initial_payload_bytes
    }

    /// Данные драйвера как есть (без нашего заголовка).
    fn payload(&self) -> Result<Vec<u8>, vk::Result> {
        // SAFETY: кэш принадлежит self и жив; спецификация не требует внешней синхронизации (см. описание модуля).
        unsafe { self.device.get_pipeline_cache_data(self.cache) }
    }

    /// Запечатанный файл для сохранения на диск.
    pub(crate) fn sealed_file(&self) -> Result<Vec<u8>, GpuError> {
        let payload = self.payload().map_err(|result| GpuError::Vulkan {
            step: "vkGetPipelineCacheData",
            result,
        })?;
        Ok(seal_cache_file(&payload))
    }
}

impl Drop for PipelineCacheOwner {
    fn drop(&mut self) {
        // SAFETY: кэш создан этим устройством и больше не используется (контекст уничтожает его первым).
        unsafe { self.device.destroy_pipeline_cache(self.cache, None) };
    }
}

#[cfg(test)]
#[path = "pipeline_cache_tests.rs"]
mod tests;
