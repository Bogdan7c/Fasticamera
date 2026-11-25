//! Источник белого: контракт «какой здесь свет» (white-balance 01, решение WB8).
//!
//! Источник **не знает**, когда его спрашивают и применят ли ответ: это решает владелец белого в движке камеры.
//! Исполняется на потоке рендера, рядом с кадрами; ответ может прийти через много кадров (нейросеть на HTP/GPU),
//! поэтому форма — «заявка / забрать позже», а заявку можно отменить.
//!
//! # Новый источник
//! Один файл с реализацией [`WhiteSource`] и строка регистрации в корне композиции. Ни владелец белого, ни
//! исполнитель источников в рендере, ни словарь `camera-contract::white` не правятся.
//!
//! # Вход — по заявке источника
//! [`WhiteSource::required_input`] называет, что источнику нужно. Подсказки цвета результата есть у кадра и так;
//! плитки статистики и уменьшенный линейный кадр готовятся, только если их заявили. Пока не заявил никто, рендер их
//! не готовит и не платит; источник с такой заявкой получает честный отказ `InputNotProvided`.

use camera_contract::auto_exposure_carry::CameraTag;
use camera_contract::capture_result::SensorTimestamp;
use camera_contract::control::ExposureTime;
use camera_contract::frame_passport::CaptureColorHints;
use camera_contract::white::{WhiteRequestId, WhiteSourceId, WhiteSourceOutcome, WhiteUnavailableReason};
use camera_contract::white_reading::WhiteReading;
use isp_model::color::dng::DngColorProfile;
use isp_model::statistics::AwbStatistics;

pub mod phone;

/// Что источнику нужно от кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WhiteInputKind {
    /// Подсказки цвета из результата кадра: бесплатно.
    ColorHints,
    /// Плитки статистики RAW (общие с экспозицией).
    SceneTiles,
    /// Уменьшенный линейный кадр RAW RGB: после уровней, до баланса.
    LinearThumbnail,
}

/// Уменьшенный линейный кадр (RAW RGB после уровней, до баланса), строками сверху вниз.
#[derive(Debug, Clone, Copy)]
pub struct LinearThumbnail<'a> {
    pub width: u32,
    pub height: u32,
    /// `width × height` троек R, G, B.
    pub pixels: &'a [[f32; 3]],
}

/// Факты потока камеры: постоянны, пока поток не сменился.
#[derive(Debug, Clone, Copy)]
pub struct WhiteStreamFacts<'a> {
    /// Камера (линза) потока.
    pub camera: CameraTag,
    /// Цветовой профиль потока; `None` — пригодной калибровки нет.
    pub profile: Option<&'a DngColorProfile>,
}

/// Один кадр глазами источника. Кадры, освещённые нашим светодиодом, и кадры без собственного результата сюда не
/// попадают: их свет — не свет сцены.
#[derive(Debug, Clone, Copy)]
pub struct WhiteFrame<'a> {
    pub sensor_timestamp: SensorTimestamp,
    pub exposure_time: Option<ExposureTime>,
    pub sensitivity_iso: Option<u32>,
    pub color_hints: &'a CaptureColorHints,
    /// Есть, только если источник заявил [`WhiteInputKind::SceneTiles`].
    pub tiles: Option<&'a AwbStatistics>,
    /// Есть, только если источник заявил [`WhiteInputKind::LinearThumbnail`].
    pub thumbnail: Option<LinearThumbnail<'a>>,
}

/// Почему источник не принял заявку.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhiteRequestRefusal {
    /// Занят прежней заявкой.
    Busy,
    /// Ответить не сможет вовсе.
    Unavailable(WhiteUnavailableReason),
}

/// Готовый ответ источника.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WhiteReply {
    pub request: WhiteRequestId,
    /// Кадр, по которому дан ответ.
    pub measured_frame: Option<SensorTimestamp>,
    pub outcome: WhiteSourceOutcome,
}

/// Источник белого.
///
/// Порядок вызовов: [`Self::begin_stream`] на каждый новый поток, [`Self::observe`] на каждый годный кадр потока
/// (и без заявки: автоматике телефона нужно видеть, что значение свежее), [`Self::request`] — когда владелец
/// спросил. Молчание — законный ответ «ещё считаю»: срок ожидания держит владелец.
pub trait WhiteSource: Send {
    fn id(&self) -> WhiteSourceId;

    /// Что источнику нужно от кадра.
    fn required_input(&self) -> WhiteInputKind;

    /// Начался новый поток: всё, что накоплено по прежнему, забыто. Незакрытая заявка снимается без ответа —
    /// владелец выдаст новому потоку новую.
    fn begin_stream(&mut self, stream: &WhiteStreamFacts<'_>);

    /// Очередной кадр потока.
    fn observe(&mut self, frame: &WhiteFrame<'_>);

    /// Заявка владельца; ответ — через [`Self::poll`].
    fn request(&mut self, request: WhiteRequestId) -> Result<(), WhiteRequestRefusal>;

    /// Заявка больше не нужна; неизвестный номер — ничего не делать.
    fn cancel(&mut self, request: WhiteRequestId);

    /// Забирает готовый ответ; `None` — готовых нет.
    fn poll(&mut self) -> Option<WhiteReply>;

    /// Что источник видит прямо сейчас (white-balance 02, WB33): показание для экрана «что я получу, если нажму».
    ///
    /// **Не ответ.** Заявки у показания нет, его не ждут и не опознают по номеру, владелец белого его не видит, а
    /// состояние источника (заявки, накопленное по потоку) вызов не меняет. Ответ — единственное, что может
    /// стать белой в картинке; показание лишь рассказывает, каким был бы ответ сейчас. Белая прошлого сеанса или
    /// иное значение, не измеренное на текущем потоке, показанием не бывает: это
    /// [`WhiteReading::NotMeasuredYet`]. Показание не требует устойчивости, которой ждёт ответ.
    fn reading(&self) -> WhiteReading;
}
