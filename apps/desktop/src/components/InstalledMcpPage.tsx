import { useEffect, useState } from "react";
import { useClient } from "../rpc/context";
import type { CatalogEntry } from "../rpc/types";
import { useAppStore } from "../state/store";
import { usePreferences } from "../settings/preferences";
import { McpInstaller, type McpSource } from "./McpInstaller";

type Connection = { connected: boolean; disconnectedAfterTest?: boolean; needsLogin?: boolean; error?: string; problems?: string[]; serverInfo?: unknown; tools: { name: string; description: string; enabled: boolean; inputSchema: unknown }[]; resources: { name: string; uri: string }[]; prompts: { name: string; description: string }[] };

export function InstalledMcpPage() {
  const client = useClient(); const ro = usePreferences((state) => state.preferences.language === "ro");
  const projects = useAppStore((state) => state.projects);
  const [entries, setEntries] = useState<CatalogEntry[]>([]);
  const [projectId, setProjectId] = useState<number | null>(null);
  const [busy, setBusy] = useState(false); const [error, setError] = useState<string | null>(null);
  const [notice,setNotice]=useState<string|null>(null);
  const [snapshots, setSnapshots] = useState<Record<number, Connection>>({});
  const [confirmRemove, setConfirmRemove] = useState<number | null>(null);
  const [editing, setEditing] = useState<{ document: string; projectId: number | null; source?:McpSource|null } | null>(null);
  const [sources,setSources]=useState<{id:number;source:McpSource}[]>([]);
  const [adding, setAdding] = useState(false);
  async function refresh() { const [rows,metadata] = await Promise.all([client.call<CatalogEntry[]>("catalog.list", {}),client.call<typeof sources>("mcp.sources",{})]); setEntries(rows.filter((entry) => entry.kind === "mcp_server")); setSources(metadata); useAppStore.setState({ catalog: rows }); }
  useEffect(() => { let live = true; void refresh().catch((caught) => { if (live) setError(String(caught.message ?? caught)); }); const off = client.on((notification) => { if (notification.method === "mcp_login") { if (!notification.params.ok) setError(notification.params.message); else void refresh().catch((caught) => setError(String(caught))); } }); return () => { live = false; off(); }; }, [client]);
  async function action(run: () => Promise<void>) { setBusy(true); setError(null); try { await run(); } catch (caught) { setError(caught instanceof Error ? caught.message : String(caught)); } finally { setBusy(false); } }
  async function connection(entry: CatalogEntry, method: string) { await action(async () => { const state = await client.call<Connection>(method, { id: entry.id, projectId }); setSnapshots((old) => ({ ...old, [entry.id]: state })); }); }
  async function setTool(entry:CatalogEntry,name:string,enabled:boolean){
    const disabled=new Set(Array.isArray(entry.details.disabled_tools)?entry.details.disabled_tools as string[]:[]);
    if(enabled)disabled.delete(name);else disabled.add(name);
    const allowlist=Array.isArray(entry.details.enabled_tools)?new Set(entry.details.enabled_tools as string[]):null;
    if(allowlist){if(enabled)allowlist.add(name);else allowlist.delete(name);}
    await action(async()=>{
      await client.call("mcp.set_tools",{id:entry.id,expectedHash:entry.content_hash,disabledTools:[...disabled],...(allowlist?{enabledTools:[...allowlist]}:{})});
      await refresh();setSnapshots(old=>{const next={...old};delete next[entry.id];return next;});
    });
  }
  return <section className="mcp-manager">
    <p className="help">{ro ? "Instrumentele pe care le-ai adăugat. Alege un proiect ca să vezi ce conexiuni are active." : "The tools you have added. Choose a project to see which connections it has on."}</p>
    <label className="setting"><span>{ro ? "Proiect pentru conexiuni" : "Project for connections"}</span><select value={projectId ?? ""} onChange={(event) => { setProjectId(event.target.value ? Number(event.target.value) : null); setSnapshots({}); }}><option value="">{ro ? "Doar test izolat" : "Isolated test only"}</option>{projects.map((project) => <option key={project.id} value={project.id}>{project.name}</option>)}</select></label>
    <div className="dialog-actions"><button type="button" disabled={busy} onClick={() => setAdding(!adding)}>{ro ? "Adaugă sau importă MCP" : "Add or import MCP"}</button><button type="button" disabled={busy} onClick={() => void action(refresh)}>{ro ? "Reîncarcă" : "Reload"}</button></div>
    {adding ? <McpInstaller onSaved={() => void action(refresh)} onCancel={() => setAdding(false)} /> : null}
    {editing ? <McpInstaller key={editing.document} initialDocument={editing.document} initialProjectId={editing.projectId} source={editing.source} onSaved={() => { setEditing(null); void action(refresh); }} onCancel={() => setEditing(null)} /> : null}
    {error ? <p role="alert">{error}</p> : null}
    {notice?<p role="status">{notice}</p>:null}
    {!entries.length ? <p className="help">{ro ? "Niciun server MCP instalat în catalog." : "No MCP servers installed in the catalog."}</p> : null}
    {entries.map((entry) => {
      const approved = !entry.runs_code.length || entry.approved_hash === entry.content_hash;
      const snapshot = snapshots[entry.id];
      return <article className="registry-entry" key={entry.id}>
        <div className="setting"><h3>{entry.name}</h3><label><input type="checkbox" aria-label={`${ro ? "Activează" : "Enable"} ${entry.name}`} checked={entry.enabled} disabled={busy || !approved} onChange={(event) => { const enabled = event.target.checked; void action(async () => { await client.call("catalog.set_enabled", { id: entry.id, enabled }); await refresh(); setSnapshots((old) => { const next = { ...old }; delete next[entry.id]; return next; }); }); }} />{ro ? "Activat" : "Enabled"}</label></div>
        <p className="help">{entry.details.url ? "Streamable HTTP" : "stdio"} · {entry.origin} · {typeof entry.scope === "object" ? projects.find((project) => project.id === (typeof entry.scope === "object" ? entry.scope.project : null))?.name ?? `Project ${entry.scope.project}` : entry.scope}</p>
        {sources.find((item)=>item.id===entry.id)?<p className="help">{ro?"Referință registry: ":"Registry reference: "}{sources.find((item)=>item.id===entry.id)?.source.name} · {sources.find((item)=>item.id===entry.id)?.source.version}</p>:null}
        {entry.details.url ? <small className="help">{String(entry.details.url)}</small> : null}
        {!approved ? <div><p className="help">{ro ? "Aprobă comanda revizuită înainte de conectare. Poate instala pachete și accesa fișiere." : "Approve the reviewed command before connecting. It may install packages and access files."}</p>{entry.runs_code.map((run, index) => <pre key={index}>{run.command}</pre>)}<button type="button" disabled={busy} onClick={() => void action(async () => { await client.call("catalog.approve", { id: entry.id, expected_hash: entry.content_hash }); await refresh(); })}>{ro ? "Aprobă această comandă" : "Approve this command"}</button></div> : null}
        <div className="dialog-actions">
          <button type="button" disabled={busy || !approved} onClick={() => void connection(entry, "mcp.test")}>{ro ? "Testează conexiunea" : "Test connection"}</button>
          {entry.details.oauth ? <button type="button" disabled={busy} onClick={() => void action(async () => { await client.call("mcp.login_installed", { id: entry.id }); })}>{ro ? "Autentifică" : "Sign in"}</button> : null}
          {!entry.plugin ? <button type="button" disabled={busy} onClick={() => void action(async () => { setEditing(await client.call("mcp.form", { id: entry.id })); })}>{ro ? "Editează configurația" : "Edit configuration"}</button> : null}
          {!entry.plugin ? <button type="button" aria-label={`${ro ? "Dezinstalează" : "Uninstall"} ${entry.name}`} disabled={busy} onClick={() => setConfirmRemove(entry.id)}>{ro ? "Dezinstalează" : "Uninstall"}</button> : null}
        </div>
        {projectId !== null ? <div className="dialog-actions">
          {[["mcp.connect", ro ? "Conectează" : "Connect"], ["mcp.disconnect", ro ? "Deconectează" : "Disconnect"], ["mcp.reconnect", ro ? "Reconectează" : "Reconnect"], ["mcp.connection", ro ? "Stare conexiune" : "Connection status"]].map(([method, title]) => <button key={method} type="button" disabled={busy || !approved || !entry.enabled} onClick={() => void connection(entry, method)}>{title}</button>)}
        </div> : null}
        {confirmRemove === entry.id ? <div role="group" aria-label={ro ? "Confirmă dezinstalarea" : "Confirm uninstall"}><p>{ro ? "Elimină această configurație? Fișierele sursă și conversațiile sunt păstrate." : "Remove this configuration? Source files and conversations are preserved."}</p><button type="button" disabled={busy} aria-label={`${ro ? "Confirmă eliminarea" : "Confirm remove"} ${entry.name}`} onClick={() => void action(async () => { const result=await client.call<{credentialCleanupFailed?:boolean}>("mcp.remove", { id: entry.id, expectedHash: entry.content_hash }); setNotice(result.credentialCleanupFailed?(ro?"Configurația a fost eliminată, dar unele credențiale nu au putut fi șterse din keychain.":"Configuration removed, but some credentials could not be deleted from the keychain."):null); setConfirmRemove(null); await refresh(); })}>{ro ? "Confirmă eliminarea" : "Confirm remove"}</button><button type="button" onClick={() => setConfirmRemove(null)}>{ro ? "Anulează" : "Cancel"}</button></div> : null}
        {snapshot ? <div>
          <p role="status">{snapshot.disconnectedAfterTest ? (snapshot.connected ? (ro ? "Test reușit; conexiunea de test a fost închisă." : "Test succeeded; the test connection was closed.") : (ro ? "Test eșuat." : "Connection test failed.")) : (snapshot.connected ? (ro ? "Conectat" : "Connected") : (ro ? "Deconectat" : "Disconnected"))}</p>
          {snapshot.error ? <p role="alert">{snapshot.error}</p> : null}
          <details><summary>{ro ? "Tool-uri" : "Tools"} ({snapshot.tools.length})</summary>{snapshot.tools.map((tool) => <label className="setting" key={tool.name}><span title={tool.description}>{tool.name}</span><input type="checkbox" aria-label={tool.name} checked={tool.enabled} disabled={busy || Boolean(entry.plugin)} onChange={(event) => void setTool(entry,tool.name,event.target.checked)} /></label>)}</details>
          <details><summary>{ro ? "Resurse" : "Resources"} ({snapshot.resources.length})</summary>{snapshot.resources.map((resource) => <p key={resource.uri}>{resource.name}: {resource.uri}</p>)}</details>
          <details><summary>Prompts ({snapshot.prompts.length})</summary>{snapshot.prompts.map((prompt) => <p key={prompt.name}>{prompt.name}: {prompt.description}</p>)}</details>
          <details><summary>{ro ? "Capabilități negociate" : "Negotiated capabilities"}</summary><pre>{JSON.stringify(snapshot.serverInfo, null, 2)}</pre></details>
        </div> : null}
      </article>;
    })}
  </section>;
}
