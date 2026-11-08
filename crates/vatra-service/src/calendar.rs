//! Calendar files and CalDAV. An iCalendar file is read and written here, and a CalDAV account (a connector of kind
//! `caldav`: the address of one calendar, a user name, and a password in the keychain) is read to the local
//! calendar and written back when the person changes an event. The server's copy is never changed by anything but a
//! person's own edit in the app or an action they approved.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use vatra_core::{
    CalendarEvent, NewCalendarEvent, Store, StoreError, format_ical_time, parse_ical_time, parse_rrule,
};

use crate::connectors::{ConnectorKind, connector_secret};
use crate::service::{Notification, Service};

const MAX_EVENTS: usize = 5000;
const MAX_BODY: usize = 16 * 1024 * 1024;

// ------------------------------------------------------------------------------------------- iCalendar

fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        if (raw.starts_with(' ') || raw.starts_with('\t')) && !lines.is_empty() {
            lines.last_mut().expect("checked").push_str(&raw[1..]);
        } else {
            lines.push(raw.to_string());
        }
    }
    lines
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace("\r\n", "\\n")
        .replace('\n', "\\n")
}

/// Splits `DTSTART;TZID=Europe/Bucharest:20261107T090000` into the name, its parameters and the value.
fn property(line: &str) -> Option<(String, BTreeMap<String, String>, String)> {
    let colon = line.find(':')?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.to_ascii_uppercase();
    let params = parts
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_ascii_uppercase(), v.trim_matches('"').to_string()))
        .collect();
    Some((name, params, value.to_string()))
}

/// The offset from UTC, in seconds, of a time zone name at a given local time. Only the zones people most often meet
/// are known (the European ones follow the same daylight-saving rule); any other name is read as UTC.
fn zone_offset(zone: &str, local: i64) -> i64 {
    let (base, dst) = match zone {
        "UTC" | "GMT" | "Etc/UTC" => (0, false),
        "Europe/London" | "Europe/Dublin" | "Europe/Lisbon" | "Atlantic/Reykjavik" => (0, true),
        "Europe/Berlin" | "Europe/Paris" | "Europe/Madrid" | "Europe/Rome" | "Europe/Amsterdam" | "Europe/Brussels"
        | "Europe/Vienna" | "Europe/Prague" | "Europe/Warsaw" | "Europe/Stockholm" | "Europe/Zurich"
        | "Europe/Budapest" | "Europe/Oslo" | "Europe/Copenhagen" => (3600, true),
        "Europe/Bucharest" | "Europe/Athens" | "Europe/Helsinki" | "Europe/Kiev" | "Europe/Kyiv" | "Europe/Sofia"
        | "Europe/Riga" | "Europe/Vilnius" | "Europe/Tallinn" | "Europe/Chisinau" => (7200, true),
        "Europe/Istanbul" | "Europe/Moscow" => (10800, false),
        _ => (0, false),
    };
    if !dst {
        return base;
    }
    // EU rule: summer time from the last Sunday of March to the last Sunday of October, at 01:00 UTC.
    let year = vatra_core_civil_year(local);
    let start = last_sunday(year, 3) * 86_400 + 3600 + base;
    let end = last_sunday(year, 10) * 86_400 + 3600 + base + 3600;
    if (start..end).contains(&local) { base + 3600 } else { base }
}

fn vatra_core_civil_year(at: i64) -> i64 {
    // Year of a timestamp, using the same civil arithmetic as the formatter.
    let text = format_ical_time(at, true);
    text[..4].parse().unwrap_or(1970)
}

fn last_sunday(year: i64, month: u32) -> i64 {
    // The last day of the month, then back to a Sunday (weekday 0 with 1970-01-01 a Thursday).
    let next_month_first = if month == 12 { format!("{:04}0101", year + 1) } else { format!("{year:04}{:02}01", month + 1) };
    let first = parse_ical_time(&next_month_first).map_or(0, |(t, _)| t) / 86_400;
    let last = first - 1;
    let weekday = (last + 4).rem_euclid(7);
    last - weekday
}

fn event_time(value: &str, params: &BTreeMap<String, String>) -> Option<(i64, bool)> {
    let (at, whole) = parse_ical_time(value)?;
    if whole || value.ends_with('Z') {
        return Some((at, whole));
    }
    match params.get("TZID") {
        Some(zone) => Some((at - zone_offset(zone, at), false)),
        None => Some((at, false)),
    }
}

