mod support;

use support::build_header;
use vatra_models::gguf::{GgufError, GgufValue, ModelFacts, parse};

fn s(v: &str) -> GgufValue {
    GgufValue::String(v.to_string())
}

#[test]
fn parses_minimal_v3_header() {
    let buf = build_header(
        3,
        &[
            ("general.architecture", s("llama")),
            ("general.name", s("Tiny")),
            ("llama.block_count", GgufValue::U32(4)),
        ],
    );
    let h = parse(&buf).unwrap();
    assert_eq!(h.version, 3);
    assert_eq!(h.tensor_count, 0);
    assert_eq!(h.kv.len(), 3);
    assert_eq!(h.kv["general.architecture"], s("llama"));
    assert_eq!(h.kv["general.name"], s("Tiny"));
    assert_eq!(h.kv["llama.block_count"], GgufValue::U32(4));
}

#[test]
fn accepts_v2() {
    let buf = build_header(2, &[("general.architecture", s("llama"))]);
    assert_eq!(parse(&buf).unwrap().version, 2);
}

#[test]
fn needs_more_reports_offset() {
    let buf = build_header(3, &[("general.name", s("a long model name"))]);
    let cut = buf.len() - 5;
    match parse(&buf[..cut]) {
        Err(GgufError::NeedMore { at_least }) => {
            assert!(at_least > cut, "at_least {at_least} should exceed {cut}");
            assert_eq!(at_least, buf.len());
            assert!(parse(&buf[..at_least]).is_ok());
        }
        other => panic!("expected NeedMore, got {other:?}"),
    }
    assert!(parse(&buf).is_ok());
}

#[test]
fn rejects_bad_magic() {
    let mut buf = build_header(3, &[("general.architecture", s("llama"))]);
    buf[0] = b'X';
    assert!(matches!(parse(&buf), Err(GgufError::Invalid(_))));
}

#[test]
fn rejects_bad_version() {
    let buf = build_header(1, &[]);
    assert!(matches!(parse(&buf), Err(GgufError::Invalid(_))));
}

#[test]
fn rejects_huge_string_len() {
    let mut buf = b"GGUF".to_vec();
    buf.extend_from_slice(&3u32.to_le_bytes());
    buf.extend_from_slice(&0u64.to_le_bytes());
    buf.extend_from_slice(&1u64.to_le_bytes());
    buf.extend_from_slice(&(1u64 << 40).to_le_bytes());
    assert_eq!(parse(&buf), Err(GgufError::TooLarge));
}

#[test]
fn rejects_huge_array_count() {
    let mut buf = b"GGUF".to_vec();
    buf.extend_from_slice(&3u32.to_le_bytes());
    buf.extend_from_slice(&0u64.to_le_bytes());
    buf.extend_from_slice(&1u64.to_le_bytes());
    buf.extend_from_slice(&3u64.to_le_bytes()); // key length
    buf.extend_from_slice(b"abc");
    buf.extend_from_slice(&9u32.to_le_bytes()); // array
    buf.extend_from_slice(&0u32.to_le_bytes()); // u8 elements
    buf.extend_from_slice(&(1u64 << 40).to_le_bytes());
    assert_eq!(parse(&buf), Err(GgufError::TooLarge));
}

#[test]
fn rejects_deep_array_nesting() {
    let mut buf = b"GGUF".to_vec();
    buf.extend_from_slice(&3u32.to_le_bytes());
    buf.extend_from_slice(&0u64.to_le_bytes());
    buf.extend_from_slice(&1u64.to_le_bytes());
    buf.extend_from_slice(&1u64.to_le_bytes());
    buf.push(b'x');
    buf.extend_from_slice(&9u32.to_le_bytes());
    for _ in 0..100_000 {
        buf.extend_from_slice(&9u32.to_le_bytes()); // elements are arrays
        buf.extend_from_slice(&1u64.to_le_bytes());
    }
    assert!(matches!(parse(&buf), Err(GgufError::Invalid(_))));
}

#[test]
fn skips_token_arrays() {
    let tokens = (0..100_000)
        .map(|i| GgufValue::String(format!("t{i}")))
        .collect();
    let buf = build_header(
        3,
        &[
            ("tokenizer.ggml.tokens", GgufValue::Array(tokens)),
            ("general.name", s("after")),
        ],
    );
    let h = parse(&buf).unwrap();
    assert_eq!(
        h.kv["tokenizer.ggml.tokens"],
        GgufValue::ArraySkipped {
            elem_type: 8,
            len: 100_000
        }
    );
    assert_eq!(h.kv["general.name"], s("after"));
}

