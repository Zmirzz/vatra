//! Web search through several providers in a chain, with the repair a person would do by hand: a cleaner query when the
//! first one is noisy, a looser one when nothing comes back, the next provider when one fails. Results are de-duplicated
//! and ordered by how well they match the question. Scholarly sources (arXiv, OpenAlex) are searched separately.
//!
//! Everything that comes back is text written by strangers; callers treat it as data.

use std::collections::HashSet;
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};
use url::Url;

use super::web_search::SearchHit;

const TIMEOUT: Duration = Duration::from_secs(20);
const USER_AGENT: &str = concat!("Vatra/", env!("CARGO_PKG_VERSION"));
const MAX_HITS: usize = 10;

/// A search provider and what it needs. Keys are held here only for the length of a search.
#[derive(Clone)]
pub enum Provider {
    Brave { key: String },
    DuckDuckGo,
    Searxng { base: String },
    Tavily { key: String },
    Serper { key: String },
}

impl Provider {
    pub fn name(&self) -> &'static str {
        match self {
            Provider::Brave { .. } => "brave",
            Provider::DuckDuckGo => "duckduckgo",
            Provider::Searxng { .. } => "searxng",
            Provider::Tavily { .. } => "tavily",
            Provider::Serper { .. } => "serper",
        }
    }
}

/// Where each provider is reached. Tests point these at a local server; nothing else changes them.
#[derive(Clone)]
pub struct Endpoints {
    pub brave: String,
    pub duckduckgo: String,
    pub tavily: String,
    pub serper: String,
    pub arxiv: String,
    pub openalex: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints {
            brave: "https://api.search.brave.com/res/v1/web/search".into(),
            duckduckgo: "https://html.duckduckgo.com/html/".into(),
            tavily: "https://api.tavily.com/search".into(),
            serper: "https://google.serper.dev/search".into(),
            arxiv: "https://export.arxiv.org/api/query".into(),
            openalex: "https://api.openalex.org/works".into(),
        }
    }
}

/// What one search did, provider by provider, so a failure is explained instead of hidden.
#[derive(Clone, Debug, Default)]
pub struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    pub provider: Option<&'static str>,
    pub attempts: Vec<(&'static str, Result<usize, String>)>,
}

pub struct SearchChain {
    providers: Vec<Provider>,
    endpoints: Endpoints,
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder().timeout(TIMEOUT).user_agent(USER_AGENT).build().map_err(|e| e.to_string())
}

impl SearchChain {
    pub fn new(providers: Vec<Provider>) -> SearchChain {
        SearchChain { providers, endpoints: Endpoints::default() }
    }

    pub fn with_endpoints(providers: Vec<Provider>, endpoints: Endpoints) -> SearchChain {
        SearchChain { providers, endpoints }
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.providers.iter().map(Provider::name).collect()
    }

    /// Searches until a provider gives results. The question steers the order of what is returned.
    pub async fn search(&self, query: &str, question: &str) -> SearchOutcome {
        let mut outcome = SearchOutcome::default();
        let cleaned = clean_query(query);
        if cleaned.is_empty() {
            return outcome;
        }
        let loose = relax_query(&cleaned);
        for provider in &self.providers {
            let mut tried = vec![cleaned.clone()];
            if loose != cleaned && !loose.is_empty() {
                tried.push(loose.clone());
            }
            let mut result: Result<Vec<SearchHit>, String> = Ok(vec![]);
            for text in &tried {
                result = self.ask(provider, text).await;
                if matches!(&result, Ok(hits) if !hits.is_empty()) || result.is_err() {
                    break;
                }
            }
            match result {
                Ok(hits) => {
                    let ranked = rank(hits, if question.is_empty() { &cleaned } else { question });
                    outcome.attempts.push((provider.name(), Ok(ranked.len())));
                    if !ranked.is_empty() {
                        outcome.hits = ranked;
                        outcome.provider = Some(provider.name());
                        return outcome;
                    }
                }
                Err(message) => outcome.attempts.push((provider.name(), Err(message))),
            }
        }
        outcome
    }

