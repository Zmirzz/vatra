import { useEffect, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { useClient } from "../rpc/context";
import { usePreferences } from "../settings/preferences";
import { useAppStore } from "../state/store";
import type { CatalogEntry } from "../rpc/types";
import { Markdown } from "./Markdown";

type Source = { path: string; projectId: number | null };
type Skill = Source & { name: string; description: string; resources: string[]; errors: string[]; hash: string | null };

export function SkillsPage() {
  const client = useClient();
  const ro = usePreferences((state) => state.preferences.language === "ro");
  const projects = useAppStore((state) => state.projects);
  const [sources, setSources] = useState<Source[]>([]);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [entries, setEntries] = useState<CatalogEntry[]>([]);
  const [path, setPath] = useState("");
  const [projectId, setProjectId] = useState<number | null>(null);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [inspection, setInspection] = useState<{ metadata: Skill; content: string } | null>(null);
  async function refresh() {
    const [roots, found, catalog] = await Promise.allSettled([
      client.call<Source[]>("skills.sources", {}), client.call<Skill[]>("skills.scan", {}), client.call<CatalogEntry[]>("catalog.list", {}),
    ]);
    if (roots.status === "fulfilled") setSources(roots.value);
    if (found.status === "fulfilled") setSkills(found.value); else setSkills([]);
    if (catalog.status === "fulfilled") { setEntries(catalog.value.filter((entry) => entry.kind === "skill")); useAppStore.setState({ catalog: catalog.value }); }
    const failure = [roots, found, catalog].find((result) => result.status === "rejected");
    if (failure?.status === "rejected") throw failure.reason;
  }
  useEffect(() => { let live = true; void refresh().catch((caught) => { if (live) setError(String(caught.message ?? caught)); }).finally(() => { if (live) setBusy(false); }); return () => { live = false; }; }, [client]);
  async function action(run: () => Promise<void>) {
    setBusy(true); setError(null); setNotice(null);
    try { await run(); } catch (caught) { setError(caught instanceof Error ? caught.message : String(caught)); } finally { setBusy(false); }
  }
  const installed = (skill: Skill) => entries.find((entry) => entry.name === skill.name && entry.origin === "manual" && !entry.plugin && (skill.projectId === null ? entry.scope === "global" : typeof entry.scope === "object" && entry.scope.project === skill.projectId));
  return <section className="skills-manager">
    <p className="help">{ro ? "Adaugă directoare cu skills. Vatra citește sursa și păstrează o copie locală la instalare; scripturile nu sunt executate." : "Add skill directories. Vatra reads the source and keeps a local copy when installed; scripts are never executed during installation."}</p>
    <form className="agent-form" onSubmit={(event) => { event.preventDefault(); void action(async () => {
      await client.call("skills.set_sources", [...sources, { path, projectId }]); setPath(""); await refresh();
    }); }}>
      <label>{ro ? "Director sursă" : "Source directory"}<input value={path} onChange={(event) => setPath(event.target.value)} placeholder={"C:\\Users\\you\\.agents\\skills"} /></label>
      <label>{ro ? "Domeniu" : "Scope"}<select value={projectId ?? ""} onChange={(event) => setProjectId(event.target.value ? Number(event.target.value) : null)}>
        <option value="">Global</option>{projects.map((project) => <option key={project.id} value={project.id}>{project.name}</option>)}
      </select></label>
      <div className="dialog-actions">
        <button type="button" disabled={busy} onClick={() => void action(refresh)}>{ro ? "Reîncarcă" : "Reload"}</button>
        <button type="submit" disabled={busy || !path.trim()}>{ro ? "Adaugă sursa" : "Add source"}</button>
      </div>
    </form>
    {sources.map((source, index) => <div className="setting" key={`${source.path}:${source.projectId}`}>
      <span className="help">{source.path}{source.projectId ? ` (${projects.find((project) => project.id === source.projectId)?.name ?? source.projectId})` : ""}</span>
      <button type="button" disabled={busy} onClick={() => void action(async () => { await client.call("skills.set_sources", sources.filter((_, i) => i !== index)); await refresh(); })}>{ro ? "Elimină sursa" : "Remove source"}</button>
    </div>)}
    {busy ? <p role="status">{ro ? "Se procesează…" : "Working…"}</p> : null}
    {error ? <p role="alert">{error}</p> : null}{notice ? <p role="status">{notice}</p> : null}
    {!busy && !skills.length ? <p className="help">{ro ? "Niciun skill descoperit în sursele configurate." : "No skills discovered in the configured sources."}</p> : null}
    {skills.map((skill) => { const entry = installed(skill); return <article className="registry-entry" key={`${skill.path}:${skill.projectId}`}>
      <h3>{skill.name}</h3><p>{skill.description}</p><small className="help">{skill.path}</small>
      {skill.errors.map((error, index) => <p role="alert" key={index}>{error}</p>)}
      <div className="dialog-actions">
        <button type="button" disabled={busy} onClick={() => void action(async () => setInspection(await client.call("skills.inspect", { path: skill.path, projectId: skill.projectId })))}>{ro ? "Inspectează" : "Inspect"} SKILL.md</button>
        {isTauri() ? <>
          <button type="button" disabled={busy} onClick={() => void action(() => invoke("open_skill_source", {path:skill.path,projectId:skill.projectId,folder:false}))}>{ro ? "Deschide SKILL.md" : "Open SKILL.md"}</button>
          <button type="button" disabled={busy} onClick={() => void action(() => invoke("open_skill_source", {path:skill.path,projectId:skill.projectId,folder:true}))}>{ro ? "Deschide directorul" : "Open folder"}</button>
        </> : null}
        <button type="button" aria-label={`${entry ? (ro ? "Actualizează" : "Update") : (ro ? "Instalează" : "Install")} ${skill.name}`} disabled={busy || skill.errors.length > 0 || !skill.hash} onClick={() => void action(async () => {
          const report = await client.call<{ unapproved: string[]; problems: string[] }>("skills.install", { path: skill.path, projectId: skill.projectId, expectedHash: skill.hash, ...(entry ? { replaceHash: entry.content_hash } : {}) });
          await refresh(); setNotice(report.unapproved.length ? (ro ? "Instalat. Codul necesită aprobare separată în Extensii." : "Installed. Executable content needs separate approval in Extensions.") : (ro ? "Skill instalat." : "Skill installed."));
        })}>{entry ? (ro ? "Actualizează" : "Update") : (ro ? "Instalează" : "Install")}</button>
      </div>
      <details><summary>{ro ? "Resurse" : "Resources"}</summary><ul>{skill.resources.map((resource) => <li key={resource}>{resource}</li>)}</ul></details>
    </article>; })}
    <h3>{ro ? "Skills instalate" : "Installed skills"}</h3>
    {entries.map((entry) => <div className="setting" key={entry.id}>
      <label><input type="checkbox" checked={entry.enabled} disabled={busy} onChange={(event) => void action(async () => { await client.call("catalog.set_enabled", { id: entry.id, enabled: event.target.checked }); await refresh(); })} />{entry.name}<small className="help"> ({entry.origin})</small></label>
      {!entry.plugin ? <button type="button" disabled={busy} aria-label={`${ro ? "Dezinstalează" : "Uninstall"} ${entry.name}`} onClick={() => void action(async () => { await client.call("skills.uninstall", { id: entry.id }); await refresh(); })}>{ro ? "Dezinstalează" : "Uninstall"}</button> : null}
    </div>)}
    {inspection ? <div className="skill-inspection"><h3>{inspection.metadata.name} / SKILL.md</h3><button type="button" onClick={() => setInspection(null)}>{ro ? "Închide previzualizarea" : "Close preview"}</button><Markdown text={inspection.content} /></div> : null}
  </section>;
}
