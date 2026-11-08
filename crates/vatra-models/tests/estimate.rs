use vatra_models::estimate::{
    EstimateNote, Fit, GIB, MIB, MemoryBudget, estimate, kv_bytes, max_gpu_ctx,
};
use vatra_models::gguf::ModelFacts;

fn budget(vram: Option<u64>, ram: u64) -> MemoryBudget {
    MemoryBudget {
        vram_free: vram,
        ram_available: ram,
    }
}

fn llama8b() -> ModelFacts {
    ModelFacts {
        block_count: Some(32),
        head_count: Some(32),
        head_count_kv: vec![8; 32],
        embedding_length: Some(4096),
        context_length: Some(32768),
        ..Default::default()
    }
}

/// File size for which `estimate(size, None, ..)` needs exactly `needed` bytes.
fn size_for(needed: u64) -> u64 {
    needed - 512 * MIB
}

#[test]
fn kv_llama_8b_32k() {
    assert_eq!(kv_bytes(&llama8b(), 32768), 4 * GIB);
}

#[test]
fn kv_single_entry_head_count_kv_applies_to_all_layers() {
    let facts = ModelFacts {
        block_count: Some(4),
        head_count: Some(32),
        head_count_kv: vec![8],
        embedding_length: Some(2048),
        ..Default::default()
    };
    // head_dim 64 for k and v: 4 layers * 1024 tokens * 8 heads * 128 * 2 bytes.
    assert_eq!(kv_bytes(&facts, 1024), 4 * 1024 * 8 * 128 * 2);
}

#[test]
fn kv_sliding_window_caps_layers() {
    let facts = ModelFacts {
        sliding_window: Some(1024),
        block_count: Some(12),
        head_count: Some(32),
        head_count_kv: vec![8; 12],
        embedding_length: Some(4096),
        ..Default::default()
    };
    // Layers 5 and 11 are full attention; the other 10 are capped at the window.
    // Per token per layer: 8 heads * (128 + 128) * 2 bytes.
    let expected = (10 * 1024 + 2 * 32768) * 8 * 256 * 2;
    assert_eq!(kv_bytes(&facts, 32768), expected);
}

#[test]
fn kv_mla_uses_lora_rank() {
    let facts = ModelFacts {
        kv_lora_rank: Some(512),
        rope_dim: Some(64),
        block_count: Some(61),
        ..Default::default()
    };
    assert_eq!(kv_bytes(&facts, 8192), 61 * 8192 * 576 * 2);
}

#[test]
fn kv_zero_heads_are_recurrent_layers() {
    let facts = ModelFacts {
        block_count: Some(4),
        head_count: Some(32),
        head_count_kv: vec![0, 0, 0, 8],
        embedding_length: Some(4096),
        recurrent: true,
        ..Default::default()
    };
    // Only the last layer holds a KV cache: 1000 tokens * 8 heads * 256 * 2 bytes.
    assert_eq!(kv_bytes(&facts, 1000), 1000 * 8 * 256 * 2);
}

#[test]
fn kv_missing_heads_is_metadata_missing() {
    let facts = ModelFacts {
        block_count: Some(32),
        embedding_length: Some(4096),
        ..Default::default()
    };
    assert_eq!(kv_bytes(&facts, 4096), 0);
    let e = estimate(GIB, Some(&facts), 4096, &budget(None, 64 * GIB));
    assert_eq!(e.kv, 0);
    assert!(e.notes.contains(&EstimateNote::MetadataMissing));
}

#[test]
fn fit_gpu_with_margin() {
    let size = size_for(6 * GIB);
    let e = estimate(size, None, 0, &budget(Some(GIB * 15 / 2), 32 * GIB));
    assert_eq!(e.needed, 6 * GIB);
    assert_eq!(e.fit, Fit::Gpu);

    let e = estimate(size, None, 0, &budget(Some(GIB * 67 / 10), 32 * GIB));
    assert_eq!(e.fit, Fit::Split);
}

