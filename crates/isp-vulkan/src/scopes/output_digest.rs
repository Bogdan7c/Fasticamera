//! Отпечаток выхода приборов (стенд насыщения и диагностика): работает в ЛЮБОЙ сборке, в том числе в release-APK.
//!
//! Раньше счётчики и картинку прибора можно было прочитать только под фичей `test-support`, поэтому на телефоне
//! стенд отдавал `unavailable`. Здесь владелец состояния ([`ColorScopes`]) сам отвечает узким методом-намерением
//! «дай отпечаток последней записи»; внутренние поля (буфер счётчиков, картинка, форма сетки) наружу не выдаются.
//!
//! Ожидание GPU здесь — суть (блокирующее чтение). На горячем пути этого метода нет: зовут его после конца
//! прогона и `wait_idle`. Чтение ничего не меняет в состоянии приборов.

use ash::vk;
use gpu_vulkan::saturation::{OutputDigest, digest_device_buffer_prefix, digest_storage_image};

use super::color_scopes::ColorScopes;
use super::error::ScopeError;

/// Размер одного счётчика в буфере, байт (`uint` шейдера накопления).
const COUNTER_BYTES: usize = std::mem::size_of::<u32>();

/// Отпечаток выхода приборов: две независимые части, чтобы расхождение сразу показывало, где оно возникло.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeOutputDigest {
    /// Отпечаток ячеек сетки последней записи (валидны ровно `cell_count` счётчиков, остальной буфер не читается).
    pub counters: OutputDigest,
    /// Отпечаток картинки прибора (байты fp16 как лежат в памяти, содержимое целиком).
    pub picture: OutputDigest,
}

impl ColorScopes {
    /// Блокирующе читает выход последней записи и считает его отпечаток.
    ///
    /// Контракт: записанный `record` уже завершился на GPU (вызывающий дождался fence или `wait_idle`).
    /// Ошибка [`ScopeError::OutputDigest`], если кадров ещё не записано, картинки нет или чтение не удалось.
    pub fn digest_last_output_blocking(&self) -> Result<ScopeOutputDigest, ScopeError> {
        let grid = self.last_recorded_grid().ok_or_else(|| ScopeError::OutputDigest {
            what: "кадров приборов ещё не записано".to_owned(),
        })?;
        let counters = digest_device_buffer_prefix(self.counters_for_readback(), grid.cell_count() * COUNTER_BYTES)
            .map_err(|error| ScopeError::OutputDigest {
                what: format!("счётчики: {error}"),
            })?;
        let target = self.picture_for_readback().ok_or_else(|| ScopeError::OutputDigest {
            what: "картинка прибора ещё не создана".to_owned(),
        })?;
        let content = vk::Extent2D {
            width: target.size().width(),
            height: target.size().height(),
        };
        let picture = digest_storage_image(target.storage(), target.current_use(), content).map_err(|error| {
            ScopeError::OutputDigest {
                what: format!("картинка прибора: {error}"),
            }
        })?;
        Ok(ScopeOutputDigest { counters, picture })
    }
}
