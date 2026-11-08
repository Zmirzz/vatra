//! GGUF header parsing. Reads metadata only; tensor infos are not parsed.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_HEADER: usize = 64 << 20;
const MAX_KV_COUNT: u64 = 1_000_000;
const MAX_INLINE_ARRAY: u64 = 64;
/// The most array values a header keeps in all; later arrays are skipped.
const MAX_KEPT_VALUES: u64 = 1 << 17;
const MAX_DEPTH: usize = 16;
const MAX_LAYERS: u64 = 1 << 16;

#[derive(Debug, Error, PartialEq)]
pub enum GgufError {
    #[error("the header continues past the bytes read so far")]
    NeedMore { at_least: usize },
    #[error("not a valid GGUF header: {0}")]
    Invalid(String),
    #[error("the GGUF header is larger than 64 MiB")]
    TooLarge,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GgufHeader {
    pub version: u32,
    pub tensor_count: u64,
    pub kv: BTreeMap<String, GgufValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GgufValue {
    U8(u8),
    I8(i8),
    U16(u16),
    I16(i16),
    U32(u32),
    I32(i32),
    F32(f32),
    Bool(bool),
    String(String),
    U64(u64),
    I64(i64),
    F64(f64),
    Array(Vec<GgufValue>),
    /// A long array whose elements were walked but not stored.
    ArraySkipped {
        elem_type: u32,
        len: u64,
    },
}

impl GgufValue {
    /// Any integer type that is not negative. Bool is not a number here.
    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Self::U8(v) => Some(v.into()),
            Self::U16(v) => Some(v.into()),
            Self::U32(v) => Some(v.into()),
            Self::U64(v) => Some(v),
            Self::I8(v) => u64::try_from(v).ok(),
            Self::I16(v) => u64::try_from(v).ok(),
            Self::I32(v) => u64::try_from(v).ok(),
            Self::I64(v) => u64::try_from(v).ok(),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }
}

/// Parses the header at the start of `buf`. Tensor infos after the metadata are ignored.
pub fn parse(buf: &[u8]) -> Result<GgufHeader, GgufError> {
    if buf.iter().zip(b"GGUF").any(|(a, b)| a != b) {
        return Err(GgufError::Invalid("bad magic".into()));
    }
    let mut r = Reader { buf, pos: 0, kept: 0 };
    r.take(4)?;
    let version = r.u32()?;
    if !matches!(version, 2 | 3) {
        return Err(GgufError::Invalid(format!("unsupported version {version}")));
    }
    let tensor_count = r.u64()?;
    let kv_count = r.u64()?;
    if kv_count > MAX_KV_COUNT {
        return Err(GgufError::Invalid(format!("{kv_count} metadata entries")));
    }
    let mut kv = BTreeMap::new();
    for _ in 0..kv_count {
        let key = r.string()?;
        let ty = r.u32()?;
        // These arrays hold one entry per layer, so they can be longer than the inline limit.
        let keep =
            key.ends_with(".attention.head_count_kv") || key.ends_with(".attention.head_count");
        let value = r.value(ty, 0, keep)?;
        kv.insert(key, value);
    }
    Ok(GgufHeader {
        version,
        tensor_count,
        kv,
    })
}

/// Model facts the rest of the crate needs, pulled out of the header.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelFacts {
    pub architecture: Option<String>,
    pub name: Option<String>,
    pub size_label: Option<String>,
    pub file_type: Option<u32>,
    pub context_length: Option<u64>,
    pub block_count: Option<u64>,
    pub embedding_length: Option<u64>,
    pub head_count: Option<u64>,
    /// One entry per layer. A scalar is repeated `block_count` times.
    pub head_count_kv: Vec<u32>,
    pub key_length: Option<u64>,
    pub value_length: Option<u64>,
    pub sliding_window: Option<u64>,
    pub expert_count: Option<u64>,
    pub expert_used_count: Option<u64>,
    pub kv_lora_rank: Option<u64>,
    pub rope_dim: Option<u64>,
    pub recurrent: bool,
    pub chat_template_mentions_tools: bool,
    pub has_chat_template: bool,
}

