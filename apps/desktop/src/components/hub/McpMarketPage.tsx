import {Check, Download, Plus, Search, Wrench} from "lucide-react";
import {useCallback, useEffect, useMemo, useRef, useState} from "react";
import {useMcpCopy} from "../../hub/mcpCopy";
import {aliasOf, fillValues, hostOf, installPlan, labelOfValue, prettyName, prettyPublisher, requiredValues, type InstallPlan, type RegistryEntry} from "../../hub/mcpMarket";
import {useClient} from "../../rpc/context";
import type {CatalogEntry} from "../../rpc/types";
import {useAppStore} from "../../state/store";
import {McpInstaller} from "../McpInstaller";

type Page = {entries: RegistryEntry[]; nextCursor: string | null; source: string};
type Review = {servers: {plan: {config: {name: string; command?: string; args?: string[]; url?: string}; reviewHash: string}; existing: {id: number; hash: string} | null}[]};
type Probe = {connected: boolean; needsLogin?: boolean; error?: string; tools?: unknown[]};

/** The MCP marketplace: search, one button to install, and a short sheet that says what will happen. */
export function McpMarketPage({onManage}: {onManage?: () => void}) {
  const t = useMcpCopy();
  const client = useClient();
  const [query, setQuery] = useState("");
  const [onlyReady, setOnlyReady] = useState(true);
  const [page, setPage] = useState<Page | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [installed, setInstalled] = useState<Set<string>>(new Set());
  const [sheet, setSheet] = useState<RegistryEntry | null>(null);
  const [own, setOwn] = useState(false);
  const generation = useRef(0);

  const loadInstalled = useCallback(async () => {
    try {
      const list = await client.call<CatalogEntry[]>("catalog.list", {});
      useAppStore.setState({catalog: list});
      setInstalled(new Set(list.filter((entry) => entry.kind === "mcp_server").map((entry) => entry.name)));
    } catch { /* the marketplace still works without it */ }
  }, [client]);

  const load = useCallback(async (text: string, cursor?: string) => {
    const mine = ++generation.current;
    setLoading(true); setError(null);
    try {
      const result = await client.call<Page>("registry.mcp", {query: text, limit: 30, ...(cursor ? {cursor} : {})});
      if (generation.current !== mine) return;
      setPage((old) => {
        const entries = cursor && old ? [...old.entries, ...result.entries] : result.entries;
        const seen = new Set<string>();
        return {...result, entries: entries.filter((e) => (seen.has(e.id) ? false : (seen.add(e.id), true)))};
      });
    } catch { if (generation.current === mine) setError(t.loadError); }
    finally { if (generation.current === mine) setLoading(false); }
  }, [client, t]);

  useEffect(() => { void loadInstalled(); }, [loadInstalled]);
  // Typing searches by itself, a moment after the last key.
  useEffect(() => {
    const timer = setTimeout(() => void load(query.trim()), query ? 350 : 0);
    return () => clearTimeout(timer);
  }, [query, load]);

  const shown = useMemo(() => (page?.entries ?? []).filter((e) => !onlyReady || installPlan(e) !== null), [page, onlyReady]);

  return (
    <section className="mcp-market">
      <header className="mcp-market-head">
        <h3>{t.title}</h3>
        <p className="help">{t.intro}</p>
      </header>
      <div className="mcp-market-tools">
        <label className="mcp-market-search">
          <Search size={15} aria-hidden="true" />
          <input type="search" aria-label={t.search} placeholder={t.searchPlaceholder} value={query} maxLength={200} onChange={(e) => setQuery(e.target.value)} />
        </label>
        <label className="mcp-market-check"><input type="checkbox" checked={onlyReady} onChange={(e) => setOnlyReady(e.target.checked)} />{t.onlyReady}</label>
      </div>
      {error ? <div role="alert" className="mcp-market-error"><span>{error}</span><button type="button" onClick={() => void load(query.trim())}>{t.retry}</button></div> : null}
      {loading && !page ? <p role="status" className="help">{t.loading}</p> : null}
      {page && !loading && shown.length === 0 ? <p role="status" className="help">{t.none}</p> : null}
      <ul className="mcp-market-grid">
        {shown.map((entry) => {
          const plan = installPlan(entry);
          const name = prettyName(entry);
          const isInstalled = installed.has(aliasOf(entry));
          const needsKey = plan !== null && requiredNames(entry, plan).length > 0;
          return (
            <li key={entry.id} className="mcp-card">
              <div className="mcp-card-head">
                <span className="mcp-tile" aria-hidden="true">{[...name][0]?.toUpperCase()}</span>
                <div><b>{name}</b><small>{t.by(prettyPublisher(entry))}</small></div>
              </div>
              <p className="mcp-card-text">{entry.description}</p>
              <div className="mcp-badges">
                {plan?.method === "remote" ? <span>{t.online}</span> : plan ? <span>{t.onComputer}</span> : null}
                {needsKey ? <span>{t.needsKey}</span> : null}
              </div>
              <div className="mcp-card-foot">
                {isInstalled ? <span className="mcp-installed"><Check size={14} aria-hidden="true" />{t.installed}</span> : null}
                {plan ? (
                  <button type="button" className={isInstalled ? undefined : "primary"} onClick={() => setSheet(entry)}>
                    {isInstalled ? <Wrench size={13} aria-hidden="true" /> : <Download size={13} aria-hidden="true" />}{isInstalled ? t.manage : t.install}
                  </button>
                ) : <small title={t.manualWhy}>{t.manual}</small>}
              </div>
            </li>
          );
        })}
      </ul>
      {page?.nextCursor ? <button type="button" className="mcp-more" disabled={loading} onClick={() => void load(query.trim(), page.nextCursor!)}>{t.loadMore}</button> : null}
      <div className="mcp-own">
        <button type="button" onClick={() => setOwn((v) => !v)}><Plus size={13} aria-hidden="true" />{t.addOwn}</button>
        <small className="help">{t.addOwnHelp}</small>
      </div>
      {own ? <McpInstaller onSaved={() => void loadInstalled()} onCancel={() => setOwn(false)} /> : null}
      {sheet ? (
        <InstallSheet key={sheet.id} entry={sheet} alreadyInstalled={installed.has(aliasOf(sheet))}
          onClose={() => setSheet(null)} onInstalled={() => void loadInstalled()} onManage={onManage} />
      ) : null}
    </section>
  );
}

