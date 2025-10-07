//! Загрузка конфигурации из ENV с разумными дефолтами и валидацией.
//!
//! Поддерживаемые переменные окружения:
//! - GEYSER_GRPC_URL          (обязательно)  — адрес Yellowstone gRPC, напр. "http://127.0.0.1:10000"
//! - GEYSER_GRPC_AUTH         (опц.)         — токен/ключ авторизации, если требуется
//! - COMMITMENT               (опц.)         — processed|confirmed|finalized (по умолчанию confirmed)
//! - PROGRAM_PUMPFUN          (опц.)         — Program ID pump.fun (Pubkey строкой)
//! - PROGRAM_PUMPSWAP         (опц.)         — Program ID PumpSwap (Pubkey строкой)
//! - IDL_PUMPFUN_PATH         (опц.)         — путь к idl/pumpfun.json
//! - IDL_PUMPSWAP_PATH        (опц.)         — путь к idl/pumpswap.json
//! - OUTPUT                   (опц.)         — pretty|jsonl (по умолчанию pretty)
//! - RECONNECT_BASE_MS        (опц.)         — стартовая задержка реконнекта, мс (по умолчанию 500)
//! - RECONNECT_MAX_MS         (опц.)         — максимум задержки реконнекта, мс (по умолчанию 10_000)
//! - CHANNEL_CAP              (опц.)         — размер внутреннего канала, по умолчанию 1024
//!
//! Пример использования в main.rs:
//! ```ignore
//! let cfg = Config::load()?;
//! println!("Config loaded: {cfg:#?}");
//! ```

use anyhow::{bail, Context, Result};
use solana_sdk::{
    pubkey::Pubkey,
};
use solana_commitment_config::{CommitmentConfig, CommitmentLevel};
use std::{env, path::PathBuf, str::FromStr};

/// Актуальные Program IDs по умолчанию.
/// При необходимости можно переопределить через ENV.
pub const DEFAULT_PUMPFUN_PROGRAM_ID: &str =
    "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
pub const DEFAULT_PUMPSWAP_PROGRAM_ID: &str =
    "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA";

/// Дефолтные пути к IDL.
pub const DEFAULT_IDL_PUMPFUN_PATH: &str = "idl/pumpfun.json";
pub const DEFAULT_IDL_PUMPSWAP_PATH: &str = "idl/pumpswap.json";

/// Дефолты устойчивости.
pub const DEFAULT_RECONNECT_BASE_MS: u64 = 500;
pub const DEFAULT_RECONNECT_MAX_MS: u64 = 10_000;
pub const DEFAULT_CHANNEL_CAP: usize = 1024;

/// Режим вывода событий.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Pretty,
    Jsonl,
}

impl OutputMode {
    fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "pretty" => Some(Self::Pretty),
            "jsonl" => Some(Self::Jsonl),
            _ => None,
        }
    }
}

/// Уровень подтверждения (commitment) для потоков.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Commitment {
    Processed,
    Confirmed,
    Finalized,
}

impl Commitment {
    fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "processed" => Some(Self::Processed),
            "confirmed" => Some(Self::Confirmed),
            "finalized" => Some(Self::Finalized),
            _ => None,
        }
    }

    /// Конвертация в `solana_sdk::commitment_config::CommitmentConfig`.
    pub fn to_solana(self) -> CommitmentConfig {
        let level = match self {
            Commitment::Processed => CommitmentLevel::Processed,
            Commitment::Confirmed => CommitmentLevel::Confirmed,
            Commitment::Finalized => CommitmentLevel::Finalized,
        };
        CommitmentConfig { commitment: level }
    }
}

/// Итоговая конфигурация приложения.
#[derive(Debug, Clone)]
pub struct Config {
    pub geyser_grpc_url: String,
    pub auth_token: Option<String>,
    pub commitment: Commitment,

    pub program_pumpfun: Pubkey,
    pub program_pumpswap: Pubkey,

    pub idl_pumpfun_path: PathBuf,
    pub idl_pumpswap_path: PathBuf,

