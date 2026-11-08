import { useState } from "react";
import { useClient } from "../rpc/context";
import { useAppStore } from "../state/store";
import { usePreferences } from "../settings/preferences";

type Review = { plan: { config: { name: string; command?: string; args: string[]; url?: string; env: Record<string,string>; headers: Record<string,string> }; secretNames: string[]; reviewHash: string }; existing: { id: number; hash: string } | null };
export type McpSource = { name:string; version:string; method:string; index:number };

export function McpInstaller({ initialDocument = "", initialProjectId = null, source, onSaved, onCancel }: { initialDocument?: string; initialProjectId?: number | null; source?:McpSource|null; onSaved?: () => void; onCancel?: () => void }) {
  const client = useClient(); const ro = usePreferences((state) => state.preferences.language === "ro");
  const projects = useAppStore((state) => state.projects);
  const [document, setDocument] = useState(initialDocument);
  const [projectId, setProjectId] = useState<number | null>(initialProjectId);
  const [review, setReview] = useState<Review[] | null>(null);
  const [busy, setBusy] = useState(false); const [error, setError] = useState<string | null>(null); const [saved, setSaved] = useState(false);
  async function action(run: () => Promise<void>) { setBusy(true); setError(null); try { await run(); } catch (caught) { setError(caught instanceof Error ? caught.message : String(caught)); } finally { setBusy(false); } }
  return <section className="mcp-installer">
    <h3>{ro ? "Configurează sau importă un server MCP" : "Configure or import an MCP server"}</h3>
    <p className="help">{ro ? "Introdu configurația mcpServers. Valorile din mediu și headere ajung în keychain. Salvarea nu execută comenzi; serverul rămâne dezactivat până îl activezi." : "Enter an mcpServers configuration. Environment and header values go to the keychain. Saving executes no commands; the server stays disabled until you enable it."}</p>
    <form className="agent-form" onSubmit={(event) => { event.preventDefault(); void action(async () => { const result = await client.call<{ servers: Review[] }>("mcp.preview", { document, projectId }); setReview(result.servers); setSaved(false); }); }}>
      <label>{ro ? "Configurație MCP JSON" : "MCP JSON configuration"}<textarea rows={9} spellCheck={false} value={document} onChange={(event) => { setDocument(event.target.value); setReview(null); }} placeholder={'{"mcpServers":{"example":{"type":"http","url":"https://example.org/mcp"}}}'} /></label>
      <label>{ro ? "Domeniu" : "Scope"}<select value={projectId ?? ""} onChange={(event) => { setProjectId(event.target.value ? Number(event.target.value) : null); setReview(null); }}>
        <option value="">Global</option>{projects.map((project) => <option key={project.id} value={project.id}>{project.name}</option>)}
      </select></label>
      <div className="dialog-actions">{onCancel ? <button type="button" onClick={onCancel}>{ro ? "Închide" : "Close"}</button> : null}<button type="submit" disabled={busy || !document.trim()}>{ro ? "Revizuiește configurația" : "Review configuration"}</button></div>
    </form>
    {error ? <p role="alert">{error}</p> : null}
    {saved ? <p role="status">{ro ? "Salvat dezactivat. Revizuiește aprobarea comenzii și testează conexiunea înainte de activare." : "Saved disabled. Review command approval and test the connection before enabling."}</p> : null}
    {review?.map((item) => <article className="registry-entry" key={item.plan.config.name}>
      <h3>{item.plan.config.name}</h3>
      <pre>{item.plan.config.command ? JSON.stringify([item.plan.config.command, ...item.plan.config.args], null, 2) : item.plan.config.url}</pre>
      {item.plan.config.command ? <p className="help">{ro ? "Această comandă poate instala pachete și accesa fișiere când este aprobată și conectată. Inspectează sursa și argumentele." : "This command may install packages and access files when approved and connected. Inspect its source and arguments."}</p> : null}
      {item.plan.secretNames.length ? <p className="help">{ro ? "Valori protejate în keychain: " : "Values protected in the keychain: "}{item.plan.secretNames.length}</p> : null}
      {item.existing ? <p className="help">{ro ? "Înlocuiește configurația existentă și o dezactivează. Istoricul conversațiilor este păstrat." : "Replaces the existing configuration and disables it. Conversation history is preserved."}</p> : null}
      <details><summary>{ro ? "Configurație fără valori secrete" : "Configuration without secret values"}</summary><pre>{JSON.stringify(item.plan.config, null, 2)}</pre></details>
      <button type="button" disabled={busy} aria-label={`${ro ? "Confirmă salvarea" : "Confirm save"} ${item.plan.config.name}`} onClick={() => void action(async () => {
        await client.call("mcp.install", { document, projectId, source, reviewHash: item.plan.reviewHash, ...(item.existing ? { replaceHash: item.existing.hash } : {}) });
        setReview(null); setDocument(""); setSaved(true); onSaved?.();
      })}>{ro ? "Confirmă salvarea" : "Confirm save"}</button>
    </article>)}
  </section>;
}
