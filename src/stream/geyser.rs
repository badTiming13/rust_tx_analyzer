// src/stream/geyser.rs
use anyhow::Result;
use futures::StreamExt;
use std::collections::HashMap;

use solana_sdk::{pubkey::Pubkey, signature::Signature};

use yellowstone_grpc_client::{ClientTlsConfig, GeyserGrpcClient};
use yellowstone_grpc_proto::geyser::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
    SubscribeRequestFilterTransactions,
};
use yellowstone_grpc_proto::prelude::{
    CompiledInstruction, InnerInstruction, InnerInstructions, Message,
};

use crate::{
    config::{Commitment as AppCommitment, Config},
    decode::DecoderRegistry,
    sinks::Sink,
    types::{InstrRef, TxContext},
};

/// Полный пайплайн: коннект → подписка на транзакции pumpfun/pumpswap → сбор контекста → декод → вывод.
pub async fn run_pipeline<S: Sink>(
    cfg: &Config,
    registry: &DecoderRegistry,
    sink: &mut S,
) -> Result<()> {
    // 1) Сборка клиента
    let mut builder = GeyserGrpcClient::build_from_shared(cfg.geyser_grpc_url.clone())?;
    if let Some(token) = &cfg.auth_token {
        builder = builder.x_token(Some(token.clone()))?;
    }
    if cfg.geyser_grpc_url.starts_with("https://") {
        let tls = ClientTlsConfig::new().with_native_roots();
        builder = builder.tls_config(tls)?;
    }
    let mut client = builder.connect().await?;

    // 2) Подписка на ТРАНЗАКЦИИ с фильтрами по программам
    let mut req = SubscribeRequest::default();
    req.set_commitment(match cfg.commitment {
        AppCommitment::Processed => CommitmentLevel::Processed,
        AppCommitment::Confirmed => CommitmentLevel::Confirmed,
        AppCommitment::Finalized => CommitmentLevel::Finalized,
    });

    // OR между фильтрами (ключи произвольные)
    let mut tx_filters: HashMap<String, SubscribeRequestFilterTransactions> = HashMap::new();

    let mut pumpfun = SubscribeRequestFilterTransactions::default();
    pumpfun.account_required.push(cfg.program_pumpfun.to_string());
    tx_filters.insert("pumpfun".to_string(), pumpfun);

    let mut pumpswap = SubscribeRequestFilterTransactions::default();
    pumpswap.account_required.push(cfg.program_pumpswap.to_string());
    tx_filters.insert("pumpswap".to_string(), pumpswap);

    req.transactions = tx_filters;

    // 3) Читаем стрим и обрабатываем каждую транзакцию
    let mut stream = client.subscribe_once(req).await?;
    println!("✅ pipeline running: decoding pumpfun/pumpswap instructions…");

    while let Some(update) = stream.next().await {
        let Ok(update) = update else {
            break;
        };

        let Some(UpdateOneof::Transaction(tx)) = update.update_oneof else {
            continue;
        };

        let Some(info) = tx.transaction else {
            continue;
        };

        // Подпись из bytes
        let Some(signature) = signature_from_bytes(&info.signature) else {
            continue;
        };

        // Meta: статус, логи, inner instructions (Vec<InnerInstructions>)
        let (status_ok, logs, inner_groups): (bool, Vec<String>, Vec<InnerInstructions>) =
            if let Some(meta) = info.meta.as_ref() {
                (meta.err.is_none(), meta.log_messages.clone(), meta.inner_instructions.clone())
            } else {
                (true, Vec::new(), Vec::new())
            };

        // Message: ключи и outer instructions
        let (account_keys, outer_instructions) = if let Some(txn) = info.transaction.as_ref() {
            if let Some(message) = txn.message.as_ref() {
                (extract_account_keys(message), message.instructions.clone())
            } else {
                (Vec::new(), Vec::new())
            }
        } else {
            (Vec::new(), Vec::new())
        };

        if account_keys.is_empty() {
            continue;
        }

        // Собираем контекст
        let mut ctx = TxContext {
            slot: tx.slot,
            signature,
            block_time: None, // можно заполнять из meta, если сервер шлёт
            status_ok,
            logs,
            outer_instructions: Vec::new(),
            inner_instructions: Vec::new(),
        };

        // Outer → InstrRef
        for ix in &outer_instructions {
            if let Some(instr_ref) = compiled_to_instr_ref(ix, &account_keys, false) {
                ctx.outer_instructions.push(instr_ref);
            }
        }

        // Inner → Vec<InnerInstructions> → Vec<InnerInstruction>
        for group in &inner_groups {
            for inner_ix in &group.instructions {
                if let Some(instr_ref) = inner_to_instr_ref(inner_ix, &account_keys) {
                    ctx.inner_instructions.push(instr_ref);
                }
            }
        }

        // Декод и вывод
        emit_decoded(&ctx, registry, sink)?;
    }

    Ok(())
}

/// Преобразовать CompiledInstruction + список account_keys → наш InstrRef.
fn compiled_to_instr_ref(
    ix: &CompiledInstruction,
    keys: &[Pubkey],
    is_inner: bool,
) -> Option<InstrRef> {
    let pi = ix.program_id_index as usize;
    let program_id = *keys.get(pi)?;
    let accounts: Vec<Pubkey> = ix
        .accounts
        .iter()
        .filter_map(|&i| keys.get(i as usize).copied())
        .collect();
    let data = ix.data.clone();
    Some(InstrRef { program_id, accounts, data, is_inner })
}

/// Преобразовать InnerInstruction напрямую → наш InstrRef (поля аналогичны CompiledInstruction).
fn inner_to_instr_ref(ix: &InnerInstruction, keys: &[Pubkey]) -> Option<InstrRef> {
    let pi = ix.program_id_index as usize;
    let program_id = *keys.get(pi)?;
    let accounts: Vec<Pubkey> = ix
        .accounts
        .iter()
        .filter_map(|&i| keys.get(i as usize).copied())
        .collect();
    let data = ix.data.clone();
    Some(InstrRef { program_id, accounts, data, is_inner: true })
}

/// Извлечь account_keys из protobuf Message как Vec<Pubkey>.
fn extract_account_keys(message: &Message) -> Vec<Pubkey> {
    message
        .account_keys
        .iter()
        .filter_map(|bytes| pubkey_from_bytes(bytes))
        .collect()
}

/// bytes → Pubkey (ожидаем 32 байта)
fn pubkey_from_bytes(v: &Vec<u8>) -> Option<Pubkey> {
    if v.len() != 32 {
        return None;
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(v);
    Some(Pubkey::new_from_array(arr))
}

/// bytes → Signature (поддерживается TryFrom<&[u8]>)
fn signature_from_bytes(v: &Vec<u8>) -> Option<Signature> {
    Signature::try_from(v.as_slice()).ok()
}

/// Пробег по outer+inner и вызов декодера реестра; успешные события — в sink.
fn emit_decoded<S: Sink>(
    ctx: &TxContext,
    registry: &DecoderRegistry,
    sink: &mut S,
) -> Result<()> {
    for instr in &ctx.outer_instructions {
        if let Some(ev) = registry.decode(instr, ctx) {
            sink.write(&ev)?;
        }
    }
    for instr in &ctx.inner_instructions {
        if let Some(ev) = registry.decode(instr, ctx) {
            sink.write(&ev)?;
        }
    }
    Ok(())
}
