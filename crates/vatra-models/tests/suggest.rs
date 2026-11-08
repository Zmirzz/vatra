use vatra_models::estimate::{Fit, GIB, MemoryBudget, estimate};
use vatra_models::gguf::ModelFacts;
use vatra_models::suggest::{
    SuggestionEntry, SuggestionList, Task, builtin, pick_variant, suggest, unlock_vram_gib,
};
use vatra_models::variant::{Variant, quality_rank};

/// `tenths` in tenths of a GiB: `gib(25)` is 2.5 GiB.
fn gib(tenths: u64) -> u64 {
    tenths * GIB / 10
}

fn variant(name: &str, size: u64) -> Variant {
    Variant {
        name: name.into(),
        files: vec![],
        size,
        mmproj: vec![],
    }
}

fn budget(vram_gib: Option<u64>, ram_gib: u64) -> MemoryBudget {
    MemoryBudget {
        vram_free: vram_gib.map(|g| g * GIB),
        ram_available: ram_gib * GIB,
    }
}

/// An 8B dense model: 32 layers, 8 KV heads of 128. KV cache is 4 GiB at 32K tokens.
fn facts() -> ModelFacts {
    ModelFacts {
        block_count: Some(32),
        head_count: Some(32),
        head_count_kv: vec![8; 32],
        embedding_length: Some(4096),
        context_length: Some(131_072),
        has_chat_template: true,
        chat_template_mentions_tools: true,
        ..Default::default()
    }
}

fn entry(repo: &str, tasks: &[Task]) -> SuggestionEntry {
    SuggestionEntry {
        family: "family".into(),
        repo: repo.into(),
        tasks: tasks.to_vec(),
        params_b: 8.0,
        note: "A note.".into(),
    }
}

#[test]
fn suggest_picks_best_gpu_quant() {
    let variants = vec![
        variant("Q8_0", gib(90)),
        variant("Q6_K", gib(70)),
        variant("Q4_K_M", gib(25)),
    ];
    let b = budget(Some(8), 0);
    // At 4K context: the 32K KV cache alone (4 GiB) would leave no Gpu fit at 8 GiB.
    let (picked, est) = pick_variant(&variants, Some(&facts()), 4096, &b).unwrap();

    let best = variants
        .iter()
        .filter(|v| quality_rank(&v.name).is_some())
        .filter(|v| estimate(v.size, Some(&facts()), 4096, &b).fit == Fit::Gpu)
        .min_by_key(|v| quality_rank(&v.name))
        .cloned()
        .unwrap();
    assert_eq!(picked, best);
    assert_eq!(est.fit, Fit::Gpu);
    assert_ne!(picked.name, "Q8_0");
}

#[test]
fn suggest_falls_back_to_split() {
    let variants = vec![
        variant("Q8_0", gib(90)),
        variant("Q6_K", gib(70)),
        variant("Q4_K_M", gib(25)),
    ];
    let (picked, est) =
        pick_variant(&variants, Some(&facts()), 32_768, &budget(Some(2), 32)).unwrap();
    assert_eq!(est.fit, Fit::Split);
    assert_eq!(picked.name, "Q4_K_M", "a split starts at Q4_K_M");
}

#[test]
fn too_large_still_suggested_smallest() {
    let variants = vec![
        variant("Q8_0", gib(90)),
        variant("Q6_K", gib(70)),
        variant("Q4_K_M", gib(25)),
        variant("Q2_K", gib(15)),
    ];
    let b = budget(Some(1), 0);
    let (picked, est) = pick_variant(&variants, Some(&facts()), 32_768, &b).unwrap();
    assert_eq!(est.fit, Fit::TooLarge);
    assert_eq!(picked.name, "Q2_K");

    let list = SuggestionList {
        verified_on: "2026-11-03".into(),
        entries: vec![entry("a/only", &[Task::Everyday])],
    };
    let out = suggest(Task::Everyday, &b, &list, &move |_| {
        Some((variants.clone(), Some(facts())))
    });
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].variant, "Q2_K");
    assert_eq!(out[0].estimate.fit, Fit::TooLarge);
    assert!(out[0].unlock_vram_gib.is_some());
}

