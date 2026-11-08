import {useCallback, useEffect, useState, type FormEvent} from "react";
import {agentLabel} from "../../conversation/labels";
import {useHubCopy} from "../../hub/copy";
import {useMore} from "../../hub/moreCopy";
import {toast} from "../../hub/toast";
import type {ConnectorView} from "../../hub/types";
import {useClient} from "../../rpc/context";
import {useAppStore} from "../../state/store";
import {OAuthSignIn} from "./OAuthSignIn";

type FieldKind = "text" | "number" | "select" | "check" | "agent" | "textarea";
type Field = {key: string; kind: FieldKind; secret?: boolean; required?: boolean; options?: string[]; initial?: unknown};

const SECURITY = ["tls", "starttls", "plain"];
const SEARCH_PROVIDERS = ["brave", "tavily", "serper", "searxng"];

/** The fields of one kind of account. A search account asks for a key, or for an address when it is SearXNG. */
function fieldsFor(kind: string, values: Record<string, unknown>): Field[] {
  switch (kind) {
    case "caldav":
      return [{key: "url", kind: "text", required: true}, {key: "username", kind: "text", required: true}, {key: "password", kind: "text", secret: true, required: true}];
    case "email":
      return [
        {key: "address", kind: "text", required: true}, {key: "display_name", kind: "text"}, {key: "username", kind: "text"}, {key: "password", kind: "text", secret: true, required: true},
        {key: "imap_host", kind: "text", required: true}, {key: "imap_port", kind: "number", required: true, initial: 993}, {key: "imap_security", kind: "select", options: SECURITY, initial: "tls"},
        {key: "smtp_host", kind: "text", required: true}, {key: "smtp_port", kind: "number", required: true, initial: 465}, {key: "smtp_security", kind: "select", options: SECURITY, initial: "tls"},
        {key: "signature", kind: "textarea"},
      ];
    case "websearch":
      return [
        {key: "provider", kind: "select", options: SEARCH_PROVIDERS, initial: "brave"},
        values.provider === "searxng" ? {key: "url", kind: "text", required: true} : {key: "api_key", kind: "text", secret: true, required: true},
      ];
    case "telegram":
      return [
        {key: "bot_token", kind: "text", secret: true, required: true}, {key: "chat_id", kind: "number", required: true}, {key: "agent", kind: "agent"}, {key: "notify_tasks", kind: "check", initial: true},
      ];
    case "ntfy":
      return [{key: "topic", kind: "text", required: true}, {key: "server", kind: "text"}, {key: "token", kind: "text", secret: true}];
    case "embeddings":
      return [
        {key: "base_url", kind: "text", required: true, initial: "http://localhost:11434/v1"},
        {key: "model", kind: "text", required: true, initial: "nomic-embed-text"},
        {key: "api_key", kind: "text", secret: true},
      ];
    default:
      return [];
  }
}
const KINDS = ["websearch", "email", "caldav", "telegram", "ntfy", "embeddings"];

