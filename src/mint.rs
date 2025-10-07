use anyhow::{Context, Result};
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::str::FromStr;

/// Данные по mint-у, пока нужен только decimals.
#[derive(Debug, Clone, Copy)]
pub struct MintInfo {
    pub decimals: u8,
}

/// Простой кэш mint-ов.
#[derive(Default)]
pub struct MintInfoCache {
    map: HashMap<Pubkey, MintInfo>,
}

impl MintInfoCache {
    pub fn new() -> Self {
        Self { map: HashMap::new() }
    }

    /// Вернуть decimals для mint-а. Если нет в кэше — попробует сходить в RPC.
    /// Если `rpc` = None, вернёт None (кроме известных кейсов типа WSOL).
    pub fn get_decimals(&mut self, mint: &Pubkey, rpc: Option<&RpcClient>) -> Option<u8> {
        // Известные кейсы: native SOL / WSOL → 9
        if is_native_like_mint(mint) {
            return Some(9);
        }

        if let Some(mi) = self.map.get(mint) {
            return Some(mi.decimals);
        }

        let rpc = rpc?;
        match fetch_mint_decimals(rpc, mint) {
            Ok(dec) => {
                self.map.insert(*mint, MintInfo { decimals: dec });
                Some(dec)
            }
            Err(_) => None,
        }
    }
}

/// Попробовать достать decimals по RPC и распарсить из данных аккаунта.
fn fetch_mint_decimals(rpc: &RpcClient, mint: &Pubkey) -> Result<u8> {
    let acc = rpc
        .get_account(mint)
        .with_context(|| format!("get_account({mint}) failed"))?;

    // Стандартный SPL Token Mint layout: decimals лежит по смещению 44.
    // Проверим размер и вытащим байт.
    if acc.data.len() < 45 {
        anyhow::bail!("mint account too small: {} bytes", acc.data.len());
    }
    Ok(acc.data[44])
}

/// WSOL и несколько «особых» значений → считаем 9 знаков.
fn is_native_like_mint(mint: &Pubkey) -> bool {
    // Wrapped SOL (WSOL)
    let wsol = Pubkey::from_str("So11111111111111111111111111111111111111112").ok();
    // Часто попадается «111111...», это не mint, но чтобы не падать
    let all_ones = Pubkey::from_str("11111111111111111111111111111111").ok();

    Some(*mint) == wsol || Some(*mint) == all_ones
}