impl ModelFacts {
    pub fn from_header(h: &GgufHeader) -> ModelFacts {
        let text = |key: &str| h.kv.get(key).and_then(GgufValue::as_str).map(str::to_owned);
        let architecture = text("general.architecture");
        let arch_value = |suffix: &str| {
            let arch = architecture.as_deref()?;
            h.kv.get(&format!("{arch}.{suffix}"))
        };
        let arch_num = |suffix: &str| arch_value(suffix).and_then(GgufValue::as_u64);

        let block_count = arch_num("block_count");
        let template = text("tokenizer.chat_template");
        let recurrent = architecture
            .as_deref()
            .is_some_and(|arch| h.kv.keys().any(|k| k.starts_with(&format!("{arch}.ssm."))));

        ModelFacts {
            name: text("general.name"),
            size_label: text("general.size_label"),
            file_type: h
                .kv
                .get("general.file_type")
                .and_then(GgufValue::as_u64)
                .and_then(|v| u32::try_from(v).ok()),
            context_length: arch_num("context_length"),
            embedding_length: arch_num("embedding_length"),
            head_count: arch_value("attention.head_count").and_then(head_count_max),
            head_count_kv: arch_value("attention.head_count_kv")
                .map(|v| per_layer_heads(v, block_count))
                .unwrap_or_default(),
            key_length: arch_num("attention.key_length"),
            value_length: arch_num("attention.value_length"),
            sliding_window: arch_num("attention.sliding_window"),
            expert_count: arch_num("expert_count"),
            expert_used_count: arch_num("expert_used_count"),
            kv_lora_rank: arch_num("attention.kv_lora_rank"),
            rope_dim: arch_num("rope.dimension_count"),
            recurrent,
            chat_template_mentions_tools: template
                .as_deref()
                .is_some_and(|t| t.contains("tools") || t.contains("tool_call")),
            has_chat_template: template.is_some(),
            block_count,
            architecture,
        }
    }
}

/// The largest value of an array, or the scalar itself.
fn head_count_max(v: &GgufValue) -> Option<u64> {
    match v {
        GgufValue::Array(items) => items.iter().filter_map(GgufValue::as_u64).max(),
        other => other.as_u64(),
    }
}

fn per_layer_heads(v: &GgufValue, block_count: Option<u64>) -> Vec<u32> {
    let as_u32 = |v: &GgufValue| v.as_u64().and_then(|n| u32::try_from(n).ok());
    match v {
        GgufValue::Array(items) => items.iter().filter_map(as_u32).collect(),
        other => match (as_u32(other), block_count) {
            (Some(n), Some(b)) if b <= MAX_LAYERS => vec![n; b as usize],
            (Some(n), _) => vec![n],
            (None, _) => Vec::new(),
        },
    }
}

fn fixed_size(ty: u32) -> Option<u64> {
    match ty {
        0 | 1 | 7 => Some(1),
        2 | 3 => Some(2),
        4..=6 => Some(4),
        10..=12 => Some(8),
        _ => None,
    }
}

/// Smallest encoded size of one value of this type. None for unknown types.
fn min_size(ty: u32) -> Option<u64> {
    match ty {
        8 => Some(8),
        9 => Some(12),
        _ => fixed_size(ty),
    }
}

fn unknown_type(ty: u32) -> GgufError {
    GgufError::Invalid(format!("unknown value type {ty}"))
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    /// Array values stored so far, against MAX_KEPT_VALUES.
    kept: u64,
}

impl<'a> Reader<'a> {
    /// End offset of `len` bytes at the cursor. Fails before anything is read if it passes the cap.
    fn end_after(&self, len: u64) -> Result<usize, GgufError> {
        if len > MAX_HEADER as u64 {
            return Err(GgufError::TooLarge);
        }
        let end = self.pos + len as usize;
        if end > MAX_HEADER {
            return Err(GgufError::TooLarge);
        }
        Ok(end)
    }

    /// Checks that `len` bytes at the cursor are buffered, without consuming them.
    fn ensure(&self, len: u64) -> Result<usize, GgufError> {
        let end = self.end_after(len)?;
        if end > self.buf.len() {
            return Err(GgufError::NeedMore { at_least: end });
        }
        Ok(end)
    }