#[test]
fn unlock_reports_smallest_vram_step() {
    // 2.5 GiB + 5 GiB KV and overhead + 1 GiB margin needs 8.5 GiB: the 12 GiB step.
    assert_eq!(
        unlock_vram_gib(&[variant("Q4_K_M", gib(25))], Some(&facts()), 32_768),
        Some(12)
    );
    // 1 GiB at 4K context needs about 3.1 GiB: the 8 GiB step.
    assert_eq!(
        unlock_vram_gib(&[variant("Q4_K_M", gib(10))], Some(&facts()), 4096),
        Some(8)
    );
    // No Q4_K_M: the smallest ranked variant is used.
    assert_eq!(
        unlock_vram_gib(&[variant("Q8_0", gib(90))], Some(&facts()), 32_768),
        Some(16)
    );
    assert_eq!(
        unlock_vram_gib(&[variant("Q4_K_M", 100 * GIB)], Some(&facts()), 32_768),
        None
    );
    assert_eq!(unlock_vram_gib(&[], Some(&facts()), 32_768), None);
}

#[test]
fn suggestions_json_is_valid() {
    let list = builtin();
    assert!(!list.verified_on.is_empty());
    assert!(!list.entries.is_empty());
    for e in &list.entries {
        assert!(e.repo.contains('/'), "{}", e.repo);
        assert!(!e.tasks.is_empty(), "{}", e.repo);
    }
}

#[test]
fn suggest_filters_by_task_and_limits_to_three() {
    let list = SuggestionList {
        verified_on: "2026-11-03".into(),
        entries: vec![
            entry("a/everyday", &[Task::Everyday]),
            entry("a/vision1", &[Task::Vision]),
            entry("a/missing", &[Task::Vision]),
            entry("a/vision2", &[Task::Vision, Task::Everyday]),
            entry("a/vision3", &[Task::Vision]),
            entry("a/vision4", &[Task::Vision]),
        ],
    };
    let lookup = |repo: &str| {
        if repo == "a/missing" {
            None
        } else {
            Some((vec![variant("Q4_K_M", gib(10))], Some(facts())))
        }
    };
    let b = budget(Some(80), 0);

    let vision: Vec<String> = suggest(Task::Vision, &b, &list, &lookup)
        .into_iter()
        .map(|s| s.repo)
        .collect();
    assert_eq!(vision, ["a/vision1", "a/vision2", "a/vision3"]);

    let everyday: Vec<String> = suggest(Task::Everyday, &b, &list, &lookup)
        .into_iter()
        .map(|s| s.repo)
        .collect();
    assert_eq!(everyday, ["a/everyday", "a/vision2"]);
}

#[test]
fn alternatives_on_first_only() {
    let list = SuggestionList {
        verified_on: "2026-11-03".into(),
        entries: vec![
            entry("a/small", &[Task::Everyday]),
            entry("a/mid", &[Task::Everyday]),
            entry("a/big", &[Task::Everyday]),
        ],
    };
    let lookup = |repo: &str| {
        let size = match repo {
            "a/small" => gib(10),
            "a/mid" => gib(25),
            _ => 20 * GIB,
        };
        Some((vec![variant("Q4_K_M", size)], Some(facts())))
    };
    let out = suggest(Task::Everyday, &budget(Some(10), 0), &list, &lookup);
    let repos: Vec<&str> = out.iter().map(|s| s.repo.as_str()).collect();
    assert_eq!(repos, ["a/small", "a/mid", "a/big"]);

    assert_eq!(
        out[0].alternatives,
        vec![
            ("a/mid".to_string(), "Q4_K_M".to_string()),
            ("a/big".to_string(), "Q4_K_M".to_string()),
        ]
    );
    assert!(out[1].alternatives.is_empty());
    assert!(out[2].alternatives.is_empty());

    assert_eq!(out[0].unlock_vram_gib, None);
    assert_eq!(out[1].unlock_vram_gib, None);
    assert_eq!(out[2].unlock_vram_gib, Some(32));
}

#[test]
fn split_prefers_q4_over_bigger_quants() {
    let variants = vec![variant("Q8_0", 4 * GIB), variant("Q6_K", 3 * GIB), variant("Q4_K_M", 2 * GIB + GIB / 3)];
    let budget = MemoryBudget { vram_free: Some(2 * GIB), ram_available: 32 * GIB };
    let (picked, estimate) = pick_variant(&variants, Some(&facts()), 32_768, &budget).unwrap();
    assert_eq!(estimate.fit, Fit::Split);
    assert_eq!(picked.name, "Q4_K_M");
}
