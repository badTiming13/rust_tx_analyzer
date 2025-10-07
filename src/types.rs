use solana_sdk::{pubkey::Pubkey, signature::Signature};

#[derive(Debug, Clone)]
pub struct TxContext {
    pub slot: u64,
    pub signature: Signature,
    pub block_time: Option<i64>,
    pub status_ok: bool,
    pub logs: Vec<String>,
    pub outer_instructions: Vec<InstrRef>,
    pub inner_instructions: Vec<InstrRef>,
}

#[derive(Debug, Clone)]
pub struct InstrRef {
    pub program_id: Pubkey,
    pub accounts: Vec<Pubkey>,
    pub data: Vec<u8>,
    pub is_inner: bool,
}

#[derive(Debug, Clone)]
pub struct DecodedEvent {
    pub program_id: Pubkey,
    pub name: String,
    pub fields: serde_json::Value, // ключи/значения из IDL
    pub signature: Signature,
    pub slot: u64,
    pub is_inner: bool,
}

impl DecodedEvent {
    /// Удобное JSON-представление для jsonl-вывода (строки вместо бинарных типов).
    pub fn to_json_object(&self) -> serde_json::Value {
        serde_json::json!({
            "slot": self.slot,
            "signature": self.signature.to_string(),
            "program_id": self.program_id.to_string(),
            "name": self.name,
            "is_inner": self.is_inner,
            "fields": self.fields,
        })
    }
}
