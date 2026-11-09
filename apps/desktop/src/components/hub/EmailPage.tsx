import {Bell, Inbox as InboxIcon, Mail, MessagesSquare, PenLine, Search, Sparkles, Star, Tag, Tags, Users, Info} from "lucide-react";
import {useCallback, useEffect, useState, type FormEvent} from "react";
import {useHubCopy} from "../../hub/copy";
import {initial, senderAddress, senderName, shortDate} from "../../hub/emailHtml";
import {usePreferences} from "../../settings/preferences";
import {Attachments, MailBody} from "./MailBody";
import {fromInput, toInputDate, toInputTime} from "../../hub/time";
import type {Account, Draft, MailHeader, MailMessage} from "../../hub/types";
import {RpcError} from "../../rpc/client";
import {useClient} from "../../rpc/context";
import {useAppStore} from "../../state/store";

type View = "inbox" | "drafts";
const CATEGORIES = ["all", "primary", "promotions", "social", "updates", "forums"] as const;
type Category = (typeof CATEGORIES)[number];
const CATEGORY_ICON = {all: InboxIcon, primary: Mail, promotions: Tag, social: Users, updates: Info, forums: MessagesSquare};
const categoryOf = (m: {category?: string}): Exclude<Category, "all"> => (CATEGORIES.includes(m.category as Category) && m.category !== "all" ? (m.category as Exclude<Category, "all">) : "primary");
const FIXED_FOLDER = "INBOX";

export function EmailPage({onOpenSettings}: {onOpenSettings: () => void}) {
  const t = useHubCopy();
  const rpc = useClient();
  const [accounts, setAccounts] = useState<Account[] | null>(null);
  const [account, setAccount] = useState("");
  const [view, setView] = useState<View>("inbox");

  useEffect(() => {
    void rpc.call<Account[]>("email.accounts", {}).then((list) => { setAccounts(list); setAccount((current) => current || list[0]?.id || ""); }).catch(() => setAccounts([]));
  }, [rpc]);

  if (accounts === null) return <div className="hub-page"><p role="status">{t.loading}</p></div>;
  if (accounts.length === 0) {
    return (
      <div className="hub-page">
        <div className="hub-empty"><Mail size={28} aria-hidden="true" /><h2>{t.emailTitle}</h2><p>{t.needAccount}</p><button type="button" className="primary" onClick={onOpenSettings}>{t.openSettings}</button></div>
      </div>
    );
  }
  return (
    <div className="hub-page hub-email">
      <header className="hub-head">
        <h1>{t.emailTitle}</h1>
        <div className="hub-actions">
          {accounts.length > 1 ? <select aria-label={t.account} value={account} onChange={(e) => setAccount(e.target.value)}>{accounts.map((a) => <option key={a.id} value={a.id}>{a.name}</option>)}</select> : null}
          <div className="hub-tabs" role="tablist">
            <button type="button" role="tab" aria-selected={view === "inbox"} onClick={() => setView("inbox")}>{t.inbox}</button>
            <button type="button" role="tab" aria-selected={view === "drafts"} onClick={() => setView("drafts")}>{t.drafts}</button>
          </div>
        </div>
      </header>
      {view === "inbox" ? <Inbox account={account} onDraft={() => setView("drafts")} /> : <Drafts account={account} />}
    </div>
  );
}

function errorText(error: unknown, t: ReturnType<typeof useHubCopy>): string {
  if (error instanceof RpcError) {
    if (error.code === 1102) return t.changedSince;
    if (error.code === 1103) return t.hourlyLimit;
    return error.message;
  }
  return t.loadError;
}