    async fn ask(&self, provider: &Provider, query: &str) -> Result<Vec<SearchHit>, String> {
        let client = client()?;
        match provider {
            Provider::Brave { key } => {
                let mut url = Url::parse(&self.endpoints.brave).map_err(|e| e.to_string())?;
                url.query_pairs_mut().append_pair("q", query).append_pair("count", "10");
                let body: Value = send_json(client.get(url).header("X-Subscription-Token", key).header("Accept", "application/json")).await?;
                Ok(body["web"]["results"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|r| hit(&r["title"], &r["url"], &r["description"]))
                    .collect())
            }
            Provider::DuckDuckGo => {
                let mut url = Url::parse(&self.endpoints.duckduckgo).map_err(|e| e.to_string())?;
                url.query_pairs_mut().append_pair("q", query);
                let response = client.get(url).send().await.map_err(|_| "DuckDuckGo couldn't be reached.".to_string())?;
                if !response.status().is_success() {
                    return Err(format!("DuckDuckGo answered {}.", response.status().as_u16()));
                }
                let html = response.text().await.map_err(|_| "DuckDuckGo answered something unreadable.".to_string())?;
                if html.contains("anomaly-modal") || html.contains("g-recaptcha") {
                    return Err("DuckDuckGo asked for a check that only a person can pass.".into());
                }
                Ok(parse_duckduckgo(&html))
            }
            Provider::Searxng { base } => {
                let mut url = Url::parse(&format!("{}/search", base.trim_end_matches('/'))).map_err(|_| "The SearXNG address isn't valid.".to_string())?;
                url.query_pairs_mut().append_pair("q", query).append_pair("format", "json");
                let body: Value = send_json(client.get(url)).await?;
                Ok(body["results"].as_array().into_iter().flatten().filter_map(|r| hit(&r["title"], &r["url"], &r["content"])).collect())
            }
            Provider::Tavily { key } => {
                let body: Value = send_json(client.post(&self.endpoints.tavily).bearer_auth(key).json(&json!({"query": query, "max_results": 10}))).await?;
                Ok(body["results"].as_array().into_iter().flatten().filter_map(|r| hit(&r["title"], &r["url"], &r["content"])).collect())
            }
            Provider::Serper { key } => {
                let body: Value = send_json(client.post(&self.endpoints.serper).header("X-API-KEY", key).json(&json!({"q": query, "num": 10}))).await?;
                Ok(body["organic"].as_array().into_iter().flatten().filter_map(|r| hit(&r["title"], &r["link"], &r["snippet"])).collect())
            }
        }
    }

    /// Papers matching a question, from arXiv and OpenAlex. A source that fails is skipped.
    pub async fn scholarly(&self, query: &str) -> Vec<SearchHit> {
        let query = clean_query(query);
        if query.is_empty() {
            return vec![];
        }
        let Ok(client) = client() else { return vec![] };
        let mut all = Vec::new();
        if let Ok(mut url) = Url::parse(&self.endpoints.arxiv) {
            url.query_pairs_mut().append_pair("search_query", &format!("all:{query}")).append_pair("max_results", "5");
            if let Ok(response) = client.get(url).send().await
                && let Ok(text) = response.text().await
            {
                all.extend(parse_arxiv(&text));
            }
        }
        if let Ok(mut url) = Url::parse(&self.endpoints.openalex) {
            url.query_pairs_mut().append_pair("search", &query).append_pair("per-page", "5");
            if let Ok(response) = client.get(url).send().await
                && let Ok(body) = response.json::<Value>().await
            {
                all.extend(parse_openalex(&body));
            }
        }
        rank(all, &query)
    }
}