#[test]
fn facts_per_layer_head_count_kv() {
    let buf = build_header(
        3,
        &[
            ("general.architecture", s("llama")),
            ("llama.block_count", GgufValue::U32(4)),
            (
                "llama.attention.head_count_kv",
                GgufValue::Array(vec![
                    GgufValue::U32(8),
                    GgufValue::U32(8),
                    GgufValue::U32(0),
                    GgufValue::U32(8),
                ]),
            ),
        ],
    );
    let facts = ModelFacts::from_header(&parse(&buf).unwrap());
    assert_eq!(facts.head_count_kv, vec![8, 8, 0, 8]);
}

#[test]
fn facts_scalar_head_count_kv_expands() {
    let buf = build_header(
        3,
        &[
            ("general.architecture", s("llama")),
            ("llama.block_count", GgufValue::U32(3)),
            ("llama.attention.head_count_kv", GgufValue::U32(4)),
        ],
    );
    let facts = ModelFacts::from_header(&parse(&buf).unwrap());
    assert_eq!(facts.head_count_kv, vec![4, 4, 4]);
}

#[test]
fn facts_template_tools() {
    let buf = build_header(
        3,
        &[("tokenizer.chat_template", s("{% if tools %}...{% endif %}"))],
    );
    let facts = ModelFacts::from_header(&parse(&buf).unwrap());
    assert!(facts.chat_template_mentions_tools);
    assert!(facts.has_chat_template);
}

#[test]
fn facts_missing_keys_are_none() {
    let empty = ModelFacts::from_header(&parse(&build_header(3, &[])).unwrap());
    assert_eq!(empty, ModelFacts::default());

    let arch_only = ModelFacts::from_header(
        &parse(&build_header(3, &[("general.architecture", s("qwen2"))])).unwrap(),
    );
    assert_eq!(
        arch_only,
        ModelFacts {
            architecture: Some("qwen2".into()),
            ..ModelFacts::default()
        }
    );
}

#[test]
fn facts_recurrent_from_ssm_keys() {
    let ssm = build_header(
        3,
        &[
            ("general.architecture", s("mamba2")),
            ("mamba2.ssm.state_size", GgufValue::U32(128)),
        ],
    );
    assert!(ModelFacts::from_header(&parse(&ssm).unwrap()).recurrent);

    let other = build_header(
        3,
        &[
            ("general.architecture", s("llama")),
            ("mamba2.ssm.state_size", GgufValue::U32(128)),
        ],
    );
    assert!(!ModelFacts::from_header(&parse(&other).unwrap()).recurrent);
}

#[test]
fn facts_read_arch_keys_and_max_head_count() {
    let buf = build_header(
        3,
        &[
            ("general.architecture", s("llama")),
            ("general.file_type", GgufValue::U32(15)),
            ("llama.context_length", GgufValue::U32(8192)),
            ("llama.embedding_length", GgufValue::U64(4096)),
            ("llama.block_count", GgufValue::U32(2)),
            (
                "llama.attention.head_count",
                GgufValue::Array(vec![GgufValue::U32(16), GgufValue::U32(32)]),
            ),
            ("llama.attention.sliding_window", GgufValue::I32(-1)),
            ("llama.rope.dimension_count", GgufValue::U16(128)),
        ],
    );
    let facts = ModelFacts::from_header(&parse(&buf).unwrap());
    assert_eq!(facts.file_type, Some(15));
    assert_eq!(facts.context_length, Some(8192));
    assert_eq!(facts.embedding_length, Some(4096));
    assert_eq!(facts.block_count, Some(2));
    assert_eq!(facts.head_count, Some(32));
    assert_eq!(facts.sliding_window, None);
    assert_eq!(facts.rope_dim, Some(128));
}

#[test]
fn small_arrays_share_one_budget() {
    let names: Vec<String> = (0..3000).map(|n| format!("x.array_{n:05}")).collect();
    let kvs: Vec<(&str, GgufValue)> =
        names.iter().map(|name| (name.as_str(), GgufValue::Array(vec![GgufValue::U8(1); 64]))).collect();
    let header = parse(&build_header(3, &kvs)).unwrap();
    let kept = header.kv.values().filter(|value| matches!(value, GgufValue::Array(_))).count();
    let skipped = header.kv.values().filter(|value| matches!(value, GgufValue::ArraySkipped { .. })).count();
    assert_eq!(kept, (1 << 17) / 64);
    assert_eq!(kept + skipped, 3000);
}