function Inbox({account, onDraft}: {account: string; onDraft: () => void}) {
  const t = useHubCopy();
  const rpc = useClient();
  const [messages, setMessages] = useState<MailHeader[] | null>(null);
  const [more, setMore] = useState<number | null>(null);
  const [open, setOpen] = useState<MailMessage | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [instructions, setInstructions] = useState("");
  const [labelDraft, setLabelDraft] = useState("");
  const [reminding, setReminding] = useState(false);
  const [category, setCategory] = useState<Category>("all");
  const [query, setQuery] = useState("");
  const [unreadOnly, setUnreadOnly] = useState(false);
  const language = usePreferences((state) => state.preferences.language);

  const load = useCallback(async (before?: number) => {
    try {
      const page = await rpc.call<{messages: MailHeader[]; has_more: boolean; next_before: number | null}>("email.list", {account, folder: FIXED_FOLDER, limit: 30, before_uid: before});
      setMessages((old) => (before && old ? [...old, ...page.messages] : page.messages));
      setMore(page.has_more ? page.next_before : null);
      setError(null);
    } catch (e) { setError(errorText(e, t)); setMessages((old) => old ?? []); }
  }, [rpc, account, t]);
  useEffect(() => { setMessages(null); setOpen(null); void load(); }, [load]);

  const patch = (key: string, change: Partial<MailHeader>) => setMessages((old) => old?.map((m) => (m.key === key ? {...m, ...change} : m)) ?? old);
  const read = async (header: MailHeader) => {
    setBusy("read"); setError(null);
    try {
      setOpen(await rpc.call<MailMessage>("email.read", {account, folder: FIXED_FOLDER, uid: header.uid}));
      // Opening a message reads it, as in any mail app.
      if (!header.seen) {
        patch(header.key, {seen: true});
        void rpc.call("email.mark", {account, folder: FIXED_FOLDER, uid: header.uid, seen: true}).catch(() => patch(header.key, {seen: false}));
      }
    } catch (e) { setError(errorText(e, t)); } finally { setBusy(null); }
  };
  const run = async (name: string, work: () => Promise<void>) => {
    setBusy(name); setError(null);
    try { await work(); } catch (e) { setError(errorText(e, t)); } finally { setBusy(null); }
  };
  const ref = (uid: number) => ({account, folder: FIXED_FOLDER, uid});

  const summarize = () => open && run("summary", async () => {
    const result = await rpc.call<{summary: string}>("email.summarize", ref(open.uid));
    setOpen({...open, summary: result.summary}); patch(open.key, {summary: result.summary});
  });
  const sortAll = () => messages && run("triage", async () => {
    const uids = messages.slice(0, 30).map((m) => m.uid);
    const result = await rpc.call<{results: {uid: number; triage: string}[]}>("email.triage", {account, folder: FIXED_FOLDER, uids});
    setMessages((old) => old?.map((m) => ({...m, triage: result.results.find((r) => r.uid === m.uid)?.triage ?? m.triage})) ?? old);
  });
  const draft = () => open && run("draft", async () => {
    await rpc.call("email.draft_reply", {...ref(open.uid), instructions});
    setInstructions(""); onDraft();
  });
  const mark = (change: {seen?: boolean; flagged?: boolean}) => open && run("mark", async () => {
    await rpc.call("email.mark", {...ref(open.uid), ...change});
    patch(open.key, {...(change.seen !== undefined ? {seen: change.seen} : {}), ...(change.flagged !== undefined ? {flagged: change.flagged} : {})});
  });
  const addLabel = (event: FormEvent) => {
    event.preventDefault();
    if (!open || !labelDraft.trim()) return;
    const labels = [...new Set([...open.labels, labelDraft.trim()])];
    void run("label", async () => {
      await rpc.call("email.label", {account, key: open.key, labels});
      setOpen({...open, labels}); patch(open.key, {labels}); setLabelDraft("");
    });
  };

  const counts = Object.fromEntries(CATEGORIES.map((c) => [c, (messages ?? []).filter((m) => !m.seen && (c === "all" || categoryOf(m) === c)).length])) as Record<Category, number>;
  const needle = query.trim().toLowerCase();
  const shown = (messages ?? []).filter((m) => (category === "all" || categoryOf(m) === category) && (!unreadOnly || !m.seen) && (!needle || `${m.from} ${m.subject}`.toLowerCase().includes(needle)));
  const current = open ? messages?.find((m) => m.key === open.key) : undefined;

  return (
    <div className="email-layout">
      <section className="email-list" aria-label={t.inbox}>
        <div className="email-list-head">
          <div className="email-cats" role="tablist" aria-label={t.inbox}>
            {CATEGORIES.map((c) => {
              const Icon = CATEGORY_ICON[c];
              return (
                <button key={c} type="button" role="tab" data-category={c} aria-selected={category === c} onClick={() => setCategory(c)}>
                  <Icon size={14} aria-hidden="true" />{t.categories[c]}
                  {counts[c] > 0 ? <span className="count" aria-label={t.unreadCount(counts[c])}>{counts[c]}</span> : null}
                </button>
              );
            })}
          </div>
          <div className="email-find">
            <label><Search size={14} aria-hidden="true" /><input type="search" value={query} placeholder={t.searchMail} aria-label={t.searchMail} onChange={(e) => setQuery(e.target.value)} /></label>
            <button type="button" aria-pressed={unreadOnly} onClick={() => setUnreadOnly((v) => !v)}>{t.unreadOnly}</button>
            <button type="button" onClick={() => void sortAll()} disabled={busy !== null || !messages?.length}><Sparkles size={14} aria-hidden="true" />{busy === "triage" ? t.working : t.sortMail}</button>
          </div>
        </div>
        {error ? <p role="alert" className="hub-error">{error}</p> : null}
        {messages === null ? <p role="status" className="email-empty">{t.loading}</p> : null}
        {messages && messages.length === 0 && !error ? <p className="email-empty">{t.noMessages}</p> : null}
        {messages && messages.length > 0 && shown.length === 0 ? <p className="email-empty">{needle || unreadOnly ? t.noMatches : t.nothingHere}</p> : null}
        <ul>
          {shown.map((m) => (
            <li key={m.key}>
              <button type="button" className="email-row" data-unread={!m.seen} data-category={categoryOf(m)} aria-current={open?.key === m.key ? "true" : undefined} onClick={() => void read(m)}>
                <span className="email-avatar" aria-hidden="true">{initial(m.from)}</span>
                <span className="email-top">
                  <span className="email-from">{m.flagged ? <Star size={12} aria-hidden="true" /> : null}{senderName(m.from)}</span>
                  <span className="email-when">{shortDate(m.date, language)}</span>
                </span>
                <span className="email-subject">{m.subject}</span>
                <span className="email-tags">
                  {m.triage ? <i className="hub-badge" data-triage={m.triage}>{t.triage[m.triage] ?? m.triage}</i> : null}
                  {m.labels.map((l) => <i key={l} className="hub-badge">{l}</i>)}
                </span>
              </button>
            </li>
          ))}
        </ul>
        {more !== null ? <button type="button" onClick={() => void load(more)}>{t.loadMore}</button> : null}
      </section>
      <section className="email-reader" aria-label={open?.subject ?? t.selectMessage}>
        {!open ? <p className="hub-meta">{busy === "read" ? t.loading : t.selectMessage}</p> : (
          <>
            <h2>{open.subject}</h2>
            <div className="email-sender" data-category={categoryOf(open)}>
              <span className="email-avatar big" aria-hidden="true">{initial(open.from)}</span>
              <div className="email-sender-text"><b>{senderName(open.from)}</b><span>{senderAddress(open.from)}</span></div>
            </div>
            <dl className="email-meta"><dt>{t.from}</dt><dd>{open.from}</dd><dt>{t.to}</dt><dd>{open.to}</dd><dt>{t.date2}</dt><dd>{open.date}</dd></dl>
            <p className="hub-note email-warning">{t.untrustedNote}</p>
            {open.html ? <MailBody html={open.html} /> : <pre className="email-body" aria-label={t.emailTitle}>{open.text}</pre>}
            <Attachments files={open.attachments ?? []} />
            {open.summary ? <div className="email-summary"><b>{t.summary}</b><p>{open.summary}</p></div> : null}
            <div className="hub-actions">
              <button type="button" onClick={() => void summarize()} disabled={busy !== null}><Sparkles size={14} aria-hidden="true" />{busy === "summary" ? t.working : t.summarize}</button>
              <button type="button" onClick={() => void mark({seen: true})} disabled={busy !== null}>{t.markSeen}</button>
              <button type="button" onClick={() => void mark({flagged: !current?.flagged})} disabled={busy !== null}><Star size={14} aria-hidden="true" />{current?.flagged ? t.unflag : t.flag}</button>
              <button type="button" onClick={() => setReminding((v) => !v)}><Bell size={14} aria-hidden="true" />{t.remindMe}</button>
            </div>
            {reminding ? <Reminder account={account} uid={open.uid} onDone={() => setReminding(false)} /> : null}
            <form className="hub-row email-labels" onSubmit={addLabel}>
              <label><Tags size={14} aria-hidden="true" /> {t.labels}<input value={labelDraft} maxLength={32} placeholder={t.addLabel} onChange={(e) => setLabelDraft(e.target.value)} /></label>
              <span>{open.labels.map((l) => <i key={l} className="hub-badge">{l}</i>)}</span>
            </form>
            <div className="email-draft-box">
              <label>{t.instructions}<textarea rows={2} value={instructions} onChange={(e) => setInstructions(e.target.value)} /></label>
              <button type="button" className="primary" onClick={() => void draft()} disabled={busy !== null}><PenLine size={14} aria-hidden="true" />{busy === "draft" ? t.working : t.draftReply}</button>
            </div>
          </>
        )}
      </section>
    </div>
  );
}

