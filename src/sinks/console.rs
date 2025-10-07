use anyhow::Result;
use serde_json::{Map, Value};
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::{collections::HashMap, str::FromStr};

use crate::{
    mint::MintInfoCache,
    sinks::Sink,
    types::DecodedEvent,
};

pub struct ConsoleSink {
    mint_cache: MintInfoCache,
    rpc: Option<RpcClient>,
}

impl ConsoleSink {
    pub fn new() -> Self {
        let rpc_url = std::env::var("RPC_URL")
            .ok()
            .or_else(|| std::env::var("SOLANA_RPC_URL").ok());
        let rpc = rpc_url.map(RpcClient::new);
        Self {
            mint_cache: MintInfoCache::new(),
            rpc,
        }
    }
}

impl Default for ConsoleSink {
    fn default() -> Self {
        Self::new()
    }
}

impl Sink for ConsoleSink {
    fn write(&mut self, ev: &DecodedEvent) -> Result<()> {
        // 1) Копируем поля для обогащения
        let mut fields = match &ev.fields {
            Value::Object(m) => m.clone(),
            _ => Map::new(),
        };

        // 2) Собираем mint-ы
        let mints = collect_mints(&fields);

        // 3) Узнаём decimals
        let base_dec = mints
            .get("base")
            .and_then(|pk| self.mint_cache.get_decimals(pk, self.rpc.as_ref()));
        let quote_dec = mints
            .get("quote")
            .and_then(|pk| self.mint_cache.get_decimals(pk, self.rpc.as_ref()));
        let generic_mint_dec = mints
            .get("mint")
            .and_then(|pk| self.mint_cache.get_decimals(pk, self.rpc.as_ref()));

        // 4) Добавляем *_ui
        add_ui_if_amount(&mut fields, "amount", generic_mint_dec.or(base_dec).or(quote_dec));
        add_ui_if_amount(&mut fields, "base_amount_in", base_dec);
        add_ui_if_amount(&mut fields, "base_amount_out", base_dec);
        add_ui_if_amount(&mut fields, "quote_amount_in", quote_dec);
        add_ui_if_amount(&mut fields, "quote_amount_out", quote_dec);
        add_ui_if_amount(&mut fields, "min_quote_amount_out", quote_dec);
        add_ui_if_amount(&mut fields, "max_quote_amount_in", quote_dec);

        // pump.fun — SOL
        add_ui_if_amount(&mut fields, "min_sol_output", Some(9));
        add_ui_if_amount(&mut fields, "max_sol_cost", Some(9));

        // эвристики
        let keys: Vec<String> = fields.keys().cloned().collect();
        for k in keys {
            let dec = if k.contains("sol") {
                Some(9)
            } else if k.starts_with("base_") {
                base_dec
            } else if k.starts_with("quote_") {
                quote_dec
            } else {
                None
            };
            if let Some(d) = dec {
                add_ui_if_amount(&mut fields, &k, Some(d));
            }
        }

        // 5) Подпись: у нас `solana_sdk::signature::Signature`
        let sig_b58 = ev.signature.to_string();

        // 6) Печать
        let fields_json = Value::Object(fields);
        println!(
            "slot={} sig={} program={} name={} inner={} fields={}",
            ev.slot, sig_b58, ev.program_id, ev.name, ev.is_inner, fields_json
        );

        Ok(())
    }
}

fn collect_mints(fields: &Map<String, Value>) -> HashMap<&'static str, Pubkey> {
    let mut out = HashMap::new();
    if let Some(pk) = parse_pk_field(fields.get("acc_base_mint")) {
        out.insert("base", pk);
    }
    if let Some(pk) = parse_pk_field(fields.get("acc_quote_mint")) {
        out.insert("quote", pk);
    }
    if let Some(pk) = parse_pk_field(fields.get("acc_mint")) {
        out.insert("mint", pk);
    }
    out
}

fn parse_pk_field(v: Option<&Value>) -> Option<Pubkey> {
    let s = v?.as_str()?;
    Pubkey::from_str(s).ok()
}

fn add_ui_if_amount(fields: &mut Map<String, Value>, key: &str, decimals: Option<u8>) {
    let dec = match decimals {
        Some(d) => d,
        None => return,
    };
    if let Some(raw) = fields.get(key).and_then(|v| to_u128(v)) {
        let ui = u128_to_ui_string(raw, dec);
        fields.insert(format!("{key}_ui"), Value::String(ui));
    }
}

fn to_u128(v: &Value) -> Option<u128> {
    match v {
        Value::String(s) => s.parse::<u128>().ok(),
        Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                Some(u as u128)
            } else if let Some(i) = n.as_i64() {
                (i >= 0).then(|| i as u128)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn u128_to_ui_string(x: u128, dec: u8) -> String {
    if dec == 0 {
        return x.to_string();
    }
    let scale = 10u128.saturating_pow(dec as u32);
    let int_part = x / scale;
    let frac_part = x % scale;

    if frac_part == 0 {
        return int_part.to_string();
    }

    let mut frac = format!("{:0width$}", frac_part, width = dec as usize);
    while frac.ends_with('0') {
        frac.pop();
    }
    format!("{int_part}.{frac}")
}
