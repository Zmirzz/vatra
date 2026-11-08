//! `vatra hardware` and `vatra models inspect`: what the app's Models page shows, from the terminal.

use std::path::Path;

use serde_json::json;
use vatra_models::estimate::{self, MemoryBudget};
use vatra_models::hardware;
use vatra_models::hf::HfClient;
use vatra_models::label;
use vatra_models::llama;

pub fn print_hardware(llama_server: Option<&Path>, as_json: bool) -> Result<(), String> {
    let install = llama::find(llama_server);
    let report = hardware::detect(install.as_ref(), Some(&vatra_service::default_models_dir()));
    if as_json {
        let value = json!({ "hardware": report, "llama": install });
        println!("{}", serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?);
        return Ok(());
    }
    println!("CPU: {} ({} cores, {} threads)", report.cpu.name.as_deref().unwrap_or("unknown"), show(report.cpu.cores), show(report.cpu.threads));
    println!("RAM: {} total, {} available", gib(report.ram.total), gib(report.ram.available));
    for gpu in &report.gpus {
        let free = gpu.free.map(|free| format!(", {} free", gib(free))).unwrap_or_default();
        println!("GPU: {} ({}{free}) [{}]", gpu.name, gib(gpu.total), gpu.backend.as_deref().unwrap_or("seen by Windows"));
    }
    match install {
        Some(install) => println!(
            "llama.cpp: build {} at {} (--fit: {})",
            show(install.build),
            install.server.display(),
            if install.caps.fit { "yes" } else { "no" }
        ),
        None => println!("llama.cpp: not found. Install it with: winget install ggml.llamacpp"),
    }
    Ok(())
}

/// Reads a variant's header from Hugging Face (never the whole file) and estimates it for this computer.
pub async fn inspect(repo: &str, variant: &str, ctx: Option<u32>, hf_url: Option<&str>, llama_server: Option<&Path>) -> Result<(), String> {
    let hf = match hf_url {
        Some(url) => HfClient::new(url.parse().map_err(|error| format!("{error}"))?),
        None => HfClient::public(),
    };
    let files = hf.files(repo).await.map_err(|error| error.to_string())?;
    let chosen = files
        .variants
        .iter()
        .find(|candidate| candidate.name.eq_ignore_ascii_case(variant))
        .ok_or_else(|| {
            let names: Vec<&str> = files.variants.iter().map(|candidate| candidate.name.as_str()).collect();
            format!("`{variant}` isn't in {repo}; it has {}", names.join(", "))
        })?;
    let first = chosen.files.first().ok_or("the variant has no files")?;
    let remote = hf.remote_facts(repo, &first.path).await.map_err(|error| error.to_string())?;
    let report = hardware::detect(llama::find(llama_server).as_ref(), None);
    let budget = MemoryBudget {
        vram_free: report.best_gpu().map(|gpu| gpu.free.unwrap_or(gpu.total)),
        ram_available: report.ram.available,
    };
    let ctx = ctx.unwrap_or_else(|| remote.facts.context_length.map_or(32_768, |max| max.min(32_768) as u32));
    let estimate = estimate::estimate(chosen.size, Some(&remote.facts), ctx, &budget);
    let fitted = estimate::max_gpu_ctx(chosen.size, &remote.facts, &budget, ctx);
    let (label, reasons) = label::estimated(Some(&remote.facts), fitted.or(Some(ctx)), Some(estimate.fit));
    let value = json!({
        "repo": repo,
        "variant": chosen.name,
        "size": chosen.size,
        "headerBytesRead": remote.bytes_read,
        "facts": remote.facts,
        "budget": budget,
        "estimate": estimate,
        "maxGpuCtx": fitted,
        "label": label,
        "reasons": reasons,
        "kind": "estimate",
    });
    println!("{}", serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?);
    Ok(())
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1u64 << 30) as f64)
}

fn show<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "?".into(), |value| value.to_string())
}
