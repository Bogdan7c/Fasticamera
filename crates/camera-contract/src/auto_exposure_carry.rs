//! Перенос экспозиции между камерами (сессия 13b, решения 118 и 126): как рендер и движок
//! различают, **на какой именно камере** посчитано произведение «выдержка × ISO».
//!
//! Две камеры телефона стримят одновременно (тёплая вторая камера), и их метки времени
//! `SENSOR_TIMESTAMP` перемешиваются на одной шкале `BOOTTIME`. Поэтому по времени одно предложение
//! AE от другого не отличить; нужна явная метка камеры. Ящики [`crate::three_a::DirectiveSlot`]
//! хранят только `Copy`-значения, а [`CameraId`] — строка, поэтому ходит компактный отпечаток
//! [`CameraTag`].

use serde::Serialize;

use crate::auto_exposure::ExposureProduct;
use crate::ids::CameraId;

/// Компактный `Copy`-отпечаток камеры: «эти два числа посчитаны на одной и той же камере».
///
/// Это отпечаток (FNV-1a, 64 бита, не зависит от платформы и запуска), а не идентификатор: он нужен
/// только для сравнения «та же камера или нет» между рендером и движком. Телефон отдаёт единицы
/// камер с короткими id («0», «2», «3»), вероятность совпадения отпечатков двух разных камер
/// пренебрежима, а ошибка по совпадению стоила бы одного отвергнутого или принятого предложения AE.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct CameraTag(u64);

impl CameraTag {
    /// Начальное значение FNV-1a (64 бита).
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    /// Простое число FNV-1a (64 бита).
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    /// Отпечаток камеры `camera`.
    pub fn of(camera: &CameraId) -> Self {
        let mut hash = Self::FNV_OFFSET_BASIS;
        for byte in camera.as_str().bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(Self::FNV_PRIME);
        }
        Self(hash)
    }
}

/// Какая экспозиция ожидается у НОВОГО потока ещё до его первого собственного замера.
///
/// Движок публикует её в [`crate::auto_exposure::ExposureDirective::stream_start`] **до** смены
/// потока (обмен тёплой камеры или переоткрытие): он помнит цель прошлой камеры и умеет пересчитать её
/// на новую (`exposure_transfer`, решение 126). Рендер по ней проверяет правдоподобие яркости первого
/// кадра нового потока и не держит прежнюю картинку, пока AE ещё не измерил новую сцену.
///
/// Оценка действует, пока у потока нет собственного замера, и только для кадров камеры `camera`:
/// для любой другой камеры она бессмысленна.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CarriedSceneExposure {
    /// Камера, для которой посчитано `target`.
    pub camera: CameraTag,
    /// Произведение «выдержка × ISO», при котором средний серый сцены ложится в цель тона при 0 EV на
    /// этой камере (то же, что `ExposureProposal::target`, только оценённое, а не измеренное).
    pub target: ExposureProduct,
}

impl CarriedSceneExposure {
    /// Оценка для камеры `camera`.
    pub fn for_camera(camera: &CameraId, target: ExposureProduct) -> Self {
        Self {
            camera: CameraTag::of(camera),
            target,
        }
    }

    /// Относится ли оценка к камере `camera`.
    pub fn is_for(&self, camera: &CameraId) -> bool {
        self.camera == CameraTag::of(camera)
    }
}

#[cfg(test)]
#[path = "auto_exposure_carry_tests.rs"]
mod tests;
