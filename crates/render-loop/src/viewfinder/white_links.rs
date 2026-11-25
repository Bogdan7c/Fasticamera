//! Связка рендера с владельцем белого и реестр источников белого (white-balance 01).
//!
//! Композиция собирает [`WhiteLinks`] один раз и отдаёт бэкенду: четыре части — указания владельца (читаем),
//! ответы источников (пишем), показания источников (пишем; читает экран) и реестр источников. Сам источник — не объект связки, а фабрика в реестре: раннер
//! создаётся заново на каждую сессию показа (surface), и источнику нужно чистое состояние.
//!
//! # Новый источник
//! Один файл с реализацией `scene_estimation::white_source::WhiteSource` и одна строка в корне композиции:
//! `registry.register(|| Ok(Box::new(MySource::new())))`. Ни рендер, ни движок не правятся.

use std::fmt;
use std::sync::Arc;

use camera_contract::three_a::DirectiveSlot;
use camera_contract::white::{WhiteAnswerChannel, WhiteDirective};
use camera_contract::white_reading::WhiteReadingBoard;
use scene_estimation::error::ConfigError;
use scene_estimation::white_source::WhiteSource;
use scene_estimation::white_source::phone::{PhoneWhiteConfig, PhoneWhiteSource};

/// Фабрика не смогла собрать источник (текст причины).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("источник белого не собран: {0}")]
pub struct WhiteSourceBuildError(pub String);

/// Фабрика источника: зовётся на каждую сессию показа.
type SourceFactory = Arc<dyn Fn() -> Result<Box<dyn WhiteSource>, WhiteSourceBuildError> + Send + Sync>;

/// Список фабрик источников белого. Клоны дёшевы и делят фабрики.
#[derive(Clone, Default)]
pub struct WhiteSourceRegistry {
    factories: Vec<SourceFactory>,
}

impl fmt::Debug for WhiteSourceRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WhiteSourceRegistry")
            .field("sources", &self.factories.len())
            .finish()
    }
}

impl WhiteSourceRegistry {
    /// Пустой реестр: любая заявка получит `Unavailable(NotRegistered)`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Реестр с единственным источником «телефон». Числа проверяются сразу, а не на первой сессии показа.
    pub fn with_phone(config: PhoneWhiteConfig) -> Result<Self, ConfigError> {
        config.validate()?;
        let mut registry = Self::new();
        registry.register(move || {
            PhoneWhiteSource::new(config)
                .map(|source| Box::new(source) as Box<dyn WhiteSource>)
                .map_err(|error| WhiteSourceBuildError(error.to_string()))
        });
        Ok(registry)
    }

    /// Регистрирует источник: одна строка на новый источник.
    pub fn register<F>(&mut self, factory: F)
    where
        F: Fn() -> Result<Box<dyn WhiteSource>, WhiteSourceBuildError> + Send + Sync + 'static,
    {
        self.factories.push(Arc::new(factory));
    }

    /// Свежие экземпляры всех источников для новой сессии показа.
    pub fn instantiate(&self) -> Result<Vec<Box<dyn WhiteSource>>, WhiteSourceBuildError> {
        self.factories.iter().map(|factory| factory()).collect()
    }

    /// Сколько источников зарегистрировано.
    pub fn len(&self) -> usize {
        self.factories.len()
    }

    /// Пуст ли реестр.
    pub fn is_empty(&self) -> bool {
        self.factories.is_empty()
    }
}

/// Всё, что связывает рендер с владельцем белого (движок камеры). `Default` — пустая связка: указаний нет
/// (белая временная D65, кадры не прячутся), источников нет.
#[derive(Debug, Clone, Default)]
pub struct WhiteLinks {
    /// Указания владельца (читающая половина; писатель — движок камеры).
    pub directives: DirectiveSlot<WhiteDirective>,
    /// Ответы источников владельцу (пишет рендер, читает и «толкается» движок).
    pub answers: WhiteAnswerChannel,
    /// Показания источников: что каждый видит сейчас (пишет рендер, читает экран; владельцу не идут, WB33).
    pub readings: WhiteReadingBoard,
    /// Источники белого.
    pub sources: WhiteSourceRegistry,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registry_is_empty_and_phone_registry_builds_fresh_instances() {
        assert!(WhiteSourceRegistry::new().is_empty());
        let registry = WhiteSourceRegistry::with_phone(PhoneWhiteConfig::default()).expect("числа верны");
        assert_eq!(registry.len(), 1);
        let first = registry.instantiate().expect("сборка");
        assert_eq!(first[0].id(), camera_contract::white::WhiteSourceId::PHONE);
        assert_eq!(
            registry.instantiate().expect("сборка").len(),
            1,
            "каждая сессия получает свой экземпляр"
        );
    }

    #[test]
    fn invalid_phone_config_is_rejected_up_front_and_a_failing_factory_is_reported() {
        let broken = PhoneWhiteConfig {
            stable_frames: 0,
            ..PhoneWhiteConfig::default()
        };
        assert!(WhiteSourceRegistry::with_phone(broken).is_err());
        let mut registry = WhiteSourceRegistry::new();
        registry.register(|| Err(WhiteSourceBuildError("сломан".to_owned())));
        assert_eq!(
            registry.instantiate().err(),
            Some(WhiteSourceBuildError("сломан".to_owned()))
        );
    }
}