    pub output_mode: OutputMode,

    pub reconnect_base_ms: u64,
    pub reconnect_max_ms: u64,
    pub channel_cap: usize,
}

impl Config {
    /// Загружает конфиг из ENV и применяет дефолты, затем валидирует значения.
    pub fn load() -> Result<Self> {
        // URL — обязательный параметр.
        let geyser_grpc_url = must_env("GEYSER_GRPC_URL")
            .context("GEYSER_GRPC_URL is required (e.g. http://127.0.0.1:10000)")?;

        let auth_token = env::var("GEYSER_GRPC_AUTH").ok().filter(|s| !s.is_empty());

        // Commitment с дефолтом.
        let commitment = env::var("COMMITMENT")
            .ok()
            .and_then(|s| Commitment::parse(&s))
            .unwrap_or(Commitment::Confirmed);

        // Program IDs (строки -> Pubkey) с дефолтами.
        let program_pumpfun = parse_pubkey_env_or_default(
            "PROGRAM_PUMPFUN",
            DEFAULT_PUMPFUN_PROGRAM_ID,
            "PROGRAM_PUMPFUN",
        )?;
        let program_pumpswap = parse_pubkey_env_or_default(
            "PROGRAM_PUMPSWAP",
            DEFAULT_PUMPSWAP_PROGRAM_ID,
            "PROGRAM_PUMPSWAP",
        )?;

        // Пути к IDL.
        let idl_pumpfun_path = PathBuf::from(
            env::var("IDL_PUMPFUN_PATH")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| DEFAULT_IDL_PUMPFUN_PATH.to_string()),
        );
        let idl_pumpswap_path = PathBuf::from(
            env::var("IDL_PUMPSWAP_PATH")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| DEFAULT_IDL_PUMPSWAP_PATH.to_string()),
        );

        // Режим вывода.
        let output_mode = env::var("OUTPUT")
            .ok()
            .and_then(|s| OutputMode::parse(&s))
            .unwrap_or(OutputMode::Pretty);

        // Параметры устойчивости.
        let reconnect_base_ms = parse_u64_env("RECONNECT_BASE_MS").unwrap_or(DEFAULT_RECONNECT_BASE_MS);
        let reconnect_max_ms = parse_u64_env("RECONNECT_MAX_MS").unwrap_or(DEFAULT_RECONNECT_MAX_MS);
        let channel_cap = parse_usize_env("CHANNEL_CAP").unwrap_or(DEFAULT_CHANNEL_CAP);

        // Простая валидация: базовая задержка не больше максимальной.
        if reconnect_base_ms == 0 || reconnect_base_ms > reconnect_max_ms {
            bail!("invalid reconnect backoff: RECONNECT_BASE_MS must be > 0 and <= RECONNECT_MAX_MS");
        }

        Ok(Self {
            geyser_grpc_url,
            auth_token,
            commitment,
            program_pumpfun,
            program_pumpswap,
            idl_pumpfun_path,
            idl_pumpswap_path,
            output_mode,
            reconnect_base_ms,
            reconnect_max_ms,
            channel_cap,
        })
    }

    /// Удобный доступ к Solana CommitmentConfig.
    pub fn commitment_config(&self) -> CommitmentConfig {
        self.commitment.to_solana()
    }
}

// ---------- Вспомогательные функции ----------

fn must_env(key: &str) -> Result<String> {
    match env::var(key) {
        Ok(v) if !v.trim().is_empty() => Ok(v),
        _ => bail!("missing or empty ENV: {key}"),
    }
}

fn parse_pubkey_env_or_default(env_key: &str, default: &str, label: &str) -> Result<Pubkey> {
    let s = env::var(env_key).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string());
    Pubkey::from_str(&s).with_context(|| format!("invalid {label}, expected Pubkey, got: {s}"))
}

fn parse_u64_env(key: &str) -> Option<u64> {
    env::var(key).ok()?.trim().parse().ok()
}

fn parse_usize_env(key: &str) -> Option<usize> {
    env::var(key).ok()?.trim().parse().ok()
}