fn duration_secs(value: &str) -> Option<i64> {
    let value = value.trim();
    let (negative, rest) = match value.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, value.trim_start_matches('+')),
    };
    let rest = rest.strip_prefix('P')?;
    let mut total = 0i64;
    let mut number = String::new();
    let mut in_time = false;
    for c in rest.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => number.push(c),
            'W' | 'D' | 'H' | 'M' | 'S' => {
                let n: i64 = number.parse().ok()?;
                number.clear();
                total += n * match (c, in_time) {
                    ('W', _) => 604_800,
                    ('D', _) => 86_400,
                    ('H', true) => 3600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                };
            }
            _ => return None,
        }
    }
    Some(if negative { -total } else { total })
}

type Fields = BTreeMap<String, (BTreeMap<String, String>, String)>;

/// The events of an iCalendar text. An event that can't be read is left out.
pub fn parse_ics(text: &str) -> Vec<NewCalendarEvent> {
    let mut events = Vec::new();
    let mut current: Option<Fields> = None;
    for line in unfold(text) {
        let upper = line.to_ascii_uppercase();
        if upper == "BEGIN:VEVENT" {
            current = Some(BTreeMap::new());
        } else if upper == "END:VEVENT" {
            if let Some(fields) = current.take()
                && let Some(event) = build_event(fields)
            {
                events.push(event);
                if events.len() >= MAX_EVENTS {
                    break;
                }
            }
        } else if let Some(fields) = current.as_mut()
            && let Some((name, params, value)) = property(&line)
        {
            fields.entry(name).or_insert((params, value));
        }
    }
    events
}

fn build_event(fields: Fields) -> Option<NewCalendarEvent> {
    let get = |name: &str| fields.get(name);
    let (params, value) = get("DTSTART")?;
    let (starts_at, all_day) = event_time(value, params)?;
    let ends_at = if let Some((p, v)) = get("DTEND") {
        event_time(v, p)?.0
    } else if let Some((_, v)) = get("DURATION") {
        starts_at + duration_secs(v)?
    } else if all_day {
        starts_at + 86_400
    } else {
        starts_at
    };
    if ends_at < starts_at {
        return None;
    }
    let text = |name: &str| get(name).map(|(_, v)| unescape(v)).unwrap_or_default();
    let rrule = get("RRULE").map(|(_, v)| v.clone()).filter(|rule| parse_rrule(rule).is_ok());
    let title = text("SUMMARY");
    Some(NewCalendarEvent {
        calendar: "local".into(),
        uid: get("UID").map(|(_, v)| v.trim().to_string()).filter(|u| !u.is_empty()).unwrap_or_default(),
        title: if title.trim().is_empty() { "(no title)".into() } else { title },
        notes: text("DESCRIPTION"),
        location: text("LOCATION"),
        starts_at,
        ends_at,
        all_day,
        rrule,
        source: "ics".into(),
        href: None,
        etag: None,
    })
}

fn event_ics(event: &CalendarEvent) -> String {
    let mut out = String::new();
    out.push_str("BEGIN:VEVENT\r\n");
    out.push_str(&format!("UID:{}\r\n", event.uid));
    out.push_str(&format!("DTSTAMP:{}\r\n", format_ical_time(event.updated_at, false)));
    if event.all_day {
        out.push_str(&format!("DTSTART;VALUE=DATE:{}\r\n", format_ical_time(event.starts_at, true)));
        out.push_str(&format!("DTEND;VALUE=DATE:{}\r\n", format_ical_time(event.ends_at.max(event.starts_at + 86_400), true)));
    } else {
        out.push_str(&format!("DTSTART:{}\r\n", format_ical_time(event.starts_at, false)));
        out.push_str(&format!("DTEND:{}\r\n", format_ical_time(event.ends_at, false)));
    }
    out.push_str(&fold(&format!("SUMMARY:{}", escape(&event.title))));
    if !event.notes.is_empty() {
        out.push_str(&fold(&format!("DESCRIPTION:{}", escape(&event.notes))));
    }
    if !event.location.is_empty() {
        out.push_str(&fold(&format!("LOCATION:{}", escape(&event.location))));
    }
    if let Some(rule) = &event.rrule {
        out.push_str(&format!("RRULE:{}\r\n", rule.trim_start_matches("RRULE:")));
    }
    out.push_str("END:VEVENT\r\n");
    out
}

fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut width = 0;
    for c in line.chars() {
        if width + c.len_utf8() > 74 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += c.len_utf8();
    }
    out.push_str("\r\n");
    out
}

fn wrap_calendar(body: &str) -> String {
    format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Vatra//Calendar//EN\r\n{body}END:VCALENDAR\r\n")
}

pub fn export_ics(store: &Store) -> Result<String, StoreError> {
    let events = store.calendar_events_in(i64::MIN / 2, i64::MAX / 2)?;
    Ok(wrap_calendar(&events.iter().map(event_ics).collect::<String>()))
}

// ------------------------------------------------------------------------------------------- CalDAV

fn xml_unescape(text: &str) -> String {
    let text = text.trim();
    text.strip_prefix("<![CDATA[").and_then(|t| t.strip_suffix("]]>")).unwrap_or(text).to_string()
}

fn entities(text: &str) -> String {
    text.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&#13;", "\r").replace("&amp;", "&")
}

/// The text of every element whose local name (without the namespace prefix) is `name`.
fn elements(xml: &str, name: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(open) = rest.find('<') {
        let tag_end = match rest[open..].find('>') {
            Some(i) => open + i,
            None => break,
        };
        let tag = &rest[open + 1..tag_end];
        let local = tag.split_whitespace().next().unwrap_or("").trim_end_matches('/');
        let local_name = local.rsplit(':').next().unwrap_or("");
        if local_name == name && !tag.starts_with('/') && !tag.ends_with('/') {
            let close = format!("</{local}>");
            if let Some(end) = rest[tag_end + 1..].find(&close) {
                found.push(rest[tag_end + 1..tag_end + 1 + end].to_string());
                rest = &rest[tag_end + 1 + end + close.len()..];
                continue;
            }
        }
        rest = &rest[tag_end + 1..];
    }
    found
}

fn element_text(xml: &str, name: &str) -> Option<String> {
    elements(xml, name).into_iter().next().map(|inner| {
        let inner = inner.trim();
        if inner.starts_with("<![CDATA[") { xml_unescape(inner) } else { entities(inner) }
    })
}

#[derive(Clone)]
pub struct Account {
    pub id: String,
    pub url: String,
    pub username: String,
    pub password: String,
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).redirect(reqwest::redirect::Policy::limited(3)).build().unwrap_or_default()
}

fn check_url(url: &str) -> Result<url::Url, String> {
    let parsed = url::Url::parse(url).map_err(|_| "The calendar address isn't valid.".to_string())?;
    match parsed.scheme() {
        "https" => Ok(parsed),
        // Plain http is for a server on this machine or the local network, never across the internet.
        "http" if parsed.host_str().is_some_and(is_local_host) => Ok(parsed),
        _ => Err("Use an https:// calendar address.".into()),
    }
}

fn is_local_host(host: &str) -> bool {
    host == "localhost"
        || host.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
            std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private(),
            std::net::IpAddr::V6(v6) => v6.is_loopback(),
        })
}

fn request(account: &Account, method: &str, url: &str) -> Result<reqwest::RequestBuilder, String> {
    check_url(url)?;
    let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
    let builder = http().request(method, url);
    Ok(match crate::oauth::access_token(&account.password) {
        Some(token) => builder.bearer_auth(token),
        None => builder.basic_auth(&account.username, Some(&account.password)),
    })
}

const REPORT_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop><d:getetag/><c:calendar-data/></d:prop>
  <c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"/></c:comp-filter></c:filter>
</c:calendar-query>"#;

