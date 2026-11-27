//! «Линейка» паритета: насколько картинка видоискателя на GPU совпадает с CPU-эталоном `isp-model`
//! (сессия 14o, решение владельца о переносе линейки из тестов в библиотеку).
//!
//! # Зачем в библиотеке, а не в тестах
//! На ПК паритет проверяют тесты `tests/viewfinder_*parity*.rs`, но поля допусков тонкие (полосы оттенка 14a —
//! 9,5e-6 при допуске 1e-5; колёса цвета 14i — 5,2e-6), а компилятор шейдеров Adreno иначе склеивает `fma`, чем
//! RADV. Поэтому ту же линейку надо уметь запускать НА ТЕЛЕФОНЕ: приложение (`gpu-saturation-bench`, группа
//! `look_parity`) считает эталон на CPU телефона для того же синтетического RAW и тех же параметров и сообщает
//! «ошибку математики сверх шага хранения» против допуска.
//!
//! # Почему модуль лежит в `saturation`
//! Здесь уже живёт всё, что нужно для измерений на устройстве без тестовых фич: синтетический RAW10 на обычной
//! памяти Vulkan ([`HostRawFrame`](gpu_vulkan::saturation::HostRawFrame)), блокирующая отправка и чтение
//! картинки. Модуль нейтрален: он знает `IspParams` и `ViewfinderIsp`, но не знает ни приложения, ни рецептов вида.
//!
//! # Раскладка
//! * `allowance` — допуски ([`ErrorAllowance`], [`MATH_EXCESS_TOLERANCE`], шаги 10-битного выхода);
//! * `metrics` — числа сверки ([`Parity`]) и шаг хранения формата картинки;
//! * `frame` — синтетический RAW-кадр ([`SyntheticRawFrame`]) и его мозаика для эталона;
//! * `scenes` — худшие по смыслу сцены (серая шкала, полосы, пересветы, около чёрного, развёртка по оттенку,
//!   кожа и пастели, яркие насыщенные);
//! * `ruler` — сама линейка [`ParityRuler`]: GPU в двух форматах хранения против эталона, итог [`ParityMeasure`].

mod allowance;
mod frame;
mod metrics;
mod ruler;
mod scenes;

pub use self::allowance::{ErrorAllowance, HALF_LSB, MATH_EXCESS_TOLERANCE, ONE_LSB};
pub use self::frame::SyntheticRawFrame;
pub use self::metrics::{Parity, compare_with_reference, storage_step};
pub use self::ruler::{ParityError, ParityMeasure, ParityRuler};
pub use self::scenes::{
    HEIGHT_OF_SYNTHETIC_SCENES, SceneCalibration, WIDTH_OF_SYNTHETIC_SCENES, bright_saturated_scene, hsv_to_rgb,
    hue_sweep_light, hue_sweep_scene, look_scenes, skin_and_pastels_light, skin_and_pastels_scene, synthetic_scenes,
    tone_scenes,
};