export function AccountsPage() {
  const t = useHubCopy();
  const m = useMore();
  const rpc = useClient();
  const [list, setList] = useState<ConnectorView[] | null>(null);
  const [editing, setEditing] = useState<{kind: string; existing: ConnectorView | null} | null>(null);
  const [status, setStatus] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [ddg, setDdg] = useState<boolean | null>(null);
  const [browser, setBrowser] = useState<{on: boolean; found: boolean} | null>(null);
  const [memory, setMemory] = useState<{configured: boolean; model?: string; sources?: number; pending?: number} | null>(null);

  const load = useCallback(async () => {
    try { setList(await rpc.call<ConnectorView[]>("connectors.list", {})); setError(null); } catch { setError(t.loadError); setList([]); }
  }, [rpc, t]);
  useEffect(() => { void load(); }, [load]);
  useEffect(() => {
    void rpc.call<{duckduckgo: boolean; browser: boolean; browser_found: boolean}>("research.settings", {}).then((s) => { setDdg(s.duckduckgo); setBrowser({on: s.browser, found: s.browser_found}); }).catch(() => undefined);
  }, [rpc]);
  const loadMemory = useCallback(() => {
    void rpc.call<{configured: boolean; model?: string; sources?: number; pending?: number}>("semantic.status", {}).then(setMemory).catch(() => undefined);
  }, [rpc]);
  useEffect(() => {
    loadMemory();
    const timer = setInterval(loadMemory, 6000);
    return () => clearInterval(timer);
  }, [loadMemory, list]);

  const kindName = (kind: string) => t.kinds[kind] ?? m.kindsExtra[kind] ?? kind;
  const kindHelp = (kind: string) => t.kindHelp[kind] ?? m.kindHelpExtra[kind] ?? "";

  const test = async (id: string) => {
    setStatus((s) => ({...s, [id]: t.working}));
    try { await rpc.call("connectors.check", {id}); setStatus((s) => ({...s, [id]: t.testOk})); } catch (e) { setStatus((s) => ({...s, [id]: e instanceof Error && e.message ? e.message : t.testFailed})); }
  };
  const remove = async (id: string) => {
    try { await rpc.call("connectors.remove", {id}); await load(); } catch { setError(t.saveError); }
  };
  const toggleBrowser = async (on: boolean) => {
    setBrowser((b) => (b ? {...b, on} : b));
    try { await rpc.call("research.settings", {browser: on}); } catch { setBrowser((b) => (b ? {...b, on: !on} : b)); toast(t.saveError, "error"); }
  };
  const reindex = async () => {
    try { await rpc.call("semantic.reindex", {}); toast(m.memoryAgainDone); loadMemory(); } catch { toast(t.saveError, "error"); }
  };
  const toggleDdg = async (on: boolean) => {
    setDdg(on);
    try { await rpc.call("research.settings", {duckduckgo: on}); } catch { setDdg(!on); toast(t.saveError, "error"); }
  };

  return (
    <div className="hub-accounts">
      <p className="hub-note">{t.accountsHelp}</p>
      {error ? <p role="alert" className="hub-error">{error}</p> : null}
      <section className="hub-card hub-ddg" aria-label={m.searchWithoutAccount}>
        <div className="hub-card-main">
          <b>{m.searchWithoutAccount}</b>
          <span className="hub-meta">{m.ddgHelp}</span>
        </div>
        <label className="hub-check"><input type="checkbox" checked={ddg ?? true} disabled={ddg === null} onChange={(e) => void toggleDdg(e.target.checked)} />{m.ddgOn}</label>
      </section>
      <section className="hub-card hub-ddg" aria-label={m.browserTitle}>
        <div className="hub-card-main">
          <b>{m.browserTitle}</b>
          <span className="hub-meta">{m.browserHelp}</span>
          {browser && !browser.found ? <span className="hub-meta">{m.browserMissing}</span> : null}
        </div>
        <label className="hub-check"><input type="checkbox" checked={browser?.on ?? false} disabled={!browser || !browser.found} onChange={(e) => void toggleBrowser(e.target.checked)} />{m.browserOn}</label>
      </section>
      {memory?.configured ? (
        <section className="hub-card hub-ddg" aria-label={m.memoryTitle}>
          <div className="hub-card-main">
            <b>{m.memoryTitle}</b>
            <span className="hub-meta">{m.memoryHelp}</span>
            <span className="hub-meta">{m.memoryIndexed(memory.sources ?? 0)} · {m.memoryPending(memory.pending ?? 0)} · {m.memoryModel(memory.model ?? "")}</span>
          </div>
          <button type="button" onClick={() => void reindex()}>{m.memoryAgain}</button>
        </section>
      ) : null}
      {list && list.length === 0 ? <p className="hub-meta">{t.noAccounts}</p> : null}
      <ul className="hub-list">
        {list?.map((c) => (
          <li key={c.id} className="hub-card">
            <div className="hub-card-main"><b>{c.name}</b><span className="hub-meta">{kindName(c.kind)}</span>{status[c.id] ? <span role="status">{status[c.id]}</span> : null}</div>
            <div className="hub-actions">
              <button type="button" onClick={() => void test(c.id)}>{t.test}</button>
              <button type="button" onClick={() => setEditing({kind: c.kind, existing: c})}>{t.edit}</button>
              <button type="button" onClick={() => void remove(c.id)}>{t.remove}</button>
            </div>
          </li>
        ))}
      </ul>
      {editing ? (
        <AccountForm key={editing.existing?.id ?? editing.kind} kind={editing.kind} existing={editing.existing} kindName={kindName(editing.kind)} kindHelp={kindHelp(editing.kind)}
          onCancel={() => setEditing(null)} onSaved={() => { setEditing(null); void load(); toast(t.saved); }} />
      ) : (
        <div className="hub-kinds" role="group" aria-label={t.addAccount}>
          <h3 className="hub-sub">{t.addAccount}</h3>
          {KINDS.map((kind) => (
            <button key={kind} type="button" className="hub-kind" onClick={() => setEditing({kind, existing: null})}><b>{kindName(kind)}</b><small>{kindHelp(kind)}</small></button>
          ))}
        </div>
      )}
    </div>
  );
}

