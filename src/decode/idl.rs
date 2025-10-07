//! Утилиты для работы с Anchor IDL (устойчивые к разным форматам).
//! Поддержка instructions.args, instructions.accounts (вкл. вложенные),
//! types (struct/enum, включая tuple-struct), и таблицы дискриминаторов.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fs, path::Path};

/// Минимальная модель Anchor IDL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Idl {
    pub name: String,
    #[serde(default, rename = "instructions")]
    pub instructions: Vec<IdlInstruction>,
    #[serde(default)]
    pub types: Vec<IdlTypeDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdlInstruction {
    pub name: String,
    #[serde(default)]
    pub args: Vec<IdlArg>,
    /// Список аккаунтов (может быть вложенным деревом).
    #[serde(default)]
    pub accounts: Vec<IdlAccountNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdlAccountNode {
    pub name: String,
    /// Вложенные аккаунты (Anchor группирует их для удобства).
    #[serde(default)]
    pub accounts: Vec<IdlAccountNode>,
    // Игнорируем остальные поля (isMut, isSigner, pda, relations, etc.)
    #[serde(flatten)]
    pub _rest: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdlArg {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: serde_json::Value,
}

/// Anchor type definition (из секции `types` в IDL).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdlTypeDef {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: IdlTypeDefTy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdlTypeDefTy {
    pub kind: String, // "struct" | "enum"
    #[serde(default)]
    pub fields: Vec<Value>,      // именованные и tuple-struct
    #[serde(default)]
    pub variants: Vec<Value>,    // enum не используем тут
}

/// Реестр определённых типов: имя → список полей (raw JSON).
pub type TypeRegistry = HashMap<String, Vec<Value>>;

/// Метаданные инструкции в таблице дискриминаторов.
#[derive(Debug, Clone)]
pub struct IdlIxMeta {
    pub name: String,
    pub args: Vec<IdlArg>,
    pub account_names: Vec<String>, // имена аккаунтов в порядке, как в runtime
}

/// Загрузка IDL из файла (устойчива к обёрткам).
pub fn load_idl_from_path<P: AsRef<Path>>(path: P) -> Result<Idl> {
    let path_ref = path.as_ref();
    let data = fs::read(path_ref)
        .with_context(|| format!("failed to read IDL file: {}", path_ref.display()))?;
    let root: Value = serde_json::from_slice(&data)
        .with_context(|| format!("failed to parse IDL JSON: {}", path_ref.display()))?;

    let idl_val = find_idl_object(&root).with_context(|| {
        let keys = root
            .as_object()
            .map(|m| m.keys().cloned().collect::<Vec<_>>());
        format!(
            "could not find Anchor IDL object (must contain 'instructions') in {}. Top-level keys: {:?}",
            path_ref.display(),
            keys
        )
    })?;

    let name = idl_val
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            idl_val
                .get("metadata")
                .and_then(|m| m.get("name"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

    let instructions: Vec<IdlInstruction> = idl_val
        .get("instructions")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();

    let types: Vec<IdlTypeDef> = idl_val
        .get("types")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();

    Ok(Idl { name, instructions, types })
}

/// Рекурсивный поиск объекта, у которого есть поле "instructions".
fn find_idl_object(v: &Value) -> Option<Value> {
    if v.get("instructions").is_some() {
        return Some(v.clone());
    }
    let candidate_keys = ["idl", "result", "value", "program", "data"];
    if let Some(obj) = v.as_object() {
        for key in candidate_keys {
            if let Some(inner) = obj.get(key) {
                if let Some(found) = find_idl_object(inner) {
                    return Some(found);
                }
            }
        }
    }
    if let Some(arr) = v.as_array() {
        for item in arr {
            if let Some(found) = find_idl_object(item) {
                return Some(found);
            }
        }
    }
    None
}

/// Дискриминатор Anchor: sha256(b"global:<name>")[0..8]
pub fn anchor_discriminator_for(ix_name: &str) -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update(format!("global:{ix_name}").as_bytes());
    let digest = hasher.finalize();
    let mut out = [0u8; 8];
    out.copy_from_slice(&digest[..8]);
    out
}

/// Построить карту дискриминатор → {name, args, account_names}
pub fn build_discriminator_map(idl: &Idl) -> HashMap<[u8; 8], IdlIxMeta> {
    let mut map = HashMap::with_capacity(idl.instructions.len());
    for ix in &idl.instructions {
        let disc = anchor_discriminator_for(&ix.name);
        let mut account_names = Vec::new();
        flatten_accounts(&ix.accounts, &mut account_names);
        map.insert(
            disc,
            IdlIxMeta {
                name: ix.name.clone(),
                args: ix.args.clone(),
                account_names,
            },
        );
    }
    map
}

/// Собрать список имён аккаунтов в порядке вызова (DFS по вложенным группам).
fn flatten_accounts(nodes: &[IdlAccountNode], out: &mut Vec<String>) {
    for n in nodes {
        out.push(n.name.clone());
        if !n.accounts.is_empty() {
            flatten_accounts(&n.accounts, out);
        }
    }
}

/// Реестр определённых типов (только struct): имя → список полей (raw JSON).
pub fn build_type_registry(idl: &Idl) -> TypeRegistry {
    let mut reg = TypeRegistry::new();
    for td in &idl.types {
        if td.ty.kind == "struct" {
            reg.insert(td.name.clone(), td.ty.fields.clone());
        }
    }
    reg
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discriminator_is_stable() {
        let d = anchor_discriminator_for("initialize");
        assert_eq!(d.len(), 8);
        assert!(d.iter().any(|&b| b != 0));
    }
}
