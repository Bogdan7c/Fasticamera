//! Подача гравитации экрану с прореживанием (сессия 12).
//!
//! Датчик гравитации быстрее глаза, а уровню экрана хватает ~30 Гц. Подача читает ящик датчика раз в
//! `camera_screen.gravity_every_n_frames` кадров рендера и отдаёт вектор только если пришло НОВОЕ показание
//! (иначе экран зря пересчитывал бы тот же уровень). Читает ящик без ожидания — это безопасно на потоке рендера.

use shell_contract::gravity::{GravityReading, GravitySubscription, GravityVector};
use shell_contract::motion::SensorSourceError;

/// Что дала подача на этом кадре.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GravityPoll {
    /// Это кадр между опросами (прореживание).
    NotDue,
    /// Датчик ещё не прислал ни одного показания.
    NoSampleYet,
    /// Новых показаний с прошлого опроса нет.
    Unchanged,
    /// Новое показание: вектор для экрана.
    Fresh(GravityVector),
}

/// Подача гравитации. Держит подписку: датчик работает, пока жива подача.
pub struct GravityFeed {
    subscription: GravitySubscription,
    every_n_frames: u32,
    frames_since_poll: u32,
    last_seen_count: Option<u64>,
}

impl GravityFeed {
    /// `every_n_frames` ≥ 1 проверено конфигом; 0 трактуется как 1 (опрос каждый кадр), чтобы не делить на ноль.
    pub fn new(subscription: GravitySubscription, every_n_frames: u32) -> Self {
        Self {
            subscription,
            every_n_frames: every_n_frames.max(1),
            // Первый кадр опрашивает сразу: экран получает позу без ожидания N кадров.
            frames_since_poll: every_n_frames.max(1),
            last_seen_count: None,
        }
    }

    /// Вызывается один раз на кадр.
    pub fn poll(&mut self) -> GravityPoll {
        self.frames_since_poll = self.frames_since_poll.saturating_add(1);
        if self.frames_since_poll < self.every_n_frames {
            return GravityPoll::NotDue;
        }
        self.frames_since_poll = 0;
        match self.subscription.slot().read() {
            GravityReading::NoSampleYet => GravityPoll::NoSampleYet,
            GravityReading::Latest {
                sample,
                samples_received,
            } => {
                if self.last_seen_count == Some(samples_received) {
                    return GravityPoll::Unchanged;
                }
                self.last_seen_count = Some(samples_received);
                GravityPoll::Fresh(sample.vector)
            }
        }
    }
}

/// Запуск подачи: ошибка датчика — явный итог, экран живёт без наклона.
pub fn start_gravity_feed(
    result: Result<GravitySubscription, SensorSourceError>,
    every_n_frames: u32,
) -> Result<GravityFeed, String> {
    result
        .map(|subscription| GravityFeed::new(subscription, every_n_frames))
        .map_err(|error| error.to_string())
}

#[cfg(test)]
#[path = "camera_screen_gravity_tests.rs"]
mod tests;