async fn send_json(request: reqwest::RequestBuilder) -> Result<Value, String> {
    let response = request.send().await.map_err(|_| "The search service couldn't be reached.".to_string())?;
    let status = response.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err("The search service refused the key.".into());
    }
    if status.as_u16() == 429 {
        return Err("The search service says there were too many requests.".into());
    }
    if !status.is_success() {
        return Err(format!("The search service answered {}.", status.as_u16()));
    }
    response.json().await.map_err(|_| "The search service answered something unreadable.".to_string())
}

fn hit(title: &Value, url: &Value, description: &Value) -> Option<SearchHit> {
    let url = url.as_str()?.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    Some(SearchHit {
        title: title.as_str().unwrap_or(url).trim().to_string(),
        url: url.to_string(),
        description: description.as_str().unwrap_or("").trim().to_string(),
    })
}

// ------------------------------------------------------------------------------------------------ query repair

/// A query as a search engine wants it: no markdown, code fences, wrapping quotes or filler, at most 200 characters.
pub fn clean_query(query: &str) -> String {
    let mut text = query.replace("```", " ").replace(['`', '*', '#', '>', '\r', '\n', '\t'], " ");
    let lower = text.trim().to_lowercase();
    for prefix in ["search for ", "search the web for ", "look up ", "google ", "find ", "caută ", "cauta "] {
        if lower.starts_with(prefix) {
            text = text.trim()[prefix.len()..].to_string();
            break;
        }
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // Quotes that wrap the whole query go; quotes around a phrase inside it stay, because they mean something.
    let wrapped = [('"', '"'), ('\'', '\''), ('“', '”')]
        .iter()
        .any(|(open, close)| text.starts_with(*open) && text.ends_with(*close) && text.matches(*open).count() <= 2 && text.chars().count() >= 2);
    let text = if wrapped { text.chars().skip(1).take(text.chars().count() - 2).collect::<String>() } else { text };
    text.trim().chars().take(200).collect()
}

/// A looser version for an empty answer: operators and quoted phrases go, and only the significant words stay.
pub fn relax_query(query: &str) -> String {
    let words: Vec<&str> = query
        .split_whitespace()
        .filter(|w| !w.contains(':') && !w.starts_with('-') && !w.starts_with('+') && !w.starts_with("OR"))
        .collect();
    let significant: Vec<String> = words
        .iter()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|w| w.chars().count() > 2 && !STOP_WORDS.contains(&w.to_lowercase().as_str()))
        .take(7)
        .collect();
    significant.join(" ")
}

const STOP_WORDS: [&str; 34] = [
    "the", "and", "for", "with", "that", "this", "from", "what", "which", "when", "where", "how", "why", "are", "was", "does", "about", "into", "have",
    "este", "sunt", "care", "cum", "ce", "pentru", "din", "unde", "când", "cand", "despre", "într", "sau", "dar", "cat",
];

// ------------------------------------------------------------------------------------------------ ranking

fn normalize_url(address: &str) -> String {
    let Ok(mut url) = Url::parse(address) else { return address.to_string() };
    url.set_fragment(None);
    let keep: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !(k.starts_with("utm_") || ["fbclid", "gclid", "ref", "ref_src"].contains(&k.as_ref())))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.set_query(None);
    if !keep.is_empty() {
        url.query_pairs_mut().extend_pairs(keep);
    }
    let host = url.host_str().unwrap_or("").trim_start_matches("www.").to_lowercase();
    format!("{host}{}{}", url.path().trim_end_matches('/'), url.query().map(|q| format!("?{q}")).unwrap_or_default())
}

fn terms(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| w.chars().count() > 2 && !STOP_WORDS.contains(&w.as_str()))
        .collect()
}