/** Which values an entry asks for, read from the registry data without fetching anything. */
function requiredNames(entry: RegistryEntry, plan: InstallPlan): string[] {
  const list = plan.method === "remote"
    ? ((entry.details.remotes as {headers?: {name?: string; isRequired?: boolean}[]}[] | undefined)?.[plan.index]?.headers ?? [])
    : ((entry.details.packages as {environmentVariables?: {name?: string; isRequired?: boolean}[]}[] | undefined)?.[plan.index]?.environmentVariables ?? []);
  return list.filter((v) => v.isRequired && v.name).map((v) => v.name as string);
}

type Phase = "loading" | "review" | "working" | "done" | "error";

function InstallSheet({entry, alreadyInstalled, onClose, onInstalled, onManage}: {entry: RegistryEntry; alreadyInstalled: boolean; onClose: () => void; onInstalled: () => void; onManage?: () => void}) {
  const t = useMcpCopy();
  const client = useClient();
  const plan = installPlan(entry) as InstallPlan;
  const alias = aliasOf(entry);
  const [phase, setPhase] = useState<Phase>("loading");
  const [document, setDocument] = useState("");
  const [values, setValues] = useState<Record<string, string>>({});
  const [step, setStep] = useState("");
  const [message, setMessage] = useState("");
  const [needsLogin, setNeedsLogin] = useState<number | null>(null);
  const names = useMemo(() => requiredValues(document), [document]);

  useEffect(() => {
    let live = true;
    client.call<{document: string}>("mcp.registry_config", {name: entry.id, version: entry.version, alias, method: plan.method, index: plan.index})
      .then((result) => { if (live) { setDocument(result.document); setPhase("review"); } })
      .catch((caught: Error) => { if (live) { setMessage(t.failed(caught.message)); setPhase("error"); } });
    return () => { live = false; };
  }, [client, entry, alias, plan.method, plan.index, t]);

  const install = async () => {
    setPhase("working"); setMessage("");
    try {
      const filled = fillValues(document, values);
      setStep(t.saving);
      const review = await client.call<Review>("mcp.preview", {document: filled, projectId: null});
      const server = review.servers[0];
      const saved = await client.call<{id: number; hash: string}>("mcp.install", {
        document: filled, projectId: null, source: {name: entry.id, version: entry.version, method: plan.method, index: plan.index},
        reviewHash: server.plan.reviewHash, ...(server.existing ? {replaceHash: server.existing.hash} : {}),
      });
      const list = await client.call<CatalogEntry[]>("catalog.list", {});
      const fresh = list.find((e) => e.id === saved.id);
      if (fresh && fresh.runs_code.length && fresh.approved_hash !== fresh.content_hash) {
        setStep(t.approving);
        await client.call("catalog.approve", {id: saved.id, expected_hash: fresh.content_hash});
      }
      setStep(t.enabling);
      await client.call("catalog.set_enabled", {id: saved.id, enabled: true});
      onInstalled();
      setStep(t.testing);
      try {
        const probe = await client.call<Probe>("mcp.test", {id: saved.id, projectId: null});
        if (probe.needsLogin) { setNeedsLogin(saved.id); setMessage(t.signInNeeded); }
        else if (probe.connected) setMessage(probe.tools?.length ? t.ready(probe.tools.length) : t.readyNoTools);
        else setMessage(`${t.testFailed(probe.error ?? "")} ${t.fixLater}`);
      } catch (caught) { setMessage(`${t.testFailed((caught as Error).message)} ${t.fixLater}`); }
      setPhase("done");
    } catch (caught) {
      setMessage(t.failed((caught as Error).message)); setPhase("error");
    }
  };

  const filled = names.every((n) => (values[n] ?? "").trim() !== "");
  const technical = useMemo(() => { try { return JSON.stringify(JSON.parse(document), null, 2); } catch { return document; } }, [document]);
  const name = prettyName(entry);

  return (
    <div className="hub-dialog" role="dialog" aria-modal="true" aria-label={t.sheetTitle(name)}>
      <div className="hub-dialog-box mcp-sheet">
        <h2>{t.sheetTitle(name)}</h2>
        <small className="help">{t.by(prettyPublisher(entry))}</small>
        {phase === "loading" ? <p role="status">{t.preparing}</p> : null}
        {phase === "review" || phase === "working" ? (
          <>
            <section aria-label={t.whatItDoes}>
              <h3>{t.whatItDoes}</h3>
              <p>{plan.method === "remote" ? t.remoteWhat(hostOf(plan.target)) : t.packageWhat(plan.target)}</p>
              {plan.method === "package" ? <p className="help">{t.needsNode}</p> : null}
              {alreadyInstalled ? <p className="help">{t.already}</p> : null}
            </section>
            {names.length ? (
              <section aria-label={t.valuesTitle}>
                <h3>{t.valuesTitle}</h3>
                <p className="help">{t.valuesHelp}</p>
                {names.map((n) => (
                  <label key={n} className="mcp-field">{labelOfValue(n)}
                    <input type="password" autoComplete="off" value={values[n] ?? ""} disabled={phase === "working"} onChange={(e) => setValues((old) => ({...old, [n]: e.target.value}))} />
                  </label>
                ))}
              </section>
            ) : null}
            <details><summary>{t.technical}</summary><pre>{technical}</pre></details>
            {phase === "working" ? <p role="status">{step}</p> : null}
            <div className="hub-actions">
              <button type="button" onClick={onClose} disabled={phase === "working"}>{t.cancel}</button>
              <button type="button" className="primary" disabled={phase === "working" || !filled} onClick={() => void install()}>{phase === "working" ? t.working : t.installNow}</button>
            </div>
            {!filled ? <small className="help">{t.missing}</small> : null}
          </>
        ) : null}
        {phase === "done" || phase === "error" ? (
          <>
            <p role={phase === "error" ? "alert" : "status"} className={phase === "error" ? "hub-error" : undefined}>{message}</p>
            <div className="hub-actions">
              {needsLogin !== null ? <button type="button" className="primary" onClick={() => void client.call("mcp.login_installed", {id: needsLogin}).then(() => setMessage(t.signInStarted)).catch((c: Error) => setMessage(t.failed(c.message)))}>{t.signIn}</button> : null}
              {phase === "done" && onManage ? <button type="button" onClick={() => { onClose(); onManage(); }}>{t.manage}</button> : null}
              {phase === "error" ? <button type="button" onClick={() => setPhase("review")}>{t.retry}</button> : null}
              <button type="button" className={needsLogin === null && phase === "done" ? "primary" : undefined} onClick={onClose}>{t.done}</button>
            </div>
          </>
        ) : null}
      </div>
    </div>
  );
}
