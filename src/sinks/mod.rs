use anyhow::Result;

use crate::types::DecodedEvent;
use crate::config::OutputMode; // ← в конфиге этот тип

pub trait Sink {
    fn write(&mut self, ev: &DecodedEvent) -> Result<()>;
}

pub mod console;
pub use console::ConsoleSink;

/// Фабрика, чтобы совпадало с вызовом в `main.rs`: `sinks::make_sink(cfg.output_mode)`
pub fn make_sink(_mode: OutputMode) -> ConsoleSink {
    // Пока у нас один тип приёмника — консоль.
    ConsoleSink::new()
}