function Reminder({account, uid, onDone}: {account: string; uid: number; onDone: () => void}) {
  const t = useHubCopy();
  const rpc = useClient();
  const projects = useAppStore((s) => s.projects);
  const tomorrow = Math.floor(Date.now() / 1000) + 86_400;
  const [project, setProject] = useState(String(projects[0]?.id ?? ""));
  const [date, setDate] = useState(toInputDate(tomorrow));
  const [time, setTime] = useState(toInputTime(tomorrow));
  const [message, setMessage] = useState<string | null>(null);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const at = fromInput(date, time);
    if (!project || !Number.isFinite(at)) { setMessage(t.saveError); return; }
    try { await rpc.call("email.remind", {account, folder: FIXED_FOLDER, uid, project_id: Number(project), at}); setMessage(t.reminderSet); setTimeout(onDone, 900); } catch (e) { setMessage(errorText(e, t)); }
  };
  return (
    <form className="hub-form hub-inline" onSubmit={(e) => void submit(e)} aria-label={t.remindAt}>
      <label>{t.project}<select value={project} onChange={(e) => setProject(e.target.value)}>{projects.length === 0 ? <option value="">{t.chooseProject}</option> : null}{projects.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}</select></label>
      <label>{t.date}<input type="date" value={date} onChange={(e) => setDate(e.target.value)} /></label>
      <label>{t.time}<input type="time" value={time} onChange={(e) => setTime(e.target.value)} /></label>
      <button type="submit" className="primary">{t.remindMe}</button>
      {message ? <p role="status">{message}</p> : null}
    </form>
  );
}

