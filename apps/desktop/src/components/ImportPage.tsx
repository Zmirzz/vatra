import { useEffect, useMemo, useState } from "react";
import { copy } from "../copy";
import { useClient } from "../rpc/context";
import type { CatalogEntry, HistoryProject, ImportItemChoice, ImportOutcome, ScanItem, ScanView } from "../rpc/types";
import { useAppStore } from "../state/store";

type Tab = "projects" | "connections" | "skills";
const KINDS: Record<Exclude<Tab, "projects">, string[]> = { connections: ["mcp_server"], skills: ["skill", "command"] };
const ORIGINS: Record<string, string> = { claude: "Claude Code", codex: "Codex" };

const hasCode = (item: ScanItem) => (item.runs_code ?? []).length > 0 || item.report.routes.some(([, verdict]) => typeof verdict === "object" && "needs_approval" in verdict);
const excluded = (item: ScanItem) => ["hook", "agent", "unsupported"].includes(item.report.kind) || item.status === null;
const tabOf = (item: ScanItem): Exclude<Tab, "projects"> | null => (Object.entries(KINDS).find(([, kinds]) => kinds.includes(item.report.kind))?.[0] as Exclude<Tab, "projects"> | undefined) ?? null;
const emptyChoice = (item: ScanItem): ImportItemChoice => ({ key: item.key, include: false, approve_code: false, secrets: {}, allow_rules: false });
const when = (text: string) => { const date = new Date(text); return Number.isNaN(date.getTime()) ? "" : date.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" }); };

/** Bringing over what a person already has in Claude Code or Codex: only connections, skills, and whole projects. */
export function ImportPage({ onMarket }: { onMarket?: () => void }) {
  const client = useClient();
  const [tab, setTab] = useState<Tab>("projects");
  const [scan, setScan] = useState<ScanView | null>(null);
  const [history, setHistory] = useState<HistoryProject[] | null>(null);
  const [choices, setChoices] = useState<Record<string, ImportItemChoice>>({});
  const [variants, setVariants] = useState<Record<string, string>>({});
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState<"scan" | "apply" | null>(null);
  const [progress, setProgress] = useState<{ done: number; total: number; label: string } | null>(null);
  const [result, setResult] = useState<ImportOutcome | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [cancelling, setCancelling] = useState(false);

  useEffect(() => client.on((notice) => { if (notice.method === "import_progress") setProgress(notice.params); }), [client]);
  // A search that runs past a minute is stuck or very slow; say so, so the person isn't left looking at a spinner.
  const [slow, setSlow] = useState(false);
  useEffect(() => {
    setSlow(false);
    if (busy !== "scan") return;
    const timer = setTimeout(() => setSlow(true), 60_000);
    return () => clearTimeout(timer);
  }, [busy]);

  const find = async () => {
    setBusy("scan"); setError(null); setProgress(null); setResult(null); setVariants({}); setPicked(new Set());
    try {
      const [found, projects] = await Promise.all([client.call<ScanView>("import.scan", {}), client.call<HistoryProject[]>("import.history", {})]);
      setScan(found); setHistory(projects);
      setChoices(Object.fromEntries([...found.items, ...(found.alternatives ?? [])].map((item) => [item.key, emptyChoice(item)])));
    } catch (caught) { setError((caught as Error).message); } finally { setBusy(null); }
  };
  useEffect(() => { void find(); }, [client]); // eslint-disable-line react-hooks/exhaustive-deps

  const update = (item: ScanItem, change: Partial<ImportItemChoice>) => setChoices((old) => ({ ...old, [item.key]: { ...(old[item.key] ?? emptyChoice(item)), ...change } }));
  const items = useMemo(() => scan?.items.map((item) => scan.alternatives?.find((other) => other.key === variants[item.key]) ?? item) ?? [], [scan, variants]);
  const visible = (which: Exclude<Tab, "projects">) => items.filter((item) => tabOf(item) === which && !excluded(item) && item.status !== "unchanged" && item.project_known);
  const chosen = (which: Exclude<Tab, "projects">) => visible(which).filter((item) => choices[item.key]?.include).length;
  const projectCount = picked.size;
  const conversationCount = (history ?? []).filter((project) => picked.has(project.key)).reduce((sum, project) => sum + project.conversations, 0);
  const summary = [
    projectCount ? `${copy.bringOver.projectsCount(projectCount)} (${copy.bringOver.conversations(conversationCount)})` : "",
    chosen("connections") ? copy.bringOver.connectionsCount(chosen("connections")) : "",
    chosen("skills") ? copy.bringOver.skillsCount(chosen("skills")) : "",
  ].filter(Boolean);

  const apply = async () => {
    setBusy("apply"); setError(null); setProgress(null); setCancelling(false);
    try {
      const shown = new Set(items.filter((item) => tabOf(item) !== null && !excluded(item)).map((item) => item.key));
      const payload = items.map((item) => (shown.has(item.key) ? choices[item.key] ?? emptyChoice(item) : emptyChoice(item)));
      setResult(await client.call<ImportOutcome>("import.apply", { items: payload, include_history: false, history_projects: [...picked] }));
      await client.call<CatalogEntry[]>("catalog.list", {}).then((catalog) => useAppStore.setState({ catalog })).catch(() => undefined);
    } catch (caught) { setError((caught as Error).message); } finally { setBusy(null); }
  };

  const row = (item: ScanItem) => {
    const choice = choices[item.key] ?? emptyChoice(item);
    const original = scan?.items.find((candidate) => candidate.key === item.key || variants[candidate.key] === item.key);
    const conflict = scan?.conflicts.find((candidate) => candidate.name === item.report.name && candidate.kind === item.report.kind && candidate.kept.source === original?.source);
    const copies = original ? [original, ...(scan?.alternatives ?? []).filter((other) => other.report.name === item.report.name && other.report.kind === item.report.kind && other.project === item.project)] : [];
    const code = hasCode(item);
    return (
      <article className="import-row" key={original?.key ?? item.key}>
        <label className="import-line">
          <input type="checkbox" aria-label={item.report.name} disabled={busy !== null} checked={choice.include} onChange={(event) => update(item, { include: event.target.checked, approve_code: false })} />
          <b>{item.report.name}</b>
          <small>{ORIGINS[item.report.origin] ?? item.report.origin}</small>
        </label>
        {item.secrets.length > 0 && choice.include ? (
          <div className="import-note">
            <span>{copy.bringOver.needsKey}</span>
            <label className="check"><input type="checkbox" checked={Object.values(choice.secrets).some(Boolean)} disabled={busy !== null} onChange={(event) => update(item, { secrets: Object.fromEntries(item.secrets.map(({ key }) => [key, event.target.checked])) })} /> {copy.bringOver.bringKey}</label>
            <small>{copy.bringOver.withoutKey}</small>
          </div>
        ) : null}
        {code && choice.include ? (
          <div className="import-note">
            <span>{copy.bringOver.runsProgram}</span>
            <label className="check"><input type="checkbox" checked={choice.approve_code} disabled={busy !== null} onChange={(event) => update(item, { approve_code: event.target.checked })} /> {copy.bringOver.approve}</label>
          </div>
        ) : null}
        {code || (conflict && copies.length > 1) ? (
          <details>
            <summary>{copy.bringOver.details}</summary>
            {(item.runs_code ?? []).map((run, index) => <pre key={index}>{run.command}</pre>)}
            {conflict && copies.length > 1 ? (
              <label className="setting"><span>{copy.transfer.chooseCopy}</span>
                <select aria-label={`${copy.transfer.chooseCopy}: ${item.report.name}`} value={item.key} disabled={busy !== null} onChange={(event) => {
                  const selected = copies.find((candidate) => candidate.key === event.target.value)!;
                  setVariants((old) => ({ ...old, [original!.key]: selected.key }));
                }}>{copies.map((candidate) => <option key={candidate.key} value={candidate.key}>{ORIGINS[candidate.report.origin] ?? candidate.report.origin} · {candidate.source}</option>)}</select>
              </label>
            ) : null}
          </details>
        ) : null}
      </article>
    );
  };

  const setAll = (which: Exclude<Tab, "projects">, on: boolean) => setChoices((old) => {
    const next = { ...old };
    for (const item of visible(which)) next[item.key] = { ...(next[item.key] ?? emptyChoice(item)), include: on && !hasCode(item), approve_code: false };
    return next;
  });
  const tabs: [Tab, string, number][] = [
    ["projects", copy.bringOver.tabs.projects, history?.length ?? 0],
    ["connections", copy.bringOver.tabs.connections, visible("connections").length],
    ["skills", copy.bringOver.tabs.skills, visible("skills").length],
  ];
  const failed = (result?.results ?? []).filter((entry) => entry.state.state === "failed");
  const worked = (result?.results ?? []).filter((entry) => entry.state.state === "added" || entry.state.state === "changed");
  const conversationsDone = worked.filter((entry) => entry.key.startsWith("history:")).length;
  const itemsDone = worked.length - conversationsDone;

  return (
    <section className="transfer-page import-page">
      <h3>{copy.bringOver.title}</h3>
      <p className="help">{copy.bringOver.intro}</p>
      {busy === "scan" ? <p role="status">{slow ? copy.bringOver.slow : copy.bringOver.looking}</p> : null}
      {error ? <p role="alert">{copy.transfer.failed} {error}</p> : null}
      {scan && history ? (
        <>
          <div className="import-tabs" role="tablist">
            {tabs.map(([id, label, count]) => (
              <button key={id} type="button" role="tab" aria-selected={tab === id} onClick={() => setTab(id)}>{label} <small>{count}</small></button>
            ))}
          </div>

          {tab === "projects" ? (
            <div role="tabpanel" aria-label={copy.bringOver.tabs.projects}>
              <p className="help">{copy.bringOver.projectsHelp}</p>
              {history.length === 0 ? <p>{copy.bringOver.nothingProjects}</p> : (
                <>
                  <div className="import-all">
                    <button type="button" className="link" onClick={() => setPicked(new Set(history.map((project) => project.key)))}>{copy.bringOver.selectAll}</button>
                    <button type="button" className="link" onClick={() => setPicked(new Set())}>{copy.bringOver.selectNone}</button>
                  </div>
                  {history.map((project) => (
                    <article className="import-row" key={project.key}>
                      <label className="import-line">
                        <input type="checkbox" aria-label={project.name || copy.bringOver.noProject} disabled={busy !== null} checked={picked.has(project.key)} onChange={(event) => setPicked((old) => { const next = new Set(old); if (event.target.checked) next.add(project.key); else next.delete(project.key); return next; })} />
                        <b>{project.name || copy.bringOver.noProject}</b>
                        <small>{copy.bringOver.conversations(project.conversations)}{when(project.last_active) ? ` · ${when(project.last_active)}` : ""}</small>
                      </label>
                    </article>
                  ))}
                </>
              )}
            </div>
          ) : (
            <div role="tabpanel" aria-label={copy.bringOver.tabs[tab]}>
              <p className="help">{tab === "connections" ? copy.bringOver.connectionsHelp : copy.bringOver.skillsHelp}</p>
              {visible(tab).length === 0 ? <p>{tab === "connections" ? copy.bringOver.nothingConnections : copy.bringOver.nothingSkills}</p> : (
                <>
                  <div className="import-all">
                    <button type="button" className="link" onClick={() => setAll(tab, true)}>{copy.bringOver.selectAll}</button>
                    <button type="button" className="link" onClick={() => setAll(tab, false)}>{copy.bringOver.selectNone}</button>
                  </div>
                  {visible(tab).map(row)}
                </>
              )}
              {tab === "connections" && onMarket ? <button type="button" className="link" onClick={onMarket}>{copy.bringOver.market}</button> : null}
            </div>
          )}

          {scan.problems.map((problem, index) => <p role="alert" key={index}>{problem.source}: {problem.message}</p>)}
          {busy || progress ? <p role="status">{progress ? `${progress.done} / ${progress.total}${progress.label ? ` · ${progress.label}` : ""}` : busy === "apply" ? copy.transfer.applying : ""}</p> : null}
          <div className="import-actions">
            <button type="button" className="primary" disabled={busy !== null || summary.length === 0} onClick={apply}>{summary.length ? copy.bringOver.bring(summary) : copy.bringOver.nothingPicked}</button>
            {busy === "apply" ? <button type="button" disabled={cancelling} onClick={async () => { setCancelling(true); try { await client.call("import.cancel", {}); } catch (caught) { setError((caught as Error).message); setCancelling(false); } }}>{cancelling ? copy.transfer.stopping : copy.cancel}</button> : null}
          </div>
        </>
      ) : null}

      {result ? (
        <section className="import-result" aria-label={copy.bringOver.done}>
          <h3>{copy.bringOver.done}</h3>
          <p>{copy.bringOver.doneBody(itemsDone, conversationsDone)}</p>
          {failed.length ? <><p role="alert">{copy.bringOver.failedSome(failed.length)}</p>{failed.slice(0, 8).map((entry) => <div key={entry.key}><b>{entry.name}</b><p className="help">{entry.state.message ?? entry.state.reason}</p></div>)}</> : null}
          <button type="button" className="link" disabled={busy !== null} onClick={() => void find()}>{copy.bringOver.again}</button>
        </section>
      ) : null}
    </section>
  );
}
