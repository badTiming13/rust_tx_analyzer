// src/decode/args.rs
//! Borsh-декодинг аргументов инструкций по схемам из Anchor IDL.
//! Толерантен к «укороченным» данным: если байтов не хватает на очередной аргумент,
//! пишем null и продолжаем (полезно для бэковард-совместимости разных версий программ).

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use solana_sdk::pubkey::Pubkey;

use super::idl::{IdlArg, TypeRegistry};

/// Декодирует аргументы инструкции из `data[8..]` по схеме IDL.
/// Возвращает JSON-объект `{arg_name: value, ...}` и число израсходованных байт.
pub fn decode_args_to_json(
    data: &[u8],
    args: &[IdlArg],
    types: &TypeRegistry,
) -> Result<(serde_json::Value, usize)> {
    let mut r = BorshReader::new(data);
    let mut obj = serde_json::Map::new();

    for arg in args {
        // Если байт нет вообще — просто null
        if r.remaining() == 0 {
            obj.insert(arg.name.clone(), serde_json::Value::Null);
            continue;
        }

        // Пытаемся декодить; если не вышло (EOF/нестандарт) — null и дальше
        match decode_type(&mut r, &arg.ty, types) {
            Ok(v) => { obj.insert(arg.name.clone(), v); }
            Err(_e) => {
                // Можно залогировать _e при желании; для стриминга оставим null
                obj.insert(arg.name.clone(), serde_json::Value::Null);
                // Не прерываем цикл — декодим оставшиеся аргументы, если есть байты
            }
        }
    }

    Ok((serde_json::Value::Object(obj), r.consumed()))
}


/// Рекурсивный декодер одного значения по описателю типа из IDL.
fn decode_type(r: &mut BorshReader, ty: &Value, types: &TypeRegistry) -> Result<Value> {
    match ty {
        Value::String(s) => decode_builtin(r, s),
        Value::Object(map) => {
            // {"defined":"TypeName"} | {"option": T} | {"vec": T} | {"array":[T, N]}
            if let Some(def) = map.get("defined") {
                let name = def.as_str().ok_or_else(|| anyhow::anyhow!("defined name must be string"))?;
                decode_defined(r, name, types)
            } else if let Some(opt) = map.get("option") {
                decode_option(r, opt, types)
            } else if let Some(elem_ty) = map.get("vec") {
                decode_vec(r, elem_ty, types)
            } else if let Some(arr) = map.get("array") {
                decode_array(r, arr, types)
            } else {
                bail!("unsupported object type in IDL: {}", ty);
            }
        }
        other => bail!("unsupported IDL type: {}", other),
    }
}

/// Базовые типы по строковым именам.
fn decode_builtin(r: &mut BorshReader, s: &str) -> Result<Value> {
    Ok(match s {
        "bool" => Value::Bool(r.read_u8()? != 0),
        "u8" => Value::from(r.read_u8()? as u64),
        "u16" => Value::from(r.read_u16()? as u64),
        "u32" => Value::from(r.read_u32()? as u64),
        "u64" => Value::String(r.read_u64()?.to_string()),
        "u128" => Value::String(r.read_u128()?.to_string()),
        "i64" => Value::String(r.read_i64()?.to_string()),
        "i128" => Value::String(r.read_i128()?.to_string()),
        "string" => Value::String(r.read_string()?),
        "bytes" => Value::String(hex_string(&r.read_bytes()?)),
        "publicKey" => {
            let pk = r.read_pubkey()?;
            Value::String(pk.to_string())
        }
        other => bail!("unknown builtin type '{}'", other),
    })
}

/// Option<T>: 0 => null, 1 => Some(T)
fn decode_option(r: &mut BorshReader, inner: &Value, types: &TypeRegistry) -> Result<Value> {
    // Если данных уже нет — трактуем как None (null)
    if r.remaining() == 0 {
        return Ok(Value::Null);
    }
    let tag = r.read_u8()?;
    match tag {
        0 => Ok(Value::Null),
        1 => decode_type(r, inner, types),
        // Бывает мусорный/нестандартный тэг из-за несовпадения версий — считаем как None.
        _ => Ok(Value::Null),
    }
}

/// Vec<T>: u32 length + элементы
fn decode_vec(r: &mut BorshReader, elem_ty: &Value, types: &TypeRegistry) -> Result<Value> {
    // Если данных уже нет — пустой вектор (лениентно)
    if r.remaining() == 0 {
        return Ok(Value::Array(vec![]));
    }
    let len = r.read_u32()? as usize;
    let mut arr = Vec::with_capacity(len);
    for _ in 0..len {
        arr.push(decode_type(r, elem_ty, types)?);
    }
    Ok(Value::Array(arr))
}

