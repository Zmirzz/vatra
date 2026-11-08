import {ChevronLeft, ChevronRight, Download, Plus, RefreshCw, Upload} from "lucide-react";
import {useCallback, useEffect, useMemo, useRef, useState, type FormEvent} from "react";
import {useHubCopy} from "../../hub/copy";
import {DAY, formatDay, formatTime, fromInput, monthGrid, startOfLocalDay, toInputDate, toInputTime} from "../../hub/time";
import type {Account, CalendarEvent, Occurrence} from "../../hub/types";
import {useRefresh} from "../../hub/useRefresh";
import {useClient} from "../../rpc/context";
import {usePreferences} from "../../settings/preferences";

const RULES: Record<string, string> = {daily: "FREQ=DAILY", weekly: "FREQ=WEEKLY", monthly: "FREQ=MONTHLY", yearly: "FREQ=YEARLY"};

export function CalendarPage() {
  const t = useHubCopy();
  const rpc = useClient();
  const language = usePreferences((s) => s.preferences.language);
  const now = new Date();
  const [cursor, setCursor] = useState({year: now.getFullYear(), month: now.getMonth()});
  const [selected, setSelected] = useState(startOfLocalDay(Math.floor(Date.now() / 1000)));
  const [occurrences, setOccurrences] = useState<Occurrence[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [editing, setEditing] = useState<Partial<CalendarEvent> | null>(null);
  const [syncing, setSyncing] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);

  const cells = useMemo(() => monthGrid(cursor.year, cursor.month), [cursor]);
  const load = useCallback(async () => {
    try {
      setOccurrences(await rpc.call<Occurrence[]>("calendar.list", {from: cells[0], to: cells[41] + DAY}));
      setError(null);
    } catch { setError(t.loadError); }
  }, [rpc, cells, t]);
  useEffect(() => { void load(); }, [load]);
  useEffect(() => { void rpc.call<Account[]>("calendar.accounts", {}).then(setAccounts).catch(() => undefined); }, [rpc]);
  useRefresh(rpc, ["calendar_changed"], () => void load());

  const byDay = useMemo(() => {
    const map = new Map<number, Occurrence[]>();
    for (const o of occurrences) {
      for (let day = startOfLocalDay(o.starts_at); day <= startOfLocalDay(Math.max(o.starts_at, o.ends_at - (o.all_day ? 1 : 0))); day += DAY) {
        if (day < cells[0] || day > cells[41]) continue;
        map.set(day, [...(map.get(day) ?? []), o]);
      }
    }
    return map;
  }, [occurrences, cells]);
  const todays = byDay.get(selected) ?? [];
  const monthLabel = new Date(cursor.year, cursor.month, 1).toLocaleDateString(language === "ro" ? "ro-RO" : "en-GB", {month: "long", year: "numeric"});
  const move = (delta: number) => setCursor(({year, month}) => { const d = new Date(year, month + delta, 1); return {year: d.getFullYear(), month: d.getMonth()}; });

  const edit = async (occurrence: Occurrence) => {
    try { setEditing(await rpc.call<CalendarEvent>("calendar.get", {id: occurrence.event_id})); } catch { setError(t.loadError); }
  };
  const sync = async () => {
    setSyncing(true); setNotice(t.syncing);
    try {
      let total = 0;
      for (const account of accounts) total += (await rpc.call<{events: number}>("calendar.sync", {id: account.id})).events;
      setNotice(t.synced(total)); setError(null);
    } catch (e) { setNotice(null); setError(e instanceof Error ? e.message : t.saveError); } finally { setSyncing(false); }
  };
  const exportIcs = async () => {
    try {
      const {text} = await rpc.call<{text: string}>("calendar.export_ics", {});
      const url = URL.createObjectURL(new Blob([text], {type: "text/calendar"}));
      const link = document.createElement("a");
      link.href = url; link.download = "vatra-calendar.ics"; link.click();
      URL.revokeObjectURL(url);
    } catch { setError(t.saveError); }
  };
  const importIcs = async (file: File | undefined) => {
    if (!file) return;
    try { const result = await rpc.call<{imported: number}>("calendar.import_ics", {text: await file.text()}); setNotice(t.importDone(result.imported)); setError(null); } catch (e) { setError(e instanceof Error ? e.message : t.saveError); }
  };

  return (
    <div className="hub-page hub-calendar">
      <header className="hub-head">
        <h1>{t.calendarTitle}</h1>
        <div className="hub-actions">
          {accounts.length ? <button type="button" onClick={() => void sync()} disabled={syncing}><RefreshCw size={14} aria-hidden="true" />{t.sync}</button> : null}
          <button type="button" onClick={() => fileInput.current?.click()}><Upload size={14} aria-hidden="true" />{t.importIcs}</button>
          <input ref={fileInput} type="file" accept=".ics,text/calendar" hidden aria-label={t.importIcs} onChange={(e) => { void importIcs(e.target.files?.[0]); e.target.value = ""; }} />
          <button type="button" onClick={() => void exportIcs()}><Download size={14} aria-hidden="true" />{t.exportIcs}</button>
          <button type="button" className="primary" onClick={() => setEditing({starts_at: selected + 9 * 3600, ends_at: selected + 10 * 3600, calendar: "local"})}><Plus size={15} aria-hidden="true" />{t.newEvent}</button>
        </div>
      </header>
      {error ? <p role="alert" className="hub-error">{error}</p> : null}
      {notice ? <p role="status" className="hub-note">{notice}</p> : null}
      {editing ? <EventForm initial={editing} accounts={accounts} onCancel={() => setEditing(null)} onSaved={() => { setEditing(null); void load(); }} /> : null}
      <div className="calendar-layout">
        <section aria-label={monthLabel}>
          <div className="calendar-bar">
            <button type="button" className="icon" aria-label={t.previous} onClick={() => move(-1)}><ChevronLeft size={16} aria-hidden="true" /></button>
            <h2>{monthLabel}</h2>
            <button type="button" className="icon" aria-label={t.next} onClick={() => move(1)}><ChevronRight size={16} aria-hidden="true" /></button>
            <button type="button" onClick={() => { const n = new Date(); setCursor({year: n.getFullYear(), month: n.getMonth()}); setSelected(startOfLocalDay(Math.floor(Date.now() / 1000))); }}>{t.today}</button>
          </div>
          <div className="calendar-grid" role="grid" aria-label={monthLabel}>
            {[0, 1, 2, 3, 4, 5, 6].map((i) => <div key={i} className="calendar-dow" role="columnheader">{t.weekdays[(i + 1) % 7]}</div>)}
            {cells.map((day) => {
              const d = new Date(day * 1000);
              const list = byDay.get(day) ?? [];
              return (
                <button key={day} type="button" role="gridcell" className="calendar-day" data-outside={d.getMonth() !== cursor.month} data-selected={day === selected}
                  data-today={day === startOfLocalDay(Math.floor(Date.now() / 1000))} aria-label={`${formatDay(day, language)}${list.length ? `, ${list.length}` : ""}`} aria-selected={day === selected} onClick={() => setSelected(day)}>
                  <span>{d.getDate()}</span>
                  {list.slice(0, 3).map((o) => <i key={`${o.event_id}-${o.starts_at}`} className="calendar-chip">{o.title}</i>)}
                  {list.length > 3 ? <small>+{list.length - 3}</small> : null}
                </button>
              );
            })}
          </div>
        </section>
        <aside aria-label={formatDay(selected, language)} className="calendar-day-list">
          <h2>{formatDay(selected, language)}</h2>
          {todays.length === 0 ? <p className="hub-meta">{t.noEvents}</p> : null}
          <ul className="hub-list">
            {todays.map((o) => (
              <li key={`${o.event_id}-${o.starts_at}`} className="hub-card">
                <button type="button" className="hub-card-main hub-plain" onClick={() => void edit(o)}>
                  <b>{o.title}</b>
                  <span className="hub-meta">{o.all_day ? t.allDay : `${formatTime(o.starts_at, language)} – ${formatTime(o.ends_at, language)}`}{o.recurring ? ` · ${t.recurring}` : ""}{o.location ? ` · ${o.location}` : ""}</span>
                </button>
              </li>
            ))}
          </ul>
        </aside>
      </div>
    </div>
  );
}

