mod config;
mod decode;
mod sinks;
mod stream;
mod types;
mod mint;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // 1) Загружаем конфиг (ты уже создал src/config.rs).
    let cfg = config::Config::load()?;
    println!("▶ config loaded: {:?}", (
        &cfg.geyser_grpc_url,
        cfg.commitment,
        cfg.program_pumpfun,
        cfg.program_pumpswap,
        &cfg.idl_pumpfun_path,
        &cfg.idl_pumpswap_path,
        cfg.output_mode,
    ));

    // 2) Загружаем IDL и собираем реестр декодеров.
    let registry = decode::load_all_idl(&cfg)?;
    for (pid, label) in registry.supported_programs() {
        println!("decoder registered: {} ({})", pid, label);
    }

    // 3) Инициализируем приёмник (консоль).
    let sink = sinks::make_sink(cfg.output_mode);

    // 4) Запускаем стрим (пока заглушка).
    stream::run(cfg, registry, sink).await
}
