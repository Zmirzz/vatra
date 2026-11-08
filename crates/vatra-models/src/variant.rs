//! A repo's GGUF files, grouped into the variants a person picks: one per quantization.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoFile {
    pub path: String,
    pub size: u64,
    /// The SHA-256 Hugging Face keeps for LFS files.
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Variant {
    /// Like `Q4_K_M` or `UD-Q4_K_XL`; the file stem when no known quantization is in the name.
    pub name: String,
    /// In order; a split model has several parts.
    pub files: Vec<RepoFile>,
    pub size: u64,
    /// Vision projectors in the same repo, offered with every variant.
    pub mmproj: Vec<RepoFile>,
}

/// Best first. Names not listed rank below these.
const QUALITY: [&str; 7] = ["Q8_0", "Q6_K", "Q5_K_M", "Q4_K_M", "UD-Q4_K_XL", "IQ4_XS", "Q3_K_M"];

/// The rank of a quantization in the suggestion order: 0 is best. None for names outside it.
pub fn quality_rank(name: &str) -> Option<u8> {
    QUALITY.iter().position(|known| known.eq_ignore_ascii_case(name)).map(|rank| rank as u8)
}

/// Whether a quantization keeps fewer than about 3.5 bits per weight, where quality drops noticeably.
pub fn is_low_quant(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    let bits = upper
        .trim_start_matches("UD-")
        .trim_start_matches('I')
        .strip_prefix('Q')
        .and_then(|rest| rest.chars().next())
        .and_then(|digit| digit.to_digit(10));
    matches!(bits, Some(1 | 2)) || upper.contains("Q3_K_S") || upper.starts_with("IQ3")
}

pub fn group(files: &[RepoFile]) -> Vec<Variant> {
    let ggufs = files.iter().filter(|file| file.path.to_ascii_lowercase().ends_with(".gguf"));
    let (mmproj, models): (Vec<&RepoFile>, Vec<&RepoFile>) =
        ggufs.partition(|file| file_name(&file.path).to_ascii_lowercase().starts_with("mmproj"));
    let mut groups: BTreeMap<String, Vec<RepoFile>> = BTreeMap::new();
    for file in models {
        groups.entry(variant_name(&file.path)).or_default().push(file.clone());
    }
    let mmproj: Vec<RepoFile> = mmproj.into_iter().cloned().collect();
    let mut variants: Vec<Variant> = groups
        .into_iter()
        .map(|(name, mut files)| {
            files.sort_by(|a, b| a.path.cmp(&b.path));
            Variant { size: files.iter().map(|file| file.size).sum(), name, files, mmproj: mmproj.clone() }
        })
        .collect();
    variants.sort_by_key(|variant| (quality_rank(&variant.name).unwrap_or(u8::MAX), variant.name.clone()));
    variants
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The quantization in a file name, without the split suffix. Files in a quantization folder use the folder.
fn variant_name(path: &str) -> String {
    let stem = file_name(path);
    let stem = &stem[..stem.len() - ".gguf".len()];
    let stem = strip_split(stem);
    quant_in(stem)
        .or_else(|| path.rsplit('/').nth(1).and_then(quant_in))
        .unwrap_or_else(|| stem.to_string())
}

/// `name-00001-of-00003` becomes `name`.
fn strip_split(stem: &str) -> &str {
    let parts: Vec<&str> = stem.rsplitn(4, '-').collect();
    match parts.as_slice() {
        [total, "of", part, rest]
            if total.len() == 5 && part.len() == 5 && total.bytes().chain(part.bytes()).all(|b| b.is_ascii_digit()) =>
        {
            rest
        }
        _ => stem,
    }
}

/// The last `-` or `.` separated part that names a quantization, with a `UD-` prefix kept.
fn quant_in(stem: &str) -> Option<String> {
    let parts: Vec<&str> = stem.split(['-', '.']).collect();
    let index = parts.iter().rposition(|part| is_quant(part))?;
    let name = parts[index].to_ascii_uppercase();
    Some(match index.checked_sub(1).map(|before| parts[before]) {
        Some(prefix) if prefix.eq_ignore_ascii_case("UD") => format!("UD-{name}"),
        _ => name,
    })
}

fn is_quant(part: &str) -> bool {
    let upper = part.to_ascii_uppercase();
    if matches!(upper.as_str(), "F16" | "BF16" | "F32" | "MXFP4") || upper.starts_with("MXFP4_") {
        return true;
    }
    let rest = upper.strip_prefix("IQ").or_else(|| upper.strip_prefix('Q'));
    rest.is_some_and(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_digit()) && rest.len() <= 6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(variant_name("Qwen3-4B-Instruct-2507-Q4_K_M.gguf"), "Q4_K_M");
        assert_eq!(variant_name("Qwen3-4B-Instruct-2507-UD-Q4_K_XL.gguf"), "UD-Q4_K_XL");
        assert_eq!(variant_name("Q8_0/model-Q8_0-00001-of-00002.gguf"), "Q8_0");
        assert_eq!(variant_name("BF16/model-00001-of-00002.gguf"), "BF16");
        assert_eq!(variant_name("gemma-3-4b-it-qat-q4_0.gguf"), "Q4_0");
        assert_eq!(variant_name("model.IQ4_XS.gguf"), "IQ4_XS");
        assert_eq!(variant_name("plain.gguf"), "plain");
        assert!(is_low_quant("Q2_K") && is_low_quant("IQ2_XXS") && is_low_quant("UD-IQ1_S") && is_low_quant("IQ3_XXS"));
        assert!(!is_low_quant("Q3_K_M") && !is_low_quant("Q4_K_M") && !is_low_quant("BF16"));
    }
}
