//! Memory a GGUF model needs for a context size, and whether it fits the budget.

use serde::{Deserialize, Serialize};

use crate::gguf::ModelFacts;

pub const GIB: u64 = 1 << 30;
pub const MIB: u64 = 1 << 20;
/// Same margin llama.cpp's --fit-target uses by default.
pub const GPU_MARGIN: u64 = 1024 * MIB;
/// RAM left to the system and other apps when a model spills into RAM.
pub const RAM_RESERVE: u64 = 4 * GIB;

const BASE_OVERHEAD: u64 = 512 * MIB;
const F16_BYTES: u64 = 2;
/// Above this, the per-layer KV walk is refused as implausible metadata.
const MAX_LAYERS: u64 = 1 << 16;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryBudget {
    pub vram_free: Option<u64>,
    pub ram_available: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Fit {
    Gpu,
    Split,
    CpuOnly,
    TooLarge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EstimateNote {
    MetadataMissing,
    MoeSplitFaster,
    LowQuant,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Estimate {
    pub needed: u64,
    pub weights: u64,
    pub kv: u64,
    pub overhead: u64,
    pub ctx: u32,
    pub fit: Fit,
    pub notes: Vec<EstimateNote>,
}

/// Estimates the memory for `ctx` tokens of a model `size` bytes on disk.
/// `size` already includes any mmproj file.
pub fn estimate(
    size: u64,
    facts: Option<&ModelFacts>,
    ctx: u32,
    budget: &MemoryBudget,
) -> Estimate {
    let mut notes = Vec::new();
    let (kv, overhead) = match facts {
        None => {
            notes.push(EstimateNote::MetadataMissing);
            (0, BASE_OVERHEAD)
        }
        Some(f) => {
            let kv = kv_cache(f, u64::from(ctx)).unwrap_or_else(|| {
                notes.push(EstimateNote::MetadataMissing);
                0
            });
            let ctx_overhead = f
                .embedding_length
                .map_or(0, |e| u64::from(ctx).saturating_mul(e).saturating_mul(4));
            (kv, BASE_OVERHEAD.saturating_add(ctx_overhead))
        }
    };
    let needed = size.saturating_add(kv).saturating_add(overhead);

    let ram_free = budget.ram_available.saturating_sub(RAM_RESERVE);
    let with_margin = needed.saturating_add(GPU_MARGIN);
    let fit = match budget.vram_free {
        Some(v) if with_margin <= v => Fit::Gpu,
        Some(v) if with_margin <= v.saturating_add(ram_free) => Fit::Split,
        None if needed <= ram_free => Fit::CpuOnly,
        _ => Fit::TooLarge,
    };

    let moe = facts.and_then(|f| f.expert_count).is_some_and(|n| n > 1);
    if fit == Fit::Split && moe {
        notes.push(EstimateNote::MoeSplitFaster);
    }

    Estimate {
        needed,
        weights: size,
        kv,
        overhead,
        ctx,
        fit,
        notes,
    }
}

/// KV cache bytes for `ctx` tokens (f16 cache). 0 when the facts can't tell.
pub fn kv_bytes(facts: &ModelFacts, ctx: u32) -> u64 {
    kv_cache(facts, u64::from(ctx)).unwrap_or(0)
}

/// The largest context (multiple of 1024) that fits on the GPU alone.
/// Needed grows with context, so the search can bisect.
pub fn max_gpu_ctx(
    size: u64,
    facts: &ModelFacts,
    budget: &MemoryBudget,
    max_ctx: u32,
) -> Option<u32> {
    let cap = u64::from(max_ctx).min(facts.context_length.unwrap_or(u64::MAX));
    let steps = cap / 1024;
    let fits = |k: u64| estimate(size, Some(facts), (k * 1024) as u32, budget).fit == Fit::Gpu;
    if steps == 0 || !fits(1) {
        return None;
    }
    let (mut lo, mut hi) = (1, steps);
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if fits(mid) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    Some((lo * 1024) as u32)
}

/// KV bytes, or None when the block count or head numbers are missing.
fn kv_cache(facts: &ModelFacts, ctx: u64) -> Option<u64> {
    let layers = facts.block_count?;

    // MLA stores one compressed latent per token and layer; head counts don't matter.
    if let Some(rank) = facts.kv_lora_rank {
        let rope = facts.rope_dim.unwrap_or(64);
        return Some(
            layers
                .saturating_mul(ctx)
                .saturating_mul(rank.saturating_add(rope))
                .saturating_mul(F16_BYTES),
        );
    }

    let head_k = facts
        .key_length
        .or_else(|| facts.embedding_length?.checked_div(facts.head_count?))?;
    let head_v = facts.value_length.unwrap_or(head_k);
    if layers > MAX_LAYERS {
        return None;
    }

    let mut total = 0u64;
    for i in 0..layers {
        let heads = layer_heads(facts, layers, i)?;
        if heads == 0 {
            // Recurrent (SSM) layers keep a small fixed state, not a KV cache.
            continue;
        }
        // Approximation: gemma-style 5:1 interleave, where every 6th layer is full attention.
        let span = match facts.sliding_window {
            Some(window) if i % 6 != 5 => ctx.min(window),
            _ => ctx,
        };
        let layer = span
            .saturating_mul(heads)
            .saturating_mul(head_k.saturating_add(head_v));
        total = total.saturating_add(layer.saturating_mul(F16_BYTES));
    }
    Some(total)
}

/// KV heads for layer `i`. A single entry applies to every layer; no entries means no GQA.
fn layer_heads(facts: &ModelFacts, layers: u64, i: u64) -> Option<u64> {
    match facts.head_count_kv.as_slice() {
        [] => facts.head_count,
        [one] => Some(u64::from(*one)),
        all if all.len() as u64 == layers => Some(u64::from(all[i as usize])),
        _ => None,
    }
}
