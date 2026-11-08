//! Search by meaning. Documents, notes, tasks and research reports are cut into short passages, each passage gets a vector
//! from an embedding model, and a question is answered by the passages whose vectors point the same way. Vectors are
//! kept in SQLite, normalised, so the match is a dot product. Nothing here talks to a model: callers bring the vectors.

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::store::{Store, StoreError};

pub(crate) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS semantic_chunks (
        kind TEXT NOT NULL,
        ref_id INTEGER NOT NULL,
        chunk INTEGER NOT NULL,
        project_id INTEGER,
        title TEXT NOT NULL,
        text TEXT NOT NULL,
        model TEXT NOT NULL,
        dim INTEGER NOT NULL,
        vector BLOB NOT NULL,
        PRIMARY KEY (kind, ref_id, chunk)
    );
    CREATE INDEX IF NOT EXISTS semantic_chunks_by_project ON semantic_chunks(project_id);
    CREATE TABLE IF NOT EXISTS semantic_sources (
        kind TEXT NOT NULL,
        ref_id INTEGER NOT NULL,
        source_hash TEXT NOT NULL,
        model TEXT NOT NULL,
        indexed_at INTEGER NOT NULL,
        PRIMARY KEY (kind, ref_id)
    );
";

const CHUNK_CHARS: usize = 900;
const CHUNK_OVERLAP: usize = 150;
const MAX_CHUNKS: usize = 40;
const MAX_SOURCE_CHARS: usize = 60_000;

