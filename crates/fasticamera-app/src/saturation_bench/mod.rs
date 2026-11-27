//! Стенд насыщения приложения (сессия 16a, P4b): сколько на самом деле стоит работа GPU.
//!
//! # Зачем (простыми словами)
//! На Adreno 740 драйвер сам занижает частоту GPU, когда работы мало, поэтому «миллисекунды в темпе vsync» не
//! говорят о цене работы. Честная цена — пропускная способность при насыщении: кадры работы идут подряд без
//! показа, цена кадра = 1 / (кадров в секунду). Этот модуль прогоняет набор работ [`catalog::entries`] одну за
//! другой на СВОЁМ `GpuContext` и собирает таблицу «работа → к/с → мс/кадр → проходы → шкала → тепло».
//!
//! # Раскладка
//! * [`catalog`] — имена и виды работ (данные, GPU не нужен);
//! * [`compare_pairs`], [`pair_comparison`] — пары «эталон и вариант»: отчёт об отличии картинок (max |Δ|, гистограмма);
//! * [`look_variant`], [`look_parity_run`], [`parity_report`] — виды картинки для групп `look_cost` и `look_parity` (14o):
//!   рецепты из рецепта продукта, прогон линейки паритета `isp_vulkan::saturation::parity` на устройстве и её итог в строке;
//! * [`extras`] — дополнительные факты строки (паритет, размер картинки прибора, созданные пайплайны);
//! * [`digest_report`] — дайджест выхода работы в итоге строки («картинка не изменилась» на телефоне);
//! * [`catalog_wait`] — ожидание каталога камер движка перед сборкой входов (причина запасного сенсора на телефоне);
//! * [`draw_filter`] — фильтр списка рисования для абляций прохода UI (чистая функция);
//! * [`report`] — типы итога строки и вердикт прогона;
//! * [`runner`] — порядок прогона: работы по одной, пауза, отмена, тепло, события; НЕ знает GPU
//!   (за него отвечает [`runner::WorkloadExecutor`], подставной в тестах);
//! * [`inputs`] — «параметры как у продукта»: сенсор, вид, основа тона, карта теней;
//! * [`screen_scene`] — настоящие списки рисования экрана камеры (PHOTO, CALIBRATE);
//! * [`backdrop`] — фон кадра показа из картинки ISP;
//! * [`gpu_executor`] — исполнитель на настоящем GPU (сборка работ, стенд, освобождение, проверка утечек);
//! * [`viewfinder_variant`] — вариант видоискателя каталога → стадии и каденция AWB работы;
//! * [`services`] — шов «откуда сценарию взять GPU-контекст стенда» (на телефоне — долгоживущий контекст
//!   диагностик, не контекст на запуск).
//!
//! Продуктовое поведение модуль не меняет: оптимизаций здесь нет, только замеры.

pub mod backdrop;
pub mod catalog;
pub mod catalog_wait;
pub mod compare_pairs;
pub mod digest_report;
pub mod draw_filter;
pub mod extras;
pub mod gpu_executor;
pub mod headroom_tuning;
pub mod headroom_variant;
pub mod inputs;
pub mod look_parity_run;
pub mod look_variant;
pub mod pair_comparison;
pub mod parity_report;
pub mod report;
pub mod runner;
pub mod screen_scene;
pub mod services;
pub mod still_variant;
pub mod summary;
#[cfg(test)]
mod test_glass;
pub mod viewfinder_tuning;
pub mod viewfinder_variant;
