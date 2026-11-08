//! Model suggestions for a task, each picked to fit this machine's memory.

use serde::{Deserialize, Serialize};

use crate::estimate::{Estimate, Fit, GIB, MemoryBudget, estimate};
use crate::gguf::ModelFacts;
use crate::variant::{Variant, is_low_quant, quality_rank};

/// VRAM sizes, in GiB, that the unlock hint can name.
const UNLOCK_STEPS_GIB: [u32; 6] = [8, 12, 16, 24, 32, 48];
const SHOWN: usize = 3;

/// Gives a repo's variants and the facts of one of its files.
type Lookup<'a> = dyn Fn(&str) -> Option<(Vec<Variant>, Option<ModelFacts>)> + 'a;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Task {
    Everyday,
    Coding,
    LongDocuments,
    Vision,
}

impl Task {
    pub fn target_ctx(self) -> u32 {
        match self {
            Task::LongDocuments => 65_536,
            _ => 32_768,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionEntry {
    pub family: String,
    pub repo: String,
    pub tasks: Vec<Task>,
    pub params_b: f64,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionList {
    pub verified_on: String,
    pub entries: Vec<SuggestionEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub family: String,
    pub repo: String,
    pub variant: String,
    pub size: u64,
    pub estimate: Estimate,
    /// (repo, variant) of the other suggestions; set on the first one only.
    pub alternatives: Vec<(String, String)>,
    /// The smallest VRAM, in GiB, that runs this variant fully on the GPU. Only set when it doesn't fit now.
    pub unlock_vram_gib: Option<u32>,
    pub low_quality: bool,
    pub note: String,
}

/// The suggestions bundled with the app.
pub fn builtin() -> SuggestionList {
    serde_json::from_str(include_str!("../data/suggestions.json"))
        .expect("bundled suggestions.json is valid")
}

/// Up to three suggestions for `task`, best fit first. `variants(repo)` gives a repo's variants and
/// the facts of one of its files; repos it can't give are skipped.
pub fn suggest(
    task: Task,
    budget: &MemoryBudget,
    list: &SuggestionList,
    variants: &Lookup,
) -> Vec<Suggestion> {
    let ctx = task.target_ctx();
    let mut found: Vec<Suggestion> = list
        .entries
        .iter()
        .filter(|entry| entry.tasks.contains(&task))
        .filter_map(|entry| {
            let (repo_variants, facts) = variants(&entry.repo)?;
            let facts = facts.as_ref();
            let (picked, est) = pick_variant(&repo_variants, facts, ctx, budget)?;
            Some(Suggestion {
                family: entry.family.clone(),
                repo: entry.repo.clone(),
                variant: picked.name.clone(),
                size: picked.size,
                low_quality: is_low_quant(&picked.name),
                unlock_vram_gib: match est.fit {
                    Fit::Gpu => None,
                    _ => unlock_vram_gib(&repo_variants, facts, ctx),
                },
                estimate: est,
                alternatives: Vec::new(),
                note: entry.note.clone(),
            })
        })
        .collect();

    found.sort_by_key(|s| fit_rank(s.estimate.fit));
    found.truncate(SHOWN);
    let alternatives: Vec<(String, String)> = found
        .iter()
        .skip(1)
        .map(|s| (s.repo.clone(), s.variant.clone()))
        .collect();
    if let Some(first) = found.first_mut() {
        first.alternatives = alternatives;
    }
    found
}

/// The best-quality variant that fits, with its estimate. Falls back to the smallest variant
/// overall (marked TooLarge) when nothing fits. None only when `variants` is empty.
pub fn pick_variant(
    variants: &[Variant],
    facts: Option<&ModelFacts>,
    ctx: u32,
    budget: &MemoryBudget,
) -> Option<(Variant, Estimate)> {
    let mut ranked: Vec<(Variant, Estimate)> = variants
        .iter()
        .filter(|v| quality_rank(&v.name).is_some())
        .map(|v| (v.clone(), estimate(v.size, facts, ctx, budget)))
        .collect();
    ranked.sort_by_key(|(v, _)| quality_rank(&v.name));

    let first_fit = |accept: &[Fit]| {
        ranked
            .iter()
            .find(|(_, est)| accept.contains(&est.fit))
            .cloned()
    };
    // Part of a split model runs from RAM, where every extra byte costs speed, so a split starts at Q4_K_M.
    let split_from = quality_rank("Q4_K_M").unwrap_or(0);
    let compact_split = ranked
        .iter()
        .find(|(v, est)| quality_rank(&v.name) >= Some(split_from) && matches!(est.fit, Fit::Split | Fit::CpuOnly))
        .cloned();
    first_fit(&[Fit::Gpu])
        .or(compact_split)
        .or_else(|| first_fit(&[Fit::Split, Fit::CpuOnly]))
        .or_else(|| {
            variants
                .iter()
                .min_by_key(|v| v.size)
                .map(|v| (v.clone(), estimate(v.size, facts, ctx, budget)))
        })
}

/// The smallest VRAM step, in GiB, that runs the Q4_K_M variant (or the smallest ranked one) fully on the GPU.
pub fn unlock_vram_gib(variants: &[Variant], facts: Option<&ModelFacts>, ctx: u32) -> Option<u32> {
    let ranked = variants.iter().filter(|v| quality_rank(&v.name).is_some());
    let target = variants
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case("Q4_K_M"))
        .or_else(|| ranked.min_by_key(|v| v.size))?;
    UNLOCK_STEPS_GIB.into_iter().find(|&gib| {
        let budget = MemoryBudget {
            vram_free: Some(u64::from(gib) * GIB),
            ram_available: 0,
        };
        estimate(target.size, facts, ctx, &budget).fit == Fit::Gpu
    })
}

fn fit_rank(fit: Fit) -> u8 {
    match fit {
        Fit::Gpu => 0,
        Fit::Split | Fit::CpuOnly => 1,
        Fit::TooLarge => 2,
    }
}