function Drafts({account}: {account: string}) {
  const t = useHubCopy();
  const rpc = useClient();
  const [drafts, setDrafts] = useState<Draft[] | null>(null);
  const [editing, setEditing] = useState<Draft | "new" | null>(null);
  const [confirming, setConfirming] = useState<Draft | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try { setDrafts(await rpc.call<Draft[]>("email.drafts", {account})); setError(null); } catch (e) { setError(errorText(e, t)); setDrafts([]); }
  }, [rpc, account, t]);
  useEffect(() => { void load(); }, [load]);

  const send = async (draft: Draft) => {
    try {
      await rpc.call("email.send", {id: draft.id, fingerprint: draft.fingerprint});
      setConfirming(null); await load(); setError(null);
    } catch (e) {
      setConfirming(null); await load(); setError(errorText(e, t));
    }
  };
  const placeholder = (d: Draft) => d.to.some((a) => a.endsWith("@example.invalid"));

  return (
    <div className="email-drafts">
      <div className="hub-actions"><button type="button" className="primary" onClick={() => setEditing("new")}><PenLine size={14} aria-hidden="true" />{t.newDraft}</button></div>
      {error ? <p role="alert" className="hub-error">{error}</p> : null}
      {editing ? <DraftForm key={editing === "new" ? "new" : editing.id} account={account} initial={editing === "new" ? null : editing} onCancel={() => setEditing(null)} onSaved={() => { setEditing(null); void load(); }} /> : null}
      {drafts === null ? <p role="status">{t.loading}</p> : null}
      {drafts && drafts.length === 0 ? <p className="hub-meta">{t.noDrafts}</p> : null}
      <ul className="hub-list">
        {drafts?.map((d) => (
          <li key={d.id} className="hub-card" data-state={d.state}>
            <div className="hub-card-main">
              <b>{d.subject || "—"} <span className="hub-badge" data-state={d.state}>{d.state === "sent" ? t.sent : d.state === "failed" ? t.failed : d.state === "sending" ? t.working : t.drafts}</span></b>
              <span className="hub-meta">{d.to.join(", ")}</span>
              <p className="hub-summary">{d.body}</p>
              {d.error ? <p role="alert" className="hub-error">{d.error}</p> : null}
              {placeholder(d) ? <p className="hub-error">{t.placeholderRecipient}</p> : null}
            </div>
            {d.state === "draft" || d.state === "failed" ? (
              <div className="hub-actions">
                <button type="button" onClick={() => setEditing(d)}>{t.edit}</button>
                <button type="button" onClick={() => void rpc.call("email.draft_delete", {id: d.id}).then(load)}>{t.delete}</button>
                <button type="button" className="primary" disabled={placeholder(d)} onClick={() => setConfirming(d)}>{t.sendDraft}</button>
              </div>
            ) : null}
          </li>
        ))}
      </ul>
      {confirming ? (
        <div className="hub-dialog" role="dialog" aria-modal="true" aria-label={t.confirmSendTitle}>
          <div className="hub-dialog-box">
            <h2>{t.confirmSendTitle}</h2>
            <p>{t.confirmSendBody(confirming.to.length)}</p>
            <dl className="email-meta"><dt>{t.to}</dt><dd>{confirming.to.join(", ")}</dd><dt>{t.subject}</dt><dd>{confirming.subject}</dd></dl>
            <pre className="email-body">{confirming.body}</pre>
            <div className="hub-actions"><button type="button" onClick={() => setConfirming(null)}>{t.cancel}</button><button type="button" className="primary" onClick={() => void send(confirming)}>{t.send}</button></div>
          </div>
        </div>
      ) : null}
    </div>
  );
}

