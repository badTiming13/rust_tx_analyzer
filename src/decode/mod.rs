//! Декодирование инструкций по IDL.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde_json::{Map, Value};
use solana_sdk::pubkey::Pubkey;

use crate::{
    config::Config,
    types::{DecodedEvent, InstrRef, TxContext},
};

pub mod idl;
use idl::{
    build_discriminator_map, build_type_registry, load_idl_from_path, IdlIxMeta, TypeRegistry,
};

mod args;
use args::decode_args_to_json;

/// Интерфейс декодеров
pub trait Decoder: Send + Sync {
    fn label(&self) -> &'static str;
    fn program_id(&self) -> Pubkey;
    fn decode(&self, instr: &InstrRef, ctx: &TxContext) -> Option<DecodedEvent>;
}

/// Реестр
pub struct DecoderRegistry {
    by_program: HashMap<Pubkey, Box<dyn Decoder>>,
}

impl DecoderRegistry {
    pub fn new() -> Self { Self { by_program: HashMap::new() } }
    pub fn register(&mut self, d: Box<dyn Decoder>) { self.by_program.insert(d.program_id(), d); }
    pub fn decode(&self, instr: &InstrRef, ctx: &TxContext) -> Option<DecodedEvent> {
        self.by_program.get(&instr.program_id)?.decode(instr, ctx)
    }
    #[allow(dead_code)]
    pub fn supported_programs(&self) -> Vec<(Pubkey, &'static str)> {
        self.by_program.iter().map(|(k, v)| (*k, v.label())).collect()
    }
}

/// Загрузка IDL и регистрация декодеров
pub fn load_all_idl(cfg: &Config) -> Result<DecoderRegistry> {
    let pumpfun_idl = load_idl_from_path(&cfg.idl_pumpfun_path)
        .with_context(|| format!("loading pumpfun IDL from {}", cfg.idl_pumpfun_path.display()))?;
    let pumpfun_disc = build_discriminator_map(&pumpfun_idl);
    let pumpfun_types = build_type_registry(&pumpfun_idl);

    let pumpswap_idl = load_idl_from_path(&cfg.idl_pumpswap_path)
        .with_context(|| format!("loading pumpswap IDL from {}", cfg.idl_pumpswap_path.display()))?;
    let pumpswap_disc = build_discriminator_map(&pumpswap_idl);
    let pumpswap_types = build_type_registry(&pumpswap_idl);

    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(AnchorDecoder::new(
        cfg.program_pumpfun,
        "pump.fun",
        pumpfun_disc,
        pumpfun_types,
    )));
    reg.register(Box::new(AnchorDecoder::new(
        cfg.program_pumpswap,
        "PumpSwap",
        pumpswap_disc,
        pumpswap_types,
    )));
    Ok(reg)
}

/// Декодер Anchor-программ
struct AnchorDecoder {
    program_id: Pubkey,
    label: &'static str,
    by_disc: HashMap<[u8; 8], IdlIxMeta>,
    types: TypeRegistry,
}

impl AnchorDecoder {
    fn new(
        program_id: Pubkey,
        label: &'static str,
        by_disc: HashMap<[u8; 8], IdlIxMeta>,
        types: TypeRegistry,
    ) -> Self {
        Self { program_id, label, by_disc, types }
    }
}

impl Decoder for AnchorDecoder {
    fn label(&self) -> &'static str { self.label }
    fn program_id(&self) -> Pubkey { self.program_id }

    fn decode(&self, instr: &InstrRef, ctx: &TxContext) -> Option<DecodedEvent> {
        if instr.program_id != self.program_id || instr.data.len() < 8 {
            return None;
        }

        // 1) дискриминатор
        let mut disc = [0u8; 8];
        disc.copy_from_slice(&instr.data[..8]);

        // 2) метаданные из IDL
        let meta = self.by_disc.get(&disc)?;

        // 3) распарсим аргументы
        let fields_json = match decode_args_to_json(&instr.data[8..], &meta.args, &self.types) {
            Ok((v, _)) => v,
            Err(_) => Value::Object(Map::new()),
        };

        // 4) добавим имена аккаунтов из IDL → "acc_<name>": "<pubkey>"
        let mut merged = match fields_json {
            Value::Object(m) => m,
            _ => Map::new(),
        };

        let n = meta.account_names.len().min(instr.accounts.len());
        for i in 0..n {
            let key = format!("acc_{}", meta.account_names[i]);
            merged.insert(key, Value::String(instr.accounts[i].to_string()));
        }

        Some(DecodedEvent {
            program_id: self.program_id,
            name: meta.name.clone(),
            fields: Value::Object(merged),
            signature: ctx.signature,
            slot: ctx.slot,
            is_inner: instr.is_inner,
        })
    }
}
