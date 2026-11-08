//! What this computer has for running models.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::llama::{self, LlamaInstall};

#[cfg(windows)]
mod windows;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareReport {
    pub cpu: Cpu,
    pub ram: Ram,
    pub gpus: Vec<Gpu>,
    /// Free space where models are kept.
    pub disk_free: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cpu {
    pub name: Option<String>,
    pub cores: Option<u32>,
    pub threads: Option<u32>,
    pub avx2: bool,
    pub avx512: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ram {
    pub total: u64,
    pub available: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gpu {
    pub name: String,
    /// The llama.cpp device name, like `CUDA0` or `Vulkan0`. None when only Windows reported the card.
    pub backend: Option<String>,
    pub total: u64,
    pub free: Option<u64>,
    pub source: GpuSource,
    pub integrated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GpuSource {
    /// Listed by `llama-server --list-devices`: exactly what llama.cpp can use.
    Llama,
    /// Seen by Windows, not (yet) by llama.cpp.
    System,
}

/// Reads the hardware. GPUs come from llama.cpp when it is installed and lists any; otherwise from the system.
pub fn detect(llama: Option<&LlamaInstall>, models_dir: Option<&Path>) -> HardwareReport {
    let mut report = system(models_dir);
    if let Some(install) = llama {
        let devices = llama::devices(install);
        if !devices.is_empty() {
            report.gpus = devices;
        }
    }
    report
}

#[cfg(windows)]
fn system(models_dir: Option<&Path>) -> HardwareReport {
    windows::detect(models_dir)
}

#[cfg(not(windows))]
fn system(_models_dir: Option<&Path>) -> HardwareReport {
    HardwareReport { cpu: cpu_features(Cpu::default()), ..HardwareReport::default() }
}

pub(crate) fn cpu_features(mut cpu: Cpu) -> Cpu {
    #[cfg(target_arch = "x86_64")]
    {
        cpu.avx2 = std::arch::is_x86_feature_detected!("avx2");
        cpu.avx512 = std::arch::is_x86_feature_detected!("avx512f");
    }
    cpu
}

impl HardwareReport {
    /// The GPU with the most free memory that llama.cpp can use, or the largest one Windows reported.
    pub fn best_gpu(&self) -> Option<&Gpu> {
        self.gpus
            .iter()
            .filter(|gpu| !gpu.integrated)
            .max_by_key(|gpu| (gpu.source == GpuSource::Llama, gpu.free.unwrap_or(gpu.total)))
    }
}

/// Free space on the disk that holds `dir`, or its nearest existing parent.
pub fn disk_free(dir: &Path) -> Option<u64> {
    #[cfg(windows)]
    return windows::disk_free(dir);
    #[cfg(not(windows))]
    {
        let _ = dir;
        None
    }
}