/// Reads every event of the account's calendar.
pub async fn fetch(account: &Account) -> Result<Vec<NewCalendarEvent>, String> {
    let response = request(account, "REPORT", &account.url)?
        .header("Depth", "1")
        .header("Content-Type", "application/xml; charset=utf-8")
        .body(REPORT_BODY)
        .send()
        .await
        .map_err(|_| "The calendar server didn't answer.".to_string())?;
    if response.status().as_u16() == 401 || response.status().as_u16() == 403 {
        return Err("The calendar server refused the user name or password.".into());
    }
    if !response.status().is_success() {
        return Err(format!("The calendar server answered {}.", response.status().as_u16()));
    }
    if response.content_length().is_some_and(|n| n as usize > MAX_BODY) {
        return Err("The calendar is too large to read.".into());
    }
    let body = response.text().await.map_err(|_| "The calendar answer couldn't be read.".to_string())?;
    if body.len() > MAX_BODY {
        return Err("The calendar is too large to read.".into());
    }
    let base = url::Url::parse(&account.url).map_err(|e| e.to_string())?;
    let mut events = Vec::new();
    for item in elements(&body, "response") {
        let href = element_text(&item, "href");
        let etag = element_text(&item, "getetag");
        let Some(data) = element_text(&item, "calendar-data") else { continue };
        for mut event in parse_ics(&data) {
            event.source = "caldav".into();
            event.calendar = account.id.clone();
            event.etag = etag.clone();
            event.href = href.as_ref().and_then(|h| base.join(h).ok()).map(|u| u.to_string());
            if event.uid.is_empty() {
                continue;
            }
            events.push(event);
        }
    }
    Ok(events)
}

pub async fn put_event(account: &Account, event: &CalendarEvent) -> Result<(Option<String>, String), String> {
    let base = url::Url::parse(&account.url).map_err(|e| e.to_string())?;
    let target = match &event.href {
        Some(href) => href.clone(),
        None => {
            let mut dir = base.clone();
            if !dir.path().ends_with('/') {
                dir.set_path(&format!("{}/", dir.path()));
            }
            let name: String = event.uid.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.@".contains(c) { c } else { '_' }).collect();
            dir.join(&format!("{name}.ics")).map_err(|e| e.to_string())?.to_string()
        }
    };
    let mut builder = request(account, "PUT", &target)?.header("Content-Type", "text/calendar; charset=utf-8");
    builder = match (&event.href, &event.etag) {
        (Some(_), Some(etag)) => builder.header("If-Match", etag.clone()),
        (None, _) => builder.header("If-None-Match", "*"),
        _ => builder,
    };
    let response = builder
        .body(wrap_calendar(&event_ics(event)))
        .send()
        .await
        .map_err(|_| "The calendar server didn't answer.".to_string())?;
    if response.status().as_u16() == 412 {
        return Err("This event changed on the server. Refresh the calendar and try again.".into());
    }
    if !response.status().is_success() {
        return Err(format!("The calendar server answered {}.", response.status().as_u16()));
    }
    let etag = response.headers().get("etag").and_then(|v| v.to_str().ok()).map(str::to_string);
    Ok((etag, target))
}

pub async fn delete_event(account: &Account, href: &str, etag: Option<&str>) -> Result<(), String> {
    let mut builder = request(account, "DELETE", href)?;
    if let Some(etag) = etag {
        builder = builder.header("If-Match", etag);
    }
    let response = builder.send().await.map_err(|_| "The calendar server didn't answer.".to_string())?;
    if response.status().is_success() || response.status().as_u16() == 404 {
        Ok(())
    } else {
        Err(format!("The calendar server answered {}.", response.status().as_u16()))
    }
}

pub struct CalDavKind;

#[async_trait]
impl ConnectorKind for CalDavKind {
    fn kind(&self) -> &'static str {
        "caldav"
    }
    fn secret_fields(&self) -> &'static [&'static str] {
        &["password"]
    }
    async fn check(&self, settings: &Value, secrets: &BTreeMap<String, String>) -> Result<(), String> {
        let account = Account {
            id: "check".into(),
            url: settings.get("url").and_then(Value::as_str).unwrap_or_default().to_string(),
            username: settings.get("username").and_then(Value::as_str).unwrap_or_default().to_string(),
            password: secrets.get("password").cloned().unwrap_or_default(),
        };
        fetch(&account).await.map(|_| ())
    }
}

async fn account(service: &Service, id: &str) -> Result<Account, String> {
    let row = service
        .store()
        .connectors()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|row| row.id == id && row.kind == "caldav")
        .ok_or("This calendar account doesn't exist.")?;
    let settings: Value = serde_json::from_str(&row.settings_json).map_err(|e| e.to_string())?;
    let password = service
        .agents()
        .secrets()
        .get(&connector_secret(id, "password"))
        .map_err(|_| "The saved password couldn't be read.")?
        .unwrap_or_default();
    let password = crate::oauth::credential(service.store(), service.agents().secrets().as_ref(), id, &settings, password).await?;
    Ok(Account {
        id: row.id,
        url: settings.get("url").and_then(Value::as_str).unwrap_or_default().to_string(),
        username: settings.get("username").and_then(Value::as_str).unwrap_or_default().to_string(),
        password,
    })
}

