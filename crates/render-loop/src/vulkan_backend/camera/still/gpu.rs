//! Узкая граница GPU-части снимка: всё, что движку снимков нужно от GPU, и ничего про Vulkan.
//!
//! Движок ([`super::engine::StillEngine`]) владеет ОЧЕРЕДЬЮ и протоколом (выбор кадра, итоги, сроки); GPU владеет
//! железом: импорт буфера камеры, копия RAW, проявление по полосам, показ в поверхность кодера. Разделение даёт
//! две вещи: логику очереди проверяют быстрые тесты на поддельном GPU, а настоящая реализация
//! ([`super::vulkan_gpu`]) проверяется отдельным стендом.
//!
//! # Контракт реализации
//! * Ничего не ждёт GPU, кроме [`StillGpu::abandon`] (ограниченное ожидание, потом учтённая утечка).
//! * Сбой самого опроса GPU (fence не опросился: потеря устройства) — НЕ обычная ошибка, а «исход неизвестен»:
//!   реализация возвращает `Lost`, а движок объявляет GPU потерянным ([`StillGpu::declare_lost`]) и оставляет всё,
//!   что GPU мог ещё читать или писать, в учтённой утечке (не уничтожает).
//! * Одна копия RAW, одно проявление и один показ в кодер «в полёте» одновременно; запуск при занятом слоте — ошибка
//!   движка (он проверяет `*_in_flight`).
//! * Лизинг кадра, отданный в [`StillGpu::capture_start`], реализация возвращает камере САМА: после fence копии
//!   (`release_after_gpu_fence`) или сразу при отказе (`release_unused`: GPU кадр не читал).

use camera_contract::frame_passport::StreamPassport;
use camera_contract::geometry::PixelRect;
use isp_model::params::IspParams;
use isp_vulkan::photo::RgbaReadback;
use raw_frame_stream::RawFrameLease;
use shell_contract::surface::NativeSurface;
use still_contract::shot::{RawSamples16, Rgba8Image};

/// Итог опроса копии RAW.
#[derive(Debug)]
pub enum CapturePoll<Raw> {
    /// Копия не запускалась (или итог уже выдан).
    NothingInFlight,
    /// GPU ещё работает.
    Pending,
    /// Копия готова; кадр уже возвращён камере.
    Ready(Raw),
    /// Копия сорвалась; GPU закончил работу (fence сигнален), кадр возвращён камере.
    Failed(String),
    /// Опрос fence копии не удался: исход на GPU неизвестен. Кадр камере НЕ возвращён (учтённая утечка лизинга,
    /// решение 43), буфер копии оставлен. GPU снимков потерян.
    Lost(String),
}

/// Что нужно, чтобы начать проявление.
pub struct DevelopStart<'a, Raw> {
    pub raw: &'a Raw,
    pub params: &'a IspParams,
    /// Область снимка в координатах буфера кадра.
    pub crop: PixelRect,
    /// Нужна ли копия готовой картинки в память (JPEG).
    pub readback: RgbaReadback,
}

/// Где проявление.
#[derive(Debug)]
pub enum DevelopProgress<Image> {
    /// Проявление не запущено (или итог уже выдан).
    Idle,
    /// Проявление идёт: часть полос отправлена и/или GPU ещё работает.
    InFlight {
        /// Сколько НОВЫХ полос отправлено этим вызовом (не больше запрошенного `max_new_slices`).
        submitted_now: u32,
    },
    /// Проявление закончено.
    Done(Image),
    /// Проявление сорвалось (проявитель сброшен, GPU закончил всё отправленное).
    Failed(String),
    /// Опрос fence полосы не удался: исход на GPU неизвестен (полосы могут ещё писать в картинку). GPU потерян.
    Lost(String),
}

/// Итог опроса показа в поверхность кодера.
#[derive(Debug, PartialEq, Eq)]
pub enum PresentProgress {
    /// Показ не запускался (или итог уже выдан).
    Idle,
    /// GPU ещё работает.
    Pending,
    /// Картинка показана, поверхность кодера отпущена.
    Done,
    /// Показ сорвался; GPU закончил копирование (картинку можно уничтожать).
    Failed(String),
    /// Опрос fence показа не удался: исход на GPU неизвестен (копия картинки может ещё идти). GPU потерян.
    Lost(String),
}

/// Отказ запустить показ в кодер: поверхность уничтожена реализацией.
#[derive(Debug, PartialEq, Eq)]
pub struct PresentRefusal(pub String);

/// Итог [`StillGpu::abandon`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbandonReport {
    /// GPU подтверждённо закончил всю работу снимков за отведённое время (иначе объекты намеренно утекают).
    pub gpu_idle: bool,
}

/// GPU-часть снимка.
pub trait StillGpu: Send + Sized {
    /// Принадлежащая снимку копия RAW. Дёшево клонируется (общий владелец буфера).
    type Raw: Clone + Send;
    /// Проявленная картинка снимка (держит память GPU).
    type Image: Send;

    /// Годится ли этот GPU для кадров потока (размер полезной области не больше рассчитанного).
    fn serves(&self, stream: &StreamPassport) -> bool;

    /// Идёт ли копия RAW.
    fn capture_in_flight(&self) -> bool;
    /// Идёт ли проявление.
    fn develop_in_flight(&self) -> bool;
    /// Идёт ли показ в кодер.
    fn present_in_flight(&self) -> bool;

    /// Запускает копию полезной области кадра. Лизинг принимается в любом случае (см. модуль).
    fn capture_start(&mut self, lease: RawFrameLease) -> Result<(), String>;
    /// Опрос копии без ожидания.
    fn capture_poll(&mut self) -> CapturePoll<Self::Raw>;
    /// Сырые отсчёты копии для DNG.
    fn raw_samples(&self, raw: &Self::Raw) -> Result<RawSamples16, String>;

    /// Начинает проявление (память и привязки; GPU не трогает).
    fn develop_start(&mut self, request: DevelopStart<'_, Self::Raw>) -> Result<(), String>;
    /// Отправляет не больше `max_new_slices` новых полос и опрашивает завершённые.
    fn develop_advance(&mut self, max_new_slices: u32) -> DevelopProgress<Self::Image>;
    /// Забирает копию картинки в памяти (если просили `RgbaReadback::ForJpeg`).
    fn take_readback(&self, image: &mut Self::Image) -> Option<Rgba8Image>;

    /// Начинает показ картинки в поверхность кодера. Поверхность принимается в любом случае.
    fn present_start(&mut self, image: &Self::Image, surface: NativeSurface) -> Result<(), PresentRefusal>;
    /// Опрос показа без ожидания.
    fn present_poll(&mut self) -> PresentProgress;

    /// Объявляет GPU потерянным БЕЗ ожидания: fence не опросился или не сигналит в срок, ждать дальше нечего.
    /// Всё, что GPU мог ещё читать или писать (лизинг кадра камеры, буфер копии, цель кодера, импорты), остаётся в
    /// учтённой утечке; лизинг камере не возвращается. После вызова движок выбрасывает GPU целиком (`forget`) и
    /// пересоздаёт его для следующего снимка. Идемпотентен.
    fn declare_lost(&mut self);

    /// Остановка: отменяет проявление и ждёт (ограниченно) завершения всей отправленной работы.
    fn abandon(&mut self) -> AbandonReport;
}
