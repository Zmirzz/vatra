import { useEffect, useState } from "react";
import { copy } from "../copy";
import { useClient } from "../rpc/context";
import type { CatalogEntry, DepPackage, SecretName } from "../rpc/types";
import { DependencyDialog } from "./DependencyDialog";
import { useAppStore } from "../state/store";
import { RouteMarks } from "./RouteMarks";

function SecretField({ entry, secret, onSaved }: { entry: CatalogEntry; secret: SecretName; onSaved: () => void }) {
  const client = useClient();
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return <form className="secret-field" onSubmit={async (event) => {
    event.preventDefault(); setBusy(true); setError(null);
    try { await client.call("catalog.set_secret", { id: entry.id, key: secret.key, value }); setValue(""); onSaved(); }
    catch (caught) { setError((caught as Error).message); } finally { setBusy(false); }
  }}><label>{secret.key}<input type="password" autoComplete="new-password" value={value} disabled={busy} onChange={(event) => setValue(event.target.value)} /></label><button type="submit" disabled={!value || busy}>{copy.transfer.value}</button>{error ? <p role="alert">{error}</p> : null}</form>;
}

export function ExtensionsPage() {
  const client = useClient();
  const projects = useAppStore((state) => state.projects);
  const [entries, setEntries] = useState<CatalogEntry[]>([]);
  const [statuses, setStatuses] = useState<{ name: string; oauth: boolean; logged_in: boolean }[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<number | null>(null);
  const [login, setLogin] = useState<string | null>(null);
  const [deps, setDeps] = useState<DepPackage[]>([]);
  const [installing, setInstalling] = useState<string | null>(null);
  const refresh = async () => {
    const list = await client.call<CatalogEntry[]>("catalog.list", {});
    setEntries(list); useAppStore.setState({ catalog: list });
  };
  useEffect(() => { let live = true;
    client.call<{ packages: DepPackage[] }>("deps.status", {}).then((status) => { if (live) setDeps(status.packages ?? []); }).catch(() => undefined);
    Promise.all([client.call<CatalogEntry[]>("catalog.list", {}), client.call<typeof statuses>("mcp.status", {})]).then(([list, status]) => { if (live) { setEntries(list); setStatuses(status); useAppStore.setState({ catalog: list }); } }).catch((caught) => { if (live) setError(caught.message); }).finally(() => { if (live) setLoading(false); });
    const off = client.on((notice) => { if (notice.method === "mcp_login") { setLogin(notice.params.message); if (!notice.params.ok) setError(notice.params.message); else void refresh().catch((caught) => setError(caught.message)); } });
    return () => { live = false; off(); };
  }, [client]);
  const action = async (id: number, fn: () => Promise<unknown>) => { setBusy(id); setError(null); try { await fn(); } catch (caught) { setError((caught as Error).message); } finally { setBusy(null); } };
  return <section className="transfer-page"><h3>{copy.transfer.extensions}</h3>
    {loading ? <p role="status">{copy.transfer.scanning}</p> : entries.length === 0 ? <p className="help">{copy.transfer.none}</p> : null}
    {error ? <p role="alert">{error}</p> : null}{login ? <p role="status">{login}</p> : null}
    {entries.length ? deps.filter((item) => !item.installed && item.purpose === "extensions").map((item) => <button key={item.id} type="button" onClick={() => setInstalling(item.id)}>{copy.deps.title.replace("{name}", item.name)}</button>) : null}
    {installing ? <DependencyDialog id={installing} onClose={(done) => { setInstalling(null); if (done) client.call<{ packages: DepPackage[] }>("deps.status", {}).then((status) => setDeps(status.packages ?? [])).catch(() => undefined); }} /> : null}
    {entries.map((entry) => {
      const needsApproval = entry.runs_code.length > 0 && entry.content_hash !== entry.approved_hash;
      const unlinked = entry.kind === "memory" && entry.scope === "global" && Boolean(entry.details.native_project);
      const status = statuses.find((candidate) => candidate.name === entry.name);
      const needsLogin = entry.kind === "mcp_server" && (status ? status.oauth && !status.logged_in : Boolean(entry.details.oauth));
      return <article className="extension-card" key={entry.id}>
        <label className="extension-title"><input type="checkbox" aria-label={entry.name} checked={entry.enabled} disabled={busy !== null || unlinked} onChange={(event) => {
          const enabled = event.target.checked;
          void action(entry.id, async () => { await client.call("catalog.set_enabled", { id: entry.id, enabled }); setEntries((old) => old.map((item) => item.id === entry.id ? { ...item, enabled } : item)); useAppStore.setState((state) => ({ catalog: state.catalog.map((item) => item.id === entry.id ? { ...item, enabled } : item) })); });
        }} /><b>{entry.name}</b></label>
        <small className="help">{entry.origin}{entry.plugin ? ` · ${entry.plugin}` : ""}</small>
        <p className="help">{unlinked ? copy.transfer.unlinkedMemory : needsApproval ? copy.transfer.approvalNeeded : (entry.secrets ?? []).length ? copy.transfer.verdict.needs_input : entry.enabled ? copy.transfer.enabled : copy.transfer.disabled}</p>
        {unlinked ? <label className="setting"><span>{copy.transfer.linkProject}</span><select aria-label={copy.transfer.linkProject} value="" disabled={busy !== null} onChange={(event) => { const project_id = Number(event.target.value); if (project_id) void action(entry.id, async () => { await client.call("catalog.set_project", { id: entry.id, project_id }); await refresh(); }); }}><option value="" disabled>{copy.transfer.chooseProject}</option>{projects.map((project) => <option key={project.id} value={project.id}>{project.name}</option>)}</select></label> : null}
        <RouteMarks routes={entry.routes ?? []} />
        {needsLogin ? <button type="button" disabled={busy !== null} onClick={() => void action(entry.id, async () => { await client.call("mcp.login", { server: entry.name }); setLogin(copy.transfer.loginStarted); })}>{copy.transfer.login}</button> : null}
        <details><summary>{copy.details}</summary>
          {entry.runs_code.map((run, index) => <pre key={index}>{run.command}</pre>)}
          {needsApproval ? <button type="button" disabled={busy !== null} onClick={() => void action(entry.id, async () => { await client.call("catalog.approve", { id: entry.id, expected_hash: entry.content_hash }); await refresh(); })}>{copy.transfer.approve}</button> : null}
          {(entry.secrets ?? []).map((secret) => <SecretField key={secret.key} entry={entry} secret={secret} onSaved={() => { void refresh().catch((caught) => setError(caught.message)); }} />)}
        </details>
      </article>;
    })}
  </section>;
}