/// Brings the account's events to the local calendar. Events that are gone on the server are removed here.
pub async fn sync(service: &Arc<Service>, id: &str) -> Result<usize, String> {
    let account = account(service, id).await?;
    let events = fetch(&account).await?;
    let now = crate::scheduler::now();
    let mut keep = Vec::new();
    for event in &events {
        service.store().save_calendar_event(event, now).map_err(|e| e.to_string())?;
        keep.push(event.uid.clone());
    }
    service.store().prune_calendar(id, &keep).map_err(|e| e.to_string())?;
    service.notify(Notification::CalendarChanged);
    Ok(events.len())
}

/// After a person changes an event of a CalDAV calendar, the change goes to the server.
pub fn push_if_remote(service: &Arc<Service>, event_id: i64) {
    let Ok(Some(event)) = service.store().calendar_event(event_id) else { return };
    if event.calendar == "local" || event.source == "ics" {
        return;
    }
    let service = Arc::clone(service);
    tokio::spawn(async move {
        let result = async {
            let account = account(&service, &event.calendar).await?;
            let (etag, href) = put_event(&account, &event).await?;
            let mut updated = event.clone();
            updated.etag = etag;
            updated.href = Some(href);
            service
                .store()
                .save_calendar_event(
                    &NewCalendarEvent {
                        calendar: updated.calendar,
                        uid: updated.uid,
                        title: updated.title,
                        notes: updated.notes,
                        location: updated.location,
                        starts_at: updated.starts_at,
                        ends_at: updated.ends_at,
                        all_day: updated.all_day,
                        rrule: updated.rrule,
                        source: "caldav".into(),
                        href: updated.href,
                        etag: updated.etag,
                    },
                    crate::scheduler::now(),
                )
                .map_err(|e| e.to_string())?;
            Ok::<(), String>(())
        }
        .await;
        if let Err(message) = result {
            service.notify(Notification::Problem { session_id: 0, message });
        }
    });
}

pub fn delete_remote(service: &Arc<Service>, event: CalendarEvent) {
    let (Some(href), true) = (event.href.clone(), event.calendar != "local" && event.source == "caldav") else { return };
    let service = Arc::clone(service);
    tokio::spawn(async move {
        let result = match account(&service, &event.calendar).await {
            Ok(account) => delete_event(&account, &href, event.etag.as_deref()).await,
            Err(message) => Err(message),
        };
        if let Err(message) = result {
            service.notify(Notification::Problem { session_id: 0, message });
        }
    });
}

type Reply = Result<Value, (i64, String)>;