function EventForm({initial, accounts, onCancel, onSaved}: {initial: Partial<CalendarEvent>; accounts: Account[]; onCancel: () => void; onSaved: () => void}) {
  const t = useHubCopy();
  const rpc = useClient();
  const start = initial.starts_at ?? Math.floor(Date.now() / 1000);
  const end = initial.ends_at ?? start + 3600;
  const [title, setTitle] = useState(initial.title ?? "");
  const [allDay, setAllDay] = useState(initial.all_day ?? false);
  const [startDate, setStartDate] = useState(toInputDate(start));
  const [startTime, setStartTime] = useState(toInputTime(start));
  const [endDate, setEndDate] = useState(toInputDate(allDay ? end - DAY : end));
  const [endTime, setEndTime] = useState(toInputTime(end));
  const [place, setPlace] = useState(initial.location ?? "");
  const [notes, setNotes] = useState(initial.notes ?? "");
  const [repeat, setRepeat] = useState(initial.rrule ? (Object.entries(RULES).find(([, rule]) => initial.rrule?.startsWith(rule))?.[0] ?? "none") : "none");
  const [calendar, setCalendar] = useState(initial.calendar ?? "local");
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const isNew = initial.id === undefined;

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const starts = allDay ? fromInput(startDate, "00:00") : fromInput(startDate, startTime);
    const ends = allDay ? fromInput(endDate, "00:00") + DAY : fromInput(endDate, endTime);
    if (!title.trim() || !Number.isFinite(starts) || !Number.isFinite(ends) || ends < starts) { setProblem(t.saveError); return; }
    setBusy(true);
    try {
      await rpc.call("calendar.save", {
        id: initial.id, title: title.trim(), location: place, notes, starts_at: starts, ends_at: ends, all_day: allDay,
        rrule: repeat === "none" ? null : repeat === "keep" ? initial.rrule : RULES[repeat],
        ...(isNew ? {calendar, source: calendar === "local" ? "local" : "caldav"} : {}),
      });
      onSaved();
    } catch (e) { setProblem(e instanceof Error ? e.message : t.saveError); } finally { setBusy(false); }
  };
  const remove = async () => {
    if (initial.id === undefined) return;
    try { await rpc.call("calendar.delete", {id: initial.id}); onSaved(); } catch { setProblem(t.saveError); }
  };

  return (
    <form className="hub-form" onSubmit={(e) => void submit(e)} aria-label={isNew ? t.newEvent : t.edit}>
      <label>{t.eventTitle}<input value={title} maxLength={200} onChange={(e) => setTitle(e.target.value)} required /></label>
      <label className="hub-check"><input type="checkbox" checked={allDay} onChange={(e) => setAllDay(e.target.checked)} />{t.allDay}</label>
      <div className="hub-row">
        <label>{t.eventStart}<input type="date" value={startDate} onChange={(e) => setStartDate(e.target.value)} /></label>
        {allDay ? null : <label>{t.time}<input type="time" value={startTime} onChange={(e) => setStartTime(e.target.value)} /></label>}
      </div>
      <div className="hub-row">
        <label>{t.eventEnd}<input type="date" value={endDate} onChange={(e) => setEndDate(e.target.value)} /></label>
        {allDay ? null : <label>{t.time}<input type="time" value={endTime} onChange={(e) => setEndTime(e.target.value)} /></label>}
      </div>
      <label>{t.eventPlace}<input value={place} maxLength={200} onChange={(e) => setPlace(e.target.value)} /></label>
      <label>{t.eventNotes}<textarea rows={3} value={notes} onChange={(e) => setNotes(e.target.value)} /></label>
      <div className="hub-row">
        <label>{t.eventRepeat}
          <select value={repeat} onChange={(e) => setRepeat(e.target.value)}>
            <option value="none">{t.repeatNone}</option><option value="daily">{t.repeatDaily}</option><option value="weekly">{t.repeatWeekly}</option>
            <option value="monthly">{t.repeatMonthly}</option><option value="yearly">{t.repeatYearly}</option>
            {initial.rrule && !Object.values(RULES).some((r) => initial.rrule?.startsWith(r)) ? <option value="keep">{initial.rrule}</option> : null}
          </select>
        </label>
        {isNew && accounts.length ? (
          <label>{t.calendarWhere}
            <select value={calendar} onChange={(e) => setCalendar(e.target.value)}>
              <option value="local">{t.localCalendar}</option>{accounts.map((a) => <option key={a.id} value={a.id}>{a.name}</option>)}
            </select>
          </label>
        ) : null}
      </div>
      {problem ? <p role="alert" className="hub-error">{problem}</p> : null}
      <div className="hub-actions">
        {!isNew ? <button type="button" onClick={() => void remove()}>{t.delete}</button> : null}
        <button type="button" onClick={onCancel}>{t.cancel}</button>
        <button type="submit" className="primary" disabled={busy}>{t.save}</button>
      </div>
    </form>
  );
}