    fn take(&mut self, len: u64) -> Result<&'a [u8], GgufError> {
        let end = self.ensure(len)?;
        let bytes = self
            .buf
            .get(self.pos..end)
            .ok_or(GgufError::NeedMore { at_least: end })?;
        self.pos = end;
        Ok(bytes)
    }

    fn le<const N: usize>(&mut self) -> Result<[u8; N], GgufError> {
        let bytes = self.take(N as u64)?;
        <[u8; N]>::try_from(bytes).map_err(|_| GgufError::Invalid("short read".into()))
    }

    fn u32(&mut self) -> Result<u32, GgufError> {
        Ok(u32::from_le_bytes(self.le()?))
    }

    fn u64(&mut self) -> Result<u64, GgufError> {
        Ok(u64::from_le_bytes(self.le()?))
    }

    fn string(&mut self) -> Result<String, GgufError> {
        let len = self.u64()?;
        Ok(String::from_utf8_lossy(self.take(len)?).into_owned())
    }

    /// Reads an array's element type and count, and checks its smallest possible size fits.
    fn array_header(&mut self, depth: usize) -> Result<(u32, u64), GgufError> {
        if depth >= MAX_DEPTH {
            return Err(GgufError::Invalid("arrays nested too deeply".into()));
        }
        let elem = self.u32()?;
        let len = self.u64()?;
        let min = min_size(elem).ok_or_else(|| unknown_type(elem))?;
        self.ensure(len.saturating_mul(min))?;
        Ok((elem, len))
    }

    fn value(&mut self, ty: u32, depth: usize, keep: bool) -> Result<GgufValue, GgufError> {
        Ok(match ty {
            0 => GgufValue::U8(u8::from_le_bytes(self.le()?)),
            1 => GgufValue::I8(i8::from_le_bytes(self.le()?)),
            2 => GgufValue::U16(u16::from_le_bytes(self.le()?)),
            3 => GgufValue::I16(i16::from_le_bytes(self.le()?)),
            4 => GgufValue::U32(u32::from_le_bytes(self.le()?)),
            5 => GgufValue::I32(i32::from_le_bytes(self.le()?)),
            6 => GgufValue::F32(f32::from_le_bytes(self.le()?)),
            7 => GgufValue::Bool(u8::from_le_bytes(self.le()?) != 0),
            8 => GgufValue::String(self.string()?),
            9 => {
                let (elem, len) = self.array_header(depth)?;
                let within_budget = self.kept.saturating_add(len) <= MAX_KEPT_VALUES;
                if within_budget && ((keep && len <= MAX_LAYERS) || len <= MAX_INLINE_ARRAY) {
                    self.kept += len;
                    let mut items = Vec::new();
                    for _ in 0..len {
                        items.push(self.value(elem, depth + 1, false)?);
                    }
                    GgufValue::Array(items)
                } else {
                    self.skip_elems(elem, len, depth + 1)?;
                    GgufValue::ArraySkipped {
                        elem_type: elem,
                        len,
                    }
                }
            }
            10 => GgufValue::U64(u64::from_le_bytes(self.le()?)),
            11 => GgufValue::I64(i64::from_le_bytes(self.le()?)),
            12 => GgufValue::F64(f64::from_le_bytes(self.le()?)),
            _ => return Err(unknown_type(ty)),
        })
    }

    /// Advances past `len` values of type `elem` without storing them.
    fn skip_elems(&mut self, elem: u32, len: u64, depth: usize) -> Result<(), GgufError> {
        match fixed_size(elem) {
            Some(size) => {
                self.take(len.saturating_mul(size))?;
            }
            None => {
                for _ in 0..len {
                    self.skip_value(elem, depth)?;
                }
            }
        }
        Ok(())
    }

    fn skip_value(&mut self, ty: u32, depth: usize) -> Result<(), GgufError> {
        match ty {
            8 => {
                let len = self.u64()?;
                self.take(len)?;
            }
            9 => {
                let (elem, len) = self.array_header(depth)?;
                self.skip_elems(elem, len, depth + 1)?;
            }
            _ => {
                let size = fixed_size(ty).ok_or_else(|| unknown_type(ty))?;
                self.take(size)?;
            }
        }
        Ok(())
    }
}
