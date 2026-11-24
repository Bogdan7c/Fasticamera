//! Реестр источников белого, который корень композиции отдаёт рендеру (план white-balance, WB8).
//!
//! Новый источник (нейросеть и т. п.) встаёт одной строкой регистрации здесь, без правок движка и рендера:
//! `registry.register(|| Ok(Box::new(MySource::new())))`. Сегодня источник один — «телефон» (штатная автоматика
//! белого), его числа берутся из конфига рендера `viewfinder.white_balance.phone`.

use render_loop::config::RenderLoopConfig;
use render_loop::viewfinder::WhiteSourceRegistry;

const LOG_TARGET: &str = "fasticamera::white";

/// Реестр источников продукта. Числа «телефона» уже проверены `RenderLoopConfig::validate`; если всё же неверны
/// (в обход проверки), реестр остаётся пустым и каждая заявка получит «источника нет» — белая держится, ошибка
/// громко в журнале, приложение не падает.
pub fn product_white_sources(render: &RenderLoopConfig) -> WhiteSourceRegistry {
    match WhiteSourceRegistry::with_phone(render.viewfinder.white_balance.phone) {
        Ok(registry) => registry,
        Err(error) => {
            tracing::error!(target: LOG_TARGET, %error, "источник белого «телефон» не собран: заявки получат «источника нет»");
            WhiteSourceRegistry::new()
        }
    }
}

#[cfg(test)]
#[path = "white_sources_tests.rs"]
mod tests;