function AccountForm({kind, existing, kindName, kindHelp, onCancel, onSaved}: {kind: string; existing: ConnectorView | null; kindName: string; kindHelp: string; onCancel: () => void; onSaved: () => void}) {
  const t = useHubCopy();
  const m = useMore();
  const rpc = useClient();
  const agents = useAppStore((s) => s.agents);
  const [name, setName] = useState(existing?.name ?? kindName);
  const [values, setValues] = useState<Record<string, unknown>>(() => {
    const start: Record<string, unknown> = {};
    for (const f of [...fieldsFor(kind, {provider: "brave"}), ...fieldsFor(kind, {provider: "searxng"})]) {
      start[f.key] = f.secret ? "" : (existing?.settings[f.key] ?? f.initial ?? (f.kind === "check" ? false : ""));
    }
    return start;
  });
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const set = (key: string, value: unknown) => setValues((old) => ({...old, [key]: value}));
  const fields = fieldsFor(kind, values);

  const securityLabel = (id: string) => (id === "tls" ? t.secure : id === "starttls" ? t.starttls : t.plain);
  const labelOf = (key: string) => (key === "base_url" ? m.baseUrl : key === "model" ? m.embeddingModel : key === "provider" ? m.provider : key === "signature" ? m.signature : key === "topic" ? m.topic : key === "server" ? m.server : key === "token" ? m.token : key === "url" && kind === "websearch" ? m.serverAddress : t.field[key] ?? key);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const settings: Record<string, unknown> = {};
    const secrets: Record<string, string> = {};
    for (const f of fields) {
      const value = values[f.key];
      if (f.secret) { if (String(value ?? "").trim()) secrets[f.key] = String(value).trim(); continue; }
      if (f.kind === "number") { if (value !== "" && value !== undefined) settings[f.key] = Number(value); continue; }
      if (typeof value === "string" ? value.trim() !== "" : value !== undefined) settings[f.key] = typeof value === "string" ? value.trim() : value;
    }
    const missing = fields.some((f) => f.required && (f.secret ? !secrets[f.key] && !existing?.hasSecret[f.key] : settings[f.key] === undefined));
    if (missing || !name.trim()) { setProblem(t.saveError); return; }
    setBusy(true);
    try {
      await rpc.call("connectors.save", {id: existing?.id, kind, name: name.trim(), settings, secrets});
      onSaved();
    } catch (e) { setProblem(e instanceof Error ? e.message : t.saveError); } finally { setBusy(false); }
  };

  return (
    <form className="hub-form" onSubmit={(e) => void submit(e)} aria-label={kindName}>
      <h3>{kindName}</h3>
      <p className="hub-meta">{kindHelp}</p>
      {!existing && (kind === "email" || kind === "caldav") ? <OAuthSignIn purpose={kind === "email" ? "email" : "calendar"} name={name === kindName ? "" : name} onSaved={onSaved} /> : null}
      <label>{t.field.name}<input value={name} maxLength={80} onChange={(e) => setName(e.target.value)} /></label>
      {fields.map((f) => {
        const label = labelOf(f.key);
        if (f.kind === "select") {
          return (
            <label key={f.key}>{label}
              <select value={String(values[f.key])} onChange={(e) => set(f.key, e.target.value)}>
                {f.options?.map((o) => <option key={o} value={o}>{f.key === "provider" ? m.providers[o] : securityLabel(o)}</option>)}
              </select>
            </label>
          );
        }
        if (f.kind === "check") return <label key={f.key} className="hub-check"><input type="checkbox" checked={Boolean(values[f.key])} onChange={(e) => set(f.key, e.target.checked)} />{label}</label>;
        if (f.kind === "agent") return <label key={f.key}>{label}<select value={String(values[f.key] ?? "")} onChange={(e) => set(f.key, e.target.value)}><option value="">—</option>{agents.map((a) => <option key={a} value={a}>{agentLabel(a)}</option>)}</select></label>;
        if (f.kind === "textarea") return <label key={f.key}>{label}<textarea rows={3} maxLength={600} value={String(values[f.key] ?? "")} onChange={(e) => set(f.key, e.target.value)} /></label>;
        return (
          <label key={f.key}>{label}
            <input type={f.secret ? "password" : f.kind === "number" ? "number" : "text"} autoComplete="off" value={String(values[f.key] ?? "")}
              placeholder={f.secret && existing?.hasSecret[f.key] ? t.keepSaved : undefined} onChange={(e) => set(f.key, e.target.value)} />
          </label>
        );
      })}
      {problem ? <p role="alert" className="hub-error">{problem}</p> : null}
      <div className="hub-actions"><button type="button" onClick={onCancel}>{t.cancel}</button><button type="submit" className="primary" disabled={busy}>{t.save}</button></div>
    </form>
  );
}