/// Drops repeats, scores each result by the question's words in its title (counted twice) and description, and keeps
/// the best ten. Results that share nothing with the question are dropped when enough others do.
pub fn rank(hits: Vec<SearchHit>, question: &str) -> Vec<SearchHit> {
    let wanted = terms(question);
    let mut seen = HashSet::new();
    let mut scored: Vec<(usize, usize, SearchHit)> = Vec::new();
    for (position, hit) in hits.into_iter().enumerate() {
        if !seen.insert(normalize_url(&hit.url)) {
            continue;
        }
        let title = terms(&hit.title);
        let body = terms(&hit.description);
        let score = wanted.iter().map(|w| if title.contains(w) { 2 } else { 0 } + usize::from(body.contains(w))).sum::<usize>();
        scored.push((score, position, hit));
    }
    let relevant = scored.iter().filter(|(s, _, _)| *s > 0).count();
    if relevant >= 3 {
        scored.retain(|(s, _, _)| *s > 0);
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(MAX_HITS).map(|(_, _, hit)| hit).collect()
}

// ------------------------------------------------------------------------------------------------ parsers

fn unescape(text: &str) -> String {
    text.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#x27;", "'").replace("&#39;", "'").replace("&nbsp;", " ")
}

fn strip_tags(html: &str) -> String {
    let tag = Regex::new(r"<[^>]*>").expect("regex");
    unescape(&tag.replace_all(html, "")).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Results of the DuckDuckGo HTML page: each link goes through a redirect that carries the real address in `uddg`.
pub fn parse_duckduckgo(html: &str) -> Vec<SearchHit> {
    let link = Regex::new(r#"(?s)<a[^>]*class="[^"]*result__a[^"]*"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#).expect("regex");
    let snippet = Regex::new(r#"(?s)class="[^"]*result__snippet[^"]*"[^>]*>(.*?)</a>|class="[^"]*result__snippet[^"]*"[^>]*>(.*?)</(?:div|td)>"#).expect("regex");
    let snippets: Vec<String> = snippet
        .captures_iter(html)
        .map(|c| strip_tags(c.get(1).or_else(|| c.get(2)).map_or("", |m| m.as_str())))
        .collect();
    let mut out = Vec::new();
    for (i, c) in link.captures_iter(html).enumerate() {
        let href = unescape(&c[1]);
        let absolute = if href.starts_with("//") { format!("https:{href}") } else { href };
        let target = Url::parse(&absolute).ok().and_then(|u| {
            if u.path().starts_with("/l/") {
                u.query_pairs().find(|(k, _)| k == "uddg").map(|(_, v)| v.into_owned())
            } else {
                Some(u.to_string())
            }
        });
        let Some(target) = target else { continue };
        if !(target.starts_with("http://") || target.starts_with("https://")) || target.contains("duckduckgo.com/y.js") {
            continue;
        }
        out.push(SearchHit { title: strip_tags(&c[2]), url: target, description: snippets.get(i).cloned().unwrap_or_default() });
    }
    out
}

pub fn parse_arxiv(atom: &str) -> Vec<SearchHit> {
    let entry = Regex::new(r"(?s)<entry>(.*?)</entry>").expect("regex");
    let field = |body: &str, name: &str| -> String {
        Regex::new(&format!(r"(?s)<{name}[^>]*>(.*?)</{name}>")).ok().and_then(|r| r.captures(body).map(|c| strip_tags(&c[1]))).unwrap_or_default()
    };
    entry
        .captures_iter(atom)
        .filter_map(|c| {
            let body = &c[1];
            let url = field(body, "id");
            (url.starts_with("http")).then(|| SearchHit { title: format!("{} (arXiv)", field(body, "title")), url, description: field(body, "summary") })
        })
        .collect()
}

pub fn parse_openalex(body: &Value) -> Vec<SearchHit> {
    body["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|work| {
            let url = work["doi"].as_str().or_else(|| work["primary_location"]["landing_page_url"].as_str()).or_else(|| work["id"].as_str())?;
            // OpenAlex sends abstracts as a map from each word to its positions.
            let mut words: Vec<(u64, &str)> = Vec::new();
            if let Some(index) = work["abstract_inverted_index"].as_object() {
                for (word, positions) in index {
                    for p in positions.as_array().into_iter().flatten().filter_map(Value::as_u64) {
                        words.push((p, word.as_str()));
                    }
                }
            }
            words.sort_by_key(|(p, _)| *p);
            let abstract_text = words.into_iter().map(|(_, w)| w).collect::<Vec<_>>().join(" ");
            let year = work["publication_year"].as_u64().map(|y| format!(" ({y})")).unwrap_or_default();
            Some(SearchHit {
                title: format!("{}{year}", work["title"].as_str().or_else(|| work["display_name"].as_str()).unwrap_or(url)),
                url: url.to_string(),
                description: abstract_text,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(title: &str, url: &str, description: &str) -> SearchHit {
        SearchHit { title: title.into(), url: url.into(), description: description.into() }
    }

    #[test]
    fn queries_are_cleaned_and_relaxed() {
        assert_eq!(clean_query("```\nsearch for \"rust async runtimes\"\n```"), "rust async runtimes");
        assert_eq!(clean_query("**caută** pret abonament"), "pret abonament");
        assert_eq!(relax_query("\"exact phrase\" site:example.com what is the price of tokio runtime -java"), "exact phrase price tokio runtime");
        assert_eq!(clean_query(&"a".repeat(500)).chars().count(), 200);
    }

    #[test]
    fn results_are_deduplicated_and_ordered_by_the_question() {
        let hits = vec![
            h("Cooking pasta", "https://a.example/pasta", "Boil water"),
            h("Tokio runtime guide", "https://docs.example/tokio?utm_source=x", "How the tokio runtime schedules tasks"),
            h("Tokio runtime guide", "https://www.docs.example/tokio/#top", "duplicate"),
            h("Async rust", "https://b.example/async", "tokio and async runtime internals"),
            h("Rust tokio basics", "https://c.example/basics", "runtime tokio"),
            h("Ftp", "ftp://nope", "x"),
        ];
        let ranked = rank(hits, "how does the tokio runtime work");
        assert_eq!(ranked[0].url, "https://docs.example/tokio?utm_source=x");
        assert_eq!(ranked.iter().filter(|r| r.title == "Tokio runtime guide").count(), 1);
        assert!(!ranked.iter().any(|r| r.title == "Cooking pasta"), "unrelated results leave when enough others match");
    }

    #[test]
    fn duckduckgo_html_is_read_through_its_redirects() {
        let html = r#"<div class="result"><a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.org%2Fpage%3Fa%3D1&amp;rut=abc">Example &amp; page</a>
<a class="result__snippet" href="x">A <b>short</b> snippet</a></div>
<div class="result"><a class="result__a" href="https://direct.example/x">Direct</a><a class="result__snippet" href="y">Second</a></div>
<a class="result__a" href="//duckduckgo.com/y.js?ad=1">Ad</a>"#;
        let hits = parse_duckduckgo(html);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "https://example.org/page?a=1");
        assert_eq!(hits[0].title, "Example & page");
        assert_eq!(hits[0].description, "A short snippet");
        assert_eq!(hits[1].url, "https://direct.example/x");
    }

    #[test]
    fn scholarly_sources_are_read() {
        let atom = "<feed><entry><id>http://arxiv.org/abs/2401.00001v1</id><title>Attention\n is all</title><summary>We study things.</summary></entry></feed>";
        let hits = parse_arxiv(atom);
        assert_eq!(hits[0].url, "http://arxiv.org/abs/2401.00001v1");
        assert!(hits[0].title.starts_with("Attention is all"));
        let body = json!({"results": [{"title": "A study", "doi": "https://doi.org/10.1/x", "publication_year": 2024, "abstract_inverted_index": {"Hello": [0], "world": [1]}}]});
        let found = parse_openalex(&body);
        assert_eq!(found[0].description, "Hello world");
        assert_eq!(found[0].title, "A study (2024)");
    }
}