/// Something to index: where it lives and the text to cut up.
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticSource {
    pub kind: String,
    pub ref_id: i64,
    pub project_id: Option<i64>,
    pub title: String,
    pub text: String,
    pub hash: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SemanticHit {
    pub kind: String,
    pub ref_id: i64,
    pub project_id: Option<i64>,
    pub title: String,
    pub snippet: String,
    pub score: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SemanticStats {
    pub sources: i64,
    pub chunks: i64,
    pub pending: i64,
}

/// Cuts text into passages of about 900 characters that overlap a little, breaking at a paragraph, a line or a
/// sentence when there is one near the end, so a passage reads as a unit. At most 40 passages.
pub fn chunk_text(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().take(MAX_SOURCE_CHARS).collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() && out.len() < MAX_CHUNKS {
        let mut end = (start + CHUNK_CHARS).min(chars.len());
        if end < chars.len() {
            // Look back for a good place to stop, no further than the last third of the passage.
            let floor = start + CHUNK_CHARS * 2 / 3;
            if let Some(at) = (floor..end).rev().find(|&i| chars[i] == '\n' && i > 0 && chars[i - 1] == '\n') {
                end = at + 1;
            } else if let Some(at) = (floor..end).rev().find(|&i| chars[i] == '\n') {
                end = at + 1;
            } else if let Some(at) = (floor..end).rev().find(|&i| matches!(chars[i], '.' | '!' | '?') && chars.get(i + 1).is_none_or(|c| c.is_whitespace())) {
                end = at + 1;
            }
        }
        let piece: String = chars[start..end].iter().collect();
        let piece = piece.trim();
        if piece.chars().count() >= 20 || (out.is_empty() && !piece.is_empty()) {
            out.push(piece.to_string());
        }
        if end >= chars.len() {
            break;
        }
        start = end.saturating_sub(CHUNK_OVERLAP).max(start + 1);
    }
    out
}

fn normalize(vector: &mut [f32]) -> bool {
    let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 || !norm.is_finite() {
        return false;
    }
    vector.iter_mut().for_each(|x| *x /= norm);
    true
}

fn to_bytes(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn from_bytes(bytes: &[u8]) -> Vec<f32> {
    bytes.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect()
}

fn hash_of(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    hasher.finalize().iter().take(12).map(|b| format!("{b:02x}")).collect()
}

impl Store {
    /// Up to `limit` sources whose text is new or changed since they were indexed with `model`: workspace documents,
    /// notes and tasks, and finished research reports.
    pub fn semantic_pending(&self, model: &str, limit: usize) -> Result<Vec<SemanticSource>, StoreError> {
        let connection = self.connection();
        let mut out = Vec::new();
        let mut items = connection.prepare(
            "SELECT id, project_id, title, body FROM items WHERE deleted_at IS NULL AND kind IN ('document','note','task') ORDER BY id",
        )?;
        let rows = items.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)))?;
        for row in rows {
            let (id, project, title, body) = row?;
            let hash = hash_of(&[&title, &body]);
            if self.is_current(&connection, "item", id, &hash, model)? {
                continue;
            }
            out.push(SemanticSource { kind: "item".into(), ref_id: id, project_id: project, title, text: body, hash });
            if out.len() >= limit {
                return Ok(out);
            }
        }
        let mut runs = connection.prepare("SELECT id, question, report FROM research_runs WHERE state='done' AND report IS NOT NULL ORDER BY id")?;
        let rows = runs.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
        for row in rows {
            let (id, question, report) = row?;
            let hash = hash_of(&[&question, &report]);
            if self.is_current(&connection, "research", id, &hash, model)? {
                continue;
            }
            out.push(SemanticSource { kind: "research".into(), ref_id: id, project_id: None, title: question, text: report, hash });
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    fn is_current(&self, connection: &rusqlite::Connection, kind: &str, id: i64, hash: &str, model: &str) -> Result<bool, StoreError> {
        let found: Option<(String, String)> = connection
            .query_row("SELECT source_hash, model FROM semantic_sources WHERE kind=?1 AND ref_id=?2", params![kind, id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        Ok(found.is_some_and(|(h, m)| h == hash && m == model))
    }

    /// Replaces the passages of one source with these (text and vector pairs), and remembers what was indexed.
    pub fn semantic_store(&self, source: &SemanticSource, model: &str, passages: Vec<(String, Vec<f32>)>, now: i64) -> Result<(), StoreError> {
        let mut connection = self.connection();
        let tx = connection.transaction()?;
        tx.execute("DELETE FROM semantic_chunks WHERE kind=?1 AND ref_id=?2", params![source.kind, source.ref_id])?;
        for (index, (text, mut vector)) in passages.into_iter().enumerate() {
            if !normalize(&mut vector) {
                continue;
            }
            tx.execute(
                "INSERT INTO semantic_chunks (kind,ref_id,chunk,project_id,title,text,model,dim,vector) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![source.kind, source.ref_id, index as i64, source.project_id, source.title, text, model, vector.len() as i64, to_bytes(&vector)],
            )?;
        }
        tx.execute(
            "INSERT INTO semantic_sources (kind,ref_id,source_hash,model,indexed_at) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(kind,ref_id) DO UPDATE SET source_hash=excluded.source_hash,model=excluded.model,indexed_at=excluded.indexed_at",
            params![source.kind, source.ref_id, source.hash, model, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Removes the passages of anything that is gone (a deleted item, a deleted research, a deleted project).
    pub fn semantic_prune(&self) -> Result<usize, StoreError> {
        let mut connection = self.connection();
        let tx = connection.transaction()?;
        let mut removed = 0;
        for (table, kind, live) in [
            ("semantic_chunks", "item", "SELECT id FROM items WHERE deleted_at IS NULL"),
            ("semantic_sources", "item", "SELECT id FROM items WHERE deleted_at IS NULL"),
            ("semantic_chunks", "research", "SELECT id FROM research_runs WHERE state='done'"),
            ("semantic_sources", "research", "SELECT id FROM research_runs WHERE state='done'"),
        ] {
            removed += tx.execute(&format!("DELETE FROM {table} WHERE kind='{kind}' AND ref_id NOT IN ({live})"), [])?;
        }
        tx.commit()?;
        Ok(removed)
    }

    /// Forgets what was indexed, so every source is embedded again (after the model changed, say).
    pub fn semantic_reset(&self) -> Result<(), StoreError> {
        let connection = self.connection();
        connection.execute("DELETE FROM semantic_sources", [])?;
        connection.execute("DELETE FROM semantic_chunks", [])?;
        Ok(())
    }

    pub fn semantic_stats(&self, model: &str) -> Result<SemanticStats, StoreError> {
        let pending = self.semantic_pending(model, 100_000)?.len() as i64;
        let connection = self.connection();
        let sources: i64 = connection.query_row("SELECT COUNT(*) FROM semantic_sources WHERE model=?1", [model], |r| r.get(0))?;
        let chunks: i64 = connection.query_row("SELECT COUNT(*) FROM semantic_chunks WHERE model=?1", [model], |r| r.get(0))?;
        Ok(SemanticStats { sources, chunks, pending })
    }

    /// The best passages for a question vector, one per source, best first. `kinds` and `project` narrow the search.
    pub fn semantic_search(&self, model: &str, query: &[f32], limit: usize, kinds: &[String], project: Option<i64>) -> Result<Vec<SemanticHit>, StoreError> {
        let mut query = query.to_vec();
        if !normalize(&mut query) {
            return Ok(vec![]);
        }
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT kind, ref_id, project_id, title, text, dim, vector FROM semantic_chunks WHERE model=?1 AND (?2 IS NULL OR project_id=?2)",
        )?;
        let rows = statement.query_map(params![model, project], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<i64>>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, i64>(5)?, r.get::<_, Vec<u8>>(6)?))
        })?;
        let mut best: std::collections::HashMap<(String, i64), SemanticHit> = std::collections::HashMap::new();
        for row in rows {
            let (kind, ref_id, project_id, title, text, dim, bytes) = row?;
            if !kinds.is_empty() && !kinds.contains(&kind) {
                continue;
            }
            if dim as usize != query.len() {
                continue;
            }
            let vector = from_bytes(&bytes);
            let score: f32 = vector.iter().zip(&query).map(|(a, b)| a * b).sum();
            let entry = best.entry((kind.clone(), ref_id));
            match entry {
                std::collections::hash_map::Entry::Occupied(mut found) => {
                    if score > found.get().score {
                        found.get_mut().score = score;
                        found.get_mut().snippet = text.chars().take(260).collect();
                    }
                }
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(SemanticHit { kind, ref_id, project_id, title, snippet: text.chars().take(260).collect(), score });
                }
            }
        }
        let mut hits: Vec<SemanticHit> = best.into_values().collect();
        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(limit.clamp(1, 50));
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NewItem;

    #[test]
    fn text_is_cut_at_natural_places_with_a_little_overlap() {
        let paragraph = "Aceasta este o propozitie despre abonamente si preturi. ".repeat(8);
        let text = format!("{paragraph}\n\n{paragraph}\n\n{paragraph}\n\n{paragraph}");
        let chunks = chunk_text(&text);
        assert!(chunks.len() >= 3, "{}", chunks.len());
        assert!(chunks.iter().all(|c| c.chars().count() <= CHUNK_CHARS + 5));
        assert!(chunks[0].ends_with('.'), "a passage stops at the end of a sentence: {:?}", &chunks[0][chunks[0].len().saturating_sub(20)..]);
        // The overlap: the start of the next passage appears at the end of the one before.
        let tail: String = chunks[0].chars().rev().take(60).collect::<Vec<_>>().into_iter().rev().collect();
        assert!(chunks[1].contains(tail.trim().split(' ').next_back().unwrap()) || chunks[1].len() > 20);
        assert_eq!(chunk_text("scurt"), vec!["scurt"]);
        assert!(chunk_text("   ").is_empty());
        assert!(chunk_text(&"x ".repeat(100_000)).len() <= MAX_CHUNKS);
    }

    fn note(store: &Store, project: i64, title: &str, body: &str) -> i64 {
        store.create_item(&NewItem { kind: "note".into(), title: title.into(), body: body.into(), fields: serde_json::json!({"format": "text"}), project_id: Some(project) }, 1).unwrap()
    }

    #[test]
    fn what_changed_is_indexed_again_and_what_is_gone_is_pruned() {
        let store = Store::open_in_memory().unwrap();
        let project = store.create_project("p").unwrap();
        let a = note(&store, project, "Retete", "supa de pui");
        let _b = note(&store, project, "Buget", "chirie si utilitati");
        let pending = store.semantic_pending("m1", 10).unwrap();
        assert_eq!(pending.len(), 2);
        store.semantic_store(&pending[0], "m1", vec![("supa".into(), vec![1.0, 0.0])], 5).unwrap();
        assert_eq!(store.semantic_pending("m1", 10).unwrap().len(), 1);
        // Another model means the vectors can't be compared, so everything is pending again.
        assert_eq!(store.semantic_pending("m2", 10).unwrap().len(), 2);
        assert!(store.delete_item(a, 9).unwrap());
        assert_eq!(store.semantic_prune().unwrap(), 2, "its source row and its passage go");
        assert_eq!(store.semantic_stats("m1").unwrap().chunks, 0);
    }

    #[test]
    fn the_closest_passage_wins_one_hit_per_source_and_filters_apply() {
        let store = Store::open_in_memory().unwrap();
        let project = store.create_project("p").unwrap();
        let other = store.create_project("q").unwrap();
        let a = note(&store, project, "Retete", "supa");
        let b = note(&store, other, "Buget", "chirie");
        let sources = store.semantic_pending("m", 10).unwrap();
        let source = |id: i64| sources.iter().find(|s| s.ref_id == id).unwrap().clone();
        store.semantic_store(&source(a), "m", vec![("supa de pui".into(), vec![1.0, 0.1]), ("ciorba de legume".into(), vec![0.9, 0.3])], 1).unwrap();
        store.semantic_store(&source(b), "m", vec![("chirie lunara".into(), vec![0.0, 1.0])], 1).unwrap();
        let hits = store.semantic_search("m", &[1.0, 0.0], 5, &[], None).unwrap();
        assert_eq!(hits.len(), 2, "one hit per source");
        assert_eq!(hits[0].title, "Retete");
        assert_eq!(hits[0].snippet, "supa de pui", "the best passage of the source is shown");
        assert!(hits[0].score > hits[1].score);
        assert_eq!(store.semantic_search("m", &[1.0, 0.0], 5, &[], Some(other)).unwrap().len(), 1);
        assert!(store.semantic_search("m", &[1.0, 0.0], 5, &["research".into()], None).unwrap().is_empty());
        // A vector of another size can't be compared and is skipped, not a crash.
        assert!(store.semantic_search("m", &[1.0, 0.0, 0.0], 5, &[], None).unwrap().is_empty());
        assert!(store.semantic_search("m", &[0.0, 0.0], 5, &[], None).unwrap().is_empty());
    }
}