pub(crate) async fn handle(service: &Arc<Service>, method: &str, params: Value) -> Reply {
    match method {
        "calendar.accounts" => Ok(json!(
            service
                .store()
                .connectors()
                .map_err(|e| (1000, e.to_string()))?
                .into_iter()
                .filter(|row| row.kind == "caldav")
                .map(|row| json!({"id": row.id, "name": row.name}))
                .collect::<Vec<_>>()
        )),
        "calendar.sync" => {
            let id = params.get("id").and_then(Value::as_str).ok_or((-32602, "an account id is needed".to_string()))?;
            let count = sync(service, id).await.map_err(|m| (1001, m))?;
            Ok(json!({"events": count}))
        }
        _ => Err((-32601, format!("unknown method `{method}`"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:abc-1\r\nDTSTART;TZID=Europe/Bucharest:20261107T090000\r\nDTEND;TZID=Europe/Bucharest:20261107T100000\r\nSUMMARY:Sedinta\\, echipa\r\nDESCRIPTION:Linia unu\\nLinia doi\r\n LUNGA\r\nLOCATION:Sala 2\r\nRRULE:FREQ=WEEKLY;BYDAY=SA\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:day-1\r\nDTSTART;VALUE=DATE:20261201\r\nSUMMARY:Libera\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:bad\r\nSUMMARY:fara data\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn reads_events_with_zones_escapes_folds_and_whole_days() {
        let events = parse_ics(SAMPLE);
        assert_eq!(events.len(), 2, "the event without a start is left out");
        let meeting = &events[0];
        assert_eq!(meeting.title, "Sedinta, echipa");
        assert_eq!(meeting.notes, "Linia unu\nLinia doiLUNGA");
        // 09:00 in Bucharest on 2026-11-07 is winter time (UTC+2): 07:00 UTC.
        assert_eq!(format_ical_time(meeting.starts_at, false), "20261107T070000Z");
        assert_eq!(meeting.ends_at - meeting.starts_at, 3600);
        assert_eq!(meeting.rrule.as_deref(), Some("FREQ=WEEKLY;BYDAY=SA"));
        let day = &events[1];
        assert!(day.all_day);
        assert_eq!(day.ends_at - day.starts_at, 86_400);
    }

    #[test]
    fn summer_time_moves_the_offset() {
        // 2026-07-01 09:00 in Bucharest is UTC+3.
        let (summer, _) = parse_ical_time("20260701T090000").unwrap();
        assert_eq!(zone_offset("Europe/Bucharest", summer), 10_800);
        let (winter, _) = parse_ical_time("20261107T090000").unwrap();
        assert_eq!(zone_offset("Europe/Bucharest", winter), 7200);
        // Just before and after the change on the last Sunday of March 2026 (the 29th).
        let (before, _) = parse_ical_time("20260329T020000").unwrap();
        let (after, _) = parse_ical_time("20260329T040000").unwrap();
        assert_eq!(zone_offset("Europe/Bucharest", before), 7200);
        assert_eq!(zone_offset("Europe/Bucharest", after), 10_800);
    }

    #[test]
    fn export_then_import_keeps_what_matters() {
        let store = Store::open_in_memory().unwrap();
        for event in parse_ics(SAMPLE) {
            store.save_calendar_event(&event, 5).unwrap();
        }
        let text = export_ics(&store).unwrap();
        let again = parse_ics(&text);
        assert_eq!(again.len(), 2);
        let meeting = again.iter().find(|e| e.uid == "abc-1").unwrap();
        assert_eq!(meeting.title, "Sedinta, echipa");
        assert_eq!(meeting.notes, "Linia unu\nLinia doiLUNGA");
        assert_eq!(meeting.rrule.as_deref(), Some("FREQ=WEEKLY;BYDAY=SA"));
        assert!(again.iter().find(|e| e.uid == "day-1").unwrap().all_day);
    }

    #[test]
    fn long_lines_are_folded_and_read_back() {
        let long = "x".repeat(300);
        let folded = fold(&format!("SUMMARY:{long}"));
        assert!(folded.split("\r\n").all(|l| l.len() <= 75));
        let events = parse_ics(&format!("BEGIN:VEVENT\r\nUID:u\r\nDTSTART:20260101T000000Z\r\n{folded}END:VEVENT\r\n"));
        assert_eq!(events[0].title, long);
    }

    #[test]
    fn multistatus_is_read_without_caring_about_prefixes() {
        let xml = "<D:multistatus xmlns:D=\"DAV:\"><D:response><D:href>/cal/a.ics</D:href><D:propstat><D:prop><D:getetag>\"e1\"</D:getetag><C:calendar-data xmlns:C=\"urn:ietf:params:xml:ns:caldav\">BEGIN:VEVENT\nUID:q&amp;a\nEND:VEVENT</C:calendar-data></D:prop></D:propstat></D:response></D:multistatus>";
        let responses = elements(xml, "response");
        assert_eq!(responses.len(), 1);
        assert_eq!(element_text(&responses[0], "href").as_deref(), Some("/cal/a.ics"));
        assert_eq!(element_text(&responses[0], "getetag").as_deref(), Some("\"e1\""));
        assert!(element_text(&responses[0], "calendar-data").unwrap().contains("UID:q&a"));
    }

    #[test]
    fn plain_http_is_for_local_servers_only() {
        assert!(check_url("https://cal.example.com/dav").is_ok());
        assert!(check_url("http://localhost:8080/x").is_ok());
        assert!(check_url("http://192.168.1.4/x").is_ok());
        assert!(check_url("http://cal.example.com/x").is_err());
        assert!(check_url("ftp://x").is_err());
    }

    #[test]
    fn durations_are_read() {
        assert_eq!(duration_secs("PT1H30M"), Some(5400));
        assert_eq!(duration_secs("P1DT2H"), Some(93_600));
        assert_eq!(duration_secs("-PT15M"), Some(-900));
        assert_eq!(duration_secs("garbage"), None);
    }
}