/// Array<[T; N]>: последовательные элементы
fn decode_array(r: &mut BorshReader, arr: &Value, types: &TypeRegistry) -> Result<Value> {
    // {"array": [T, N]}
    let a = arr.as_array().ok_or_else(|| anyhow::anyhow!("array expects [T, N]"))?;
    if a.len() != 2 {
        bail!("array expects 2 elements [T, N], got {}", a.len());
    }
    let elem_ty = &a[0];
    let n = a[1].as_u64().ok_or_else(|| anyhow::anyhow!("array length N must be u64"))? as usize;

    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        // Если данных нет — заполняем null (лениентно), чтобы выровнять длину
        if r.remaining() == 0 {
            out.push(Value::Null);
        } else {
            out.push(decode_type(r, elem_ty, types)?);
        }
    }
    Ok(Value::Array(out))
}

/// Defined struct: ищем в реестре types по имени.
/// Поддерживаются именованные поля ({name,type}) и tuple-struct (["u8", {...}, ...]).
fn decode_defined(r: &mut BorshReader, name: &str, types: &TypeRegistry) -> Result<Value> {
    let fields = types.get(name).ok_or_else(|| anyhow::anyhow!("unknown defined type '{}'", name))?;
    // Именованные?
    if fields.first().and_then(|v| v.as_object()).and_then(|o| o.get("name")).is_some() {
        // [{name, type}, ...]
        let mut obj = serde_json::Map::new();
        for field in fields {
            // если кончились байты — кладём null
            if r.remaining() == 0 {
                let fname = field
                    .as_object()
                    .and_then(|o| o.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("_unknown");
                obj.insert(fname.to_string(), Value::Null);
                continue;
            }
            let fobj = field.as_object().ok_or_else(|| anyhow::anyhow!("expected named field object"))?;
            let fname = fobj.get("name").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("field.name must be string"))?;
            let fty = fobj.get("type").ok_or_else(|| anyhow::anyhow!("field.type missing"))?;
            let v = decode_type(r, fty, types).with_context(|| format!("in defined '{}.{}'", name, fname))?;
            obj.insert(fname.to_string(), v);
        }
        Ok(Value::Object(obj))
    } else {
        // Tuple-struct: ["bool", {"array":["u8",32]}, ...]
        let mut obj = serde_json::Map::new();
        for (i, fty) in fields.iter().enumerate() {
            if r.remaining() == 0 {
                obj.insert(format!("_{}", i), Value::Null);
            } else {
                let v = decode_type(r, fty, types).with_context(|| format!("in defined '{}' tuple field {}", name, i))?;
                obj.insert(format!("_{}", i), v);
            }
        }
        Ok(Value::Object(obj))
    }
}

/// Маленький reader поверх среза для Borsh.
struct BorshReader<'a> {
    data: &'a [u8],
    off: usize,
}
impl<'a> BorshReader<'a> {
    fn new(data: &'a [u8]) -> Self { Self { data, off: 0 } }
    fn remaining(&self) -> usize { self.data.len().saturating_sub(self.off) }
    fn consumed(&self) -> usize { self.off }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            bail!("unexpected EOF: need {}, have {}", n, self.remaining());
        }
        let s = &self.data[self.off..self.off + n];
        self.off += n;
        Ok(s)
    }

    fn read_u8(&mut self) -> Result<u8> { Ok(self.take(1)?[0]) }
    fn read_u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn read_u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn read_u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }
    fn read_u128(&mut self) -> Result<u128> {
        let b = self.take(16)?;
        Ok(u128::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13],
            b[14], b[15],
        ]))
    }
    fn read_i64(&mut self) -> Result<i64> {
        let b = self.take(8)?;
        Ok(i64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }
    fn read_i128(&mut self) -> Result<i128> {
        let b = self.take(16)?;
        Ok(i128::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13],
            b[14], b[15],
        ]))
    }

    fn read_string(&mut self) -> Result<String> {
        let len = self.read_u32()? as usize;
        let b = self.take(len)?;
        let s = std::str::from_utf8(b).context("invalid UTF-8 in string")?;
        Ok(s.to_string())
    }

    /// Borsh bytes: Vec<u8> (u32 length + bytes)
    fn read_bytes(&mut self) -> Result<Vec<u8>> {
        let len = self.read_u32()? as usize;
        let b = self.take(len)?;
        Ok(b.to_vec())
    }

    fn read_pubkey(&mut self) -> Result<Pubkey> {
        let b = self.take(32)?;
        let mut arr = [0u8; 32];
        arr.copy_from_slice(b);
        Ok(Pubkey::new_from_array(arr))
    }
}

/// Небольшой helper: bytes → "0x…"
fn hex_string(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push_str("0x");
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}
