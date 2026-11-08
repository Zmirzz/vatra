use vatra_models::gguf::GgufValue;

/// Writes a GGUF header with no tensors and the given key/value pairs.
///
/// Arrays take their element type from the first item (empty arrays use u8).
/// `ArraySkipped` only comes out of the parser, so it cannot be written.
pub fn build_header(version: u32, kvs: &[(&str, GgufValue)]) -> Vec<u8> {
    let mut out = b"GGUF".to_vec();
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&(kvs.len() as u64).to_le_bytes());
    for (key, value) in kvs {
        put_string(&mut out, key);
        out.extend_from_slice(&type_id(value).to_le_bytes());
        put_value(&mut out, value);
    }
    out
}

fn type_id(value: &GgufValue) -> u32 {
    match value {
        GgufValue::U8(_) => 0,
        GgufValue::I8(_) => 1,
        GgufValue::U16(_) => 2,
        GgufValue::I16(_) => 3,
        GgufValue::U32(_) => 4,
        GgufValue::I32(_) => 5,
        GgufValue::F32(_) => 6,
        GgufValue::Bool(_) => 7,
        GgufValue::String(_) => 8,
        GgufValue::Array(_) | GgufValue::ArraySkipped { .. } => 9,
        GgufValue::U64(_) => 10,
        GgufValue::I64(_) => 11,
        GgufValue::F64(_) => 12,
    }
}

fn put_string(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u64).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

/// Writes a value body (no type tag). Array elements are written this way too.
fn put_value(out: &mut Vec<u8>, value: &GgufValue) {
    match value {
        GgufValue::U8(v) => out.push(*v),
        GgufValue::I8(v) => out.extend_from_slice(&v.to_le_bytes()),
        GgufValue::U16(v) => out.extend_from_slice(&v.to_le_bytes()),
        GgufValue::I16(v) => out.extend_from_slice(&v.to_le_bytes()),
        GgufValue::U32(v) => out.extend_from_slice(&v.to_le_bytes()),
        GgufValue::I32(v) => out.extend_from_slice(&v.to_le_bytes()),
        GgufValue::F32(v) => out.extend_from_slice(&v.to_le_bytes()),
        GgufValue::Bool(v) => out.push(u8::from(*v)),
        GgufValue::String(s) => put_string(out, s),
        GgufValue::U64(v) => out.extend_from_slice(&v.to_le_bytes()),
        GgufValue::I64(v) => out.extend_from_slice(&v.to_le_bytes()),
        GgufValue::F64(v) => out.extend_from_slice(&v.to_le_bytes()),
        GgufValue::Array(items) => {
            let elem = items.first().map_or(0, type_id);
            out.extend_from_slice(&elem.to_le_bytes());
            out.extend_from_slice(&(items.len() as u64).to_le_bytes());
            for item in items {
                put_value(out, item);
            }
        }
        GgufValue::ArraySkipped { .. } => unreachable!("ArraySkipped only comes out of the parser"),
    }
}