function DraftForm({account, initial, onCancel, onSaved}: {account: string; initial: Draft | null; onCancel: () => void; onSaved: () => void}) {
  const t = useHubCopy();
  const rpc = useClient();
  const [to, setTo] = useState(initial?.to.join(", ") ?? "");
  const [subject, setSubject] = useState(initial?.subject ?? "");
  const [body, setBody] = useState(initial?.body ?? "");
  const [problem, setProblem] = useState<string | null>(null);
  const [suggestions, setSuggestions] = useState<{address: string; name: string}[]>([]);
  // Completes the address being typed (the part after the last comma) from the people met in this mailbox.
  useEffect(() => {
    const part = to.split(",").at(-1)?.trim() ?? "";
    if (part.length < 2) { setSuggestions([]); return; }
    const timer = setTimeout(() => {
      void rpc.call<{address: string; name: string}[]>("email.contacts", {account, query: part}).then(setSuggestions).catch(() => setSuggestions([]));
    }, 200);
    return () => clearTimeout(timer);
  }, [to, account, rpc]);
  const pick = (address: string) => setTo((old) => `${old.split(",").slice(0, -1).map((a) => a.trim()).filter(Boolean).concat(address).join(", ")}, `);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const list = to.split(",").map((a) => a.trim()).filter(Boolean);
    try {
      if (initial) await rpc.call("email.draft_save", {id: initial.id, to: list, subject, body});
      else await rpc.call("email.draft_create", {account, to: list, subject, body});
      onSaved();
    } catch (e) { setProblem(errorText(e, t)); }
  };
  return (
    <form className="hub-form" onSubmit={(e) => void submit(e)} aria-label={t.draftEdit}>
      <label>{t.recipients}<input value={to} onChange={(e) => setTo(e.target.value)} required /></label>
      {suggestions.length ? (
        <ul className="email-suggest" aria-label={t.recipients}>
          {suggestions.map((s) => <li key={s.address}><button type="button" onClick={() => pick(s.address)}>{s.name ? `${s.name} · ` : ""}{s.address}</button></li>)}
        </ul>
      ) : null}
      <label>{t.subject}<input value={subject} maxLength={300} onChange={(e) => setSubject(e.target.value)} /></label>
      <label>{t.body}<textarea rows={8} value={body} onChange={(e) => setBody(e.target.value)} /></label>
      {problem ? <p role="alert" className="hub-error">{problem}</p> : null}
      <div className="hub-actions"><button type="button" onClick={onCancel}>{t.cancel}</button><button type="submit" className="primary">{t.save}</button></div>
    </form>
  );
}
