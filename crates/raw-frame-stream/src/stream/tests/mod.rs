//! Тесты потока кадров. Все идут через публичный API (`RawFrameStream` и его ручки) с
//! поддельным производителем, который записывает, что и с какими fence-ами ему вернули.

mod concurrency;
mod distribution;
mod epochs;
mod pairing;
mod pool_and_flow;
mod release;
mod support;
