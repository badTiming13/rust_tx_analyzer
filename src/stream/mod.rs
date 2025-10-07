//! Поток из Yellowstone gRPC → нормализованный контекст → декодер → sink.

pub mod geyser;

use anyhow::Result;

use crate::{config::Config, decode::DecoderRegistry, sinks::Sink};

/// Главный цикл: подключение к gRPC, сбор контекста, декод и вывод.
pub async fn run<S: Sink>(cfg: Config, registry: DecoderRegistry, mut sink: S) -> Result<()> {
    geyser::run_pipeline(&cfg, &registry, &mut sink).await
}