#[test]
fn fit_cpu_only_without_gpu() {
    let size = size_for(6 * GIB);
    let e = estimate(size, None, 0, &budget(None, 32 * GIB));
    assert_eq!(e.fit, Fit::CpuOnly);

    // RAM minus the 4 GiB reserve is only 4 GiB here.
    let e = estimate(size, None, 0, &budget(None, 8 * GIB));
    assert_eq!(e.fit, Fit::TooLarge);
}

#[test]
fn too_large_still_returns_estimate() {
    let e = estimate(20 * GIB, None, 4096, &budget(Some(4 * GIB), 8 * GIB));
    assert_eq!(e.fit, Fit::TooLarge);
    assert_eq!(e.weights, 20 * GIB);
    assert_eq!(e.kv, 0);
    assert_eq!(e.overhead, 512 * MIB);
    assert_eq!(e.needed, 20 * GIB + 512 * MIB);
    assert_eq!(e.ctx, 4096);
    assert_eq!(e.notes, vec![EstimateNote::MetadataMissing]);
}

#[test]
fn missing_metadata_uses_size_only() {
    let e = estimate(10 * GIB, None, 4096, &budget(None, 64 * GIB));
    assert_eq!(e.kv, 0);
    assert_eq!(e.needed, 10 * GIB + 512 * MIB);
    assert_eq!(e.notes, vec![EstimateNote::MetadataMissing]);
}

#[test]
fn moe_split_note() {
    let budget = budget(Some(4 * GIB), 32 * GIB);
    let moe = ModelFacts {
        expert_count: Some(8),
        ..llama8b()
    };
    let e = estimate(10 * GIB, Some(&moe), 4096, &budget);
    assert_eq!(e.fit, Fit::Split);
    assert!(e.notes.contains(&EstimateNote::MoeSplitFaster));

    let dense = llama8b();
    let e = estimate(10 * GIB, Some(&dense), 4096, &budget);
    assert_eq!(e.fit, Fit::Split);
    assert!(e.notes.is_empty());

    let single = ModelFacts {
        expert_count: Some(1),
        ..llama8b()
    };
    let e = estimate(10 * GIB, Some(&single), 4096, &budget);
    assert!(!e.notes.contains(&EstimateNote::MoeSplitFaster));
}

#[test]
fn max_gpu_ctx_monotonic() {
    let facts = llama8b();
    let size = 5 * GIB;
    let results: Vec<Option<u32>> = [7, 8, 9, 10, 12, 40]
        .iter()
        .map(|&v| max_gpu_ctx(size, &facts, &budget(Some(v * GIB), 0), 32768))
        .collect();
    for pair in results.windows(2) {
        assert!(pair[0] <= pair[1], "{results:?}");
    }

    // 5 GiB weights + 512 MiB overhead + 1 GiB margin is more than 6 GiB before any context.
    assert_eq!(
        max_gpu_ctx(size, &facts, &budget(Some(6 * GIB), 0), 32768),
        None
    );

    // Result must fit, and the next step must not.
    let r = max_gpu_ctx(size, &facts, &budget(Some(8 * GIB), 0), 32768).unwrap();
    assert!((1024..32768).contains(&r));
    let b = budget(Some(8 * GIB), 0);
    assert_eq!(estimate(size, Some(&facts), r, &b).fit, Fit::Gpu);
    assert_ne!(estimate(size, Some(&facts), r + 1024, &b).fit, Fit::Gpu);

    // Capped by the model's context length, and by max_ctx.
    assert_eq!(
        max_gpu_ctx(size, &facts, &budget(Some(100 * GIB), 0), 65536),
        Some(32768)
    );
    assert_eq!(
        max_gpu_ctx(size, &facts, &budget(Some(100 * GIB), 0), 2000),
        Some(1024)
    );

    // Weights alone do not fit.
    assert_eq!(
        max_gpu_ctx(20 * GIB, &facts, &budget(Some(8 * GIB), 0), 32768),
        None
    );
}
