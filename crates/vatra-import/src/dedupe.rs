use std::time::SystemTime;

use serde::Serialize;
use vatra_extensions::Origin;

use crate::scan::{Found, FoundKind};

/// The items after duplicates are merged. Items of the same kind, name and project with the same content become one
/// item, which lists its other origins in `also_from`. Items with the same name and different content are a conflict:
/// the newest copy is kept, and the others are listed in the conflict.
#[derive(Debug, Default, PartialEq, Serialize)]
pub struct DedupeResult {
    pub items: Vec<Found>,
    pub conflicts: Vec<Conflict>,
}

/// One name with copies that differ. `kept` is the newest copy, which is in `items`; `dropped` are the others.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Conflict {
    pub kind: FoundKind,
    pub name: String,
    pub kept: Found,
    pub dropped: Vec<Found>,
}

/// Merges the copies of each item. The order of the first copy of each item is kept.
pub fn dedupe(found: Vec<Found>) -> DedupeResult {
    let mut groups: Vec<Vec<Found>> = Vec::new();
    for item in found {
        match groups.iter_mut().find(|group| same_item(&group[0], &item)) {
            Some(group) => group.push(item),
            None => groups.push(vec![item]),
        }
    }

    let mut result = DedupeResult::default();
    for group in groups {
        let mut variants: Vec<Vec<Found>> = Vec::new();
        for item in group {
            match variants.iter_mut().find(|variant| variant[0].hash == item.hash) {
                Some(variant) => variant.push(item),
                None => variants.push(vec![item]),
            }
        }

        let newest = newest_variant(&variants);
        let kept = merged(&variants[newest]);
        if variants.len() > 1 {
            let dropped = variants
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != newest)
                .map(|(_, variant)| merged(variant))
                .collect();
            result.conflicts.push(Conflict {
                kind: kept.kind,
                name: kept.name.clone(),
                kept: kept.clone(),
                dropped,
            });
        }
        result.items.push(kept);
    }
    result
}

fn same_item(a: &Found, b: &Found) -> bool {
    a.kind == b.kind && a.name == b.name && a.project == b.project
}

/// The variant whose source changed last. On a tie, or when no time is known, the first one wins.
fn newest_variant(variants: &[Vec<Found>]) -> usize {
    let mut best = 0;
    for index in 1..variants.len() {
        if newest_time(&variants[index]) > newest_time(&variants[best]) {
            best = index;
        }
    }
    best
}

fn newest_time(variant: &[Found]) -> Option<SystemTime> {
    variant.iter().filter_map(|item| item.modified).max()
}

/// One item for a variant: its first copy, with the origins of the other copies in `also_from`.
fn merged(variant: &[Found]) -> Found {
    let mut item = variant[0].clone();
    item.modified = newest_time(variant);
    for other in &variant[1..] {
        let origins = std::iter::once(other.origin).chain(other.also_from.iter().copied());
        for origin in origins {
            push_origin(&mut item, origin);
        }
    }
    item
}

fn push_origin(item: &mut Found, origin: Origin) {
    if origin != item.origin && !item.also_from.contains(&origin) {
        item.also_from.push(origin);
    }
}
