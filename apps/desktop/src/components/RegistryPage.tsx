import { useEffect, useRef, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useClient } from "../rpc/context";
import { usePreferences } from "../settings/preferences";
import { McpInstaller, type McpSource } from "./McpInstaller";

type Entry = {
  id: string; name: string; description: string; version: string;
  publishers: string[]; repository: string | null; platformAvailable: boolean;
  details: Record<string, unknown>;
};
type Page = { entries: Entry[]; nextCursor: string | null; source: string };

function httpsLink(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  try { const url = new URL(value); return url.protocol === "https:" && !url.username && !url.password ? url.href : undefined; }
  catch { return undefined; }
}

/** Registry data is displayed as text. It cannot create commands, secrets or HTML. */
export function RegistryPage({ ecosystem }: { ecosystem: "acp" | "mcp" }) {
  const client = useClient();
  const ro = usePreferences((state) => state.preferences.language === "ro");
  const [query, setQuery] = useState("");
  const [platformOnly, setPlatformOnly] = useState(true);
  const [result, setResult] = useState<Page | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [configuration, setConfiguration] = useState<{document:string;source:McpSource} | null>(null);
  const [installed, setInstalled] = useState<{id:number;alias:string;source:McpSource}[]>([]);
  const [installing, setInstalling] = useState<string | null>(null);
  const [installNotice, setInstallNotice] = useState<{id:string;text:string} | null>(null);
  const generation = useRef(0);
  const lastQuery = useRef("");

  async function load(cursor?: string) {
    const current = ++generation.current;
    setBusy(true); setError(null);
    if (!cursor) { setResult(null); lastQuery.current = query; }
    try {
      const page = await client.call<Page>(`registry.${ecosystem}`, {
        query: cursor ? lastQuery.current : query, limit: 30,
        ...(cursor ? { cursor } : {}), ...(ecosystem === "acp" ? { platformOnly } : {}),
      });
      if (generation.current !== current) return;
      setResult((old) => {
        const entries = cursor && old ? [...old.entries, ...page.entries] : page.entries;
        return { ...page, entries: entries.filter((entry, index) => entries.findIndex((other) => other.id === entry.id && other.version === entry.version) === index) };
      });
    } catch (caught) {
      if (generation.current === current) setError(caught instanceof Error ? caught.message : String(caught));
    } finally { if (generation.current === current) setBusy(false); }
  }

  useEffect(() => {
    void load();
    return () => { ++generation.current; };
    // A submitted query owns pagination. Typing does not fetch or replace results.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, ecosystem]);
  useEffect(() => {let live=true;if(ecosystem==="mcp")void client.call<typeof installed>("mcp.sources",{}).then((list)=>{if(live)setInstalled(list);}).catch(()=>undefined);return()=>{live=false;};},[client,ecosystem]);

  async function configure(entry: Entry, method: string, index: number) {
    setBusy(true); setError(null);
    try {
      const alias = installed.find((item)=>item.source.name===entry.id)?.alias ?? ((entry.id.split("/").at(-1) ?? "server").replace(/[^a-zA-Z0-9._-]/g, "-").slice(0, 80) || "server");
      const result = await client.call<{ document: string }>("mcp.registry_config", { name: entry.id, version: entry.version, alias, method, index });
      setConfiguration({document:result.document,source:{name:entry.id,version:entry.version,method,index}});
    } catch (caught) { setError(caught instanceof Error ? caught.message : String(caught)); }
    finally { setBusy(false); }
  }

  async function installMcp(entry: Entry, method: string, index: number) {
    const key = `mcp:${entry.id}:${entry.version}:${method}:${index}`;
    setInstalling(key); setError(null); setInstallNotice(null);
    try {
      const alias = ((entry.id.split("/").at(-1) ?? "server").replace(/[^a-zA-Z0-9._-]/g, "-").slice(0, 80) || "server");
      await client.call("registry.mcp_install", { name: entry.id, version: entry.version, alias, method, index });
      void client.call<typeof installed>("mcp.sources",{}).then(setInstalled).catch(()=>undefined);
      setInstallNotice({id:entry.id,text:ro?"Instalat. Activează-l din MCP.":"Installed. Enable it from MCP."});
    } catch (caught) { setError(caught instanceof Error ? caught.message : String(caught)); }
    finally { setInstalling(null); }
  }

  async function installAcp(entry: Entry) {
    const key = `acp:${entry.id}:${entry.version}`;
    setInstalling(key); setError(null); setInstallNotice(null);
    try {
      const alias = ((entry.id.split("/").at(-1) ?? entry.name).replace(/[^a-zA-Z0-9._-]/g, "-").slice(0, 80) || entry.name);
      await client.call("registry.acp_install", { id: entry.id, version: entry.version, alias });
      setInstallNotice({id:entry.id,text:ro?"Agent instalat. Repornești aplicația pentru a-l vedea.":"Agent installed. Restart the app to see it."});
    } catch (caught) { setError(caught instanceof Error ? caught.message : String(caught)); }
    finally { setInstalling(null); }
  }

  return <section className="registry-page">
    <p className="help">{ro ? "Găsește agenți și instrumente în registrul oficial. Verifică la fiecare dacă funcționează cu ce ai și dacă cere autentificare." : "Find agents and tools in the official registry. Check each one works with what you have and whether it needs you to sign in."}</p>
    <form className="registry-search" onSubmit={(event) => { event.preventDefault(); void load(); }}>
      <input type="search" aria-label={ro ? "Caută în registru" : "Search registry"} placeholder={ro ? "Nume, editor sau descriere…" : "Name, publisher or description…"} value={query} maxLength={256} onChange={(event) => setQuery(event.target.value)} />
      <button type="submit" disabled={busy}>{ro ? "Caută" : "Search"}</button>
      {ecosystem === "acp" ? <label><input type="checkbox" checked={platformOnly} onChange={(event) => setPlatformOnly(event.target.checked)} />{ro ? "Distribuție pentru acest sistem" : "Distribution for this system"}</label> : null}
    </form>
    {busy ? <p role="status">{ro ? "Se încarcă registrul…" : "Loading registry…"}</p> : null}
    {error ? <p role="alert">{error}</p> : null}
    {installNotice ? <p role="status">{installNotice.text}</p> : null}
    {configuration !== null ? <McpInstaller key={configuration.document} initialDocument={configuration.document} source={configuration.source} onSaved={()=>{void client.call<typeof installed>("mcp.sources",{}).then(setInstalled).catch(()=>undefined);}} onCancel={() => setConfiguration(null)} /> : null}
    {result && !result.entries.length ? <p role="status">{ro ? "Niciun rezultat." : "No results."}</p> : null}
    {result?.entries.map((entry) => <article className="registry-entry" key={`${entry.id}:${entry.version}`}>
      <div className="registry-entry-heading"><h3>{entry.name}</h3><span className="help">{entry.version}</span></div>
      <p>{entry.description}</p>
      <small className="help">{entry.publishers.join(", ")}</small>
      {ecosystem==="mcp"&&installed.some((item)=>item.source.name===entry.id)?<p className="help">{ro?"Referință salvată din registry: ":"Saved registry reference: "}{installed.find((item)=>item.source.name===entry.id)?.source.version}{installed.find((item)=>item.source.name===entry.id)?.source.version!==entry.version?(ro?" · Versiunea din registry diferă":" · Registry version differs"):""}</p>:null}
      {ecosystem === "mcp" ? <div className="dialog-actions">
        {Array.isArray(entry.details.remotes) ? entry.details.remotes.map((remote, index) => <button key={`remote-${index}`} type="button" disabled={busy || installing===`mcp:${entry.id}:${entry.version}:remote:${index}`} onClick={() => void configure(entry, "remote", index)}>{ro ? "Configurează" : "Configure"} {String(remote.type ?? "remote")} {index + 1}</button>) : null}
        {Array.isArray(entry.details.packages) ? entry.details.packages.map((pkg, index) => <>
          <button key={`package-${index}`} type="button" disabled={busy || installing===`mcp:${entry.id}:${entry.version}:package:${index}`} onClick={() => void configure(entry, "package", index)}>{ro ? "Configurează" : "Configure"} {String(pkg.registryType ?? "package")} {index + 1}</button>
          <button key={`install-pkg-${index}`} type="button" disabled={busy || installing===`mcp:${entry.id}:${entry.version}:package:${index}`} onClick={() => void installMcp(entry, "package", index)} className="primary">{ro ? "Instalează" : "Install"} {String(pkg.registryType ?? "package")} {index + 1}</button>
        </>) : null}
      </div> : null}
      {httpsLink(entry.repository) ? <a href={httpsLink(entry.repository)} target="_blank" rel="noreferrer noopener" onClick={(event) => {
        if (isTauri()) { event.preventDefault(); void openUrl(httpsLink(entry.repository)!).catch((caught) => setError(String(caught))); }
      }}>{ro ? "Cod sursă" : "Repository"}</a> : null}
      {ecosystem === "acp" ? <div className="dialog-actions">
        {entry.platformAvailable ? <button type="button" disabled={busy || installing===`acp:${entry.id}:${entry.version}`} onClick={() => void installAcp(entry)} className="primary">{ro ? "Instalează agentul" : "Install agent"}</button> : <p className="help">{ro ? "Fără distribuție pentru acest sistem." : "No distribution for this system."}</p>}
      </div> : null}
      <details><summary>{ro ? "Detalii" : "Details"}</summary>
        <p className="help">{ro ? "Metadate externe. Nicio comandă nu este executată la vizualizare." : "External metadata. Viewing this does not execute any commands."}</p>
        <pre>{JSON.stringify(entry.details, null, 2)}</pre>
      </details>
    </article>)}
    {result?.nextCursor ? <button type="button" disabled={busy} onClick={() => void load(result.nextCursor!)}>{ro ? "Mai multe" : "Load more"}</button> : null}
    {result ? <small className="help">{ro ? "Sursă: " : "Source: "}{result.source}</small> : null}
  </section>;
}
