import { useEffect, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { copy } from "../copy";
import { useClient } from "../rpc/context";
import type {
  CheckStep, Estimate, HardwareReport, InstalledModel, LabelView, LlamaStatus, ModelDownloadNotice, RepoSummary, RepoView, Suggestion, Task,
} from "../rpc/types";
import { useAppStore } from "../state/store";
import { DependencyDialog } from "./DependencyDialog";
import { ParityResults } from "./ParityResults";
import { HuggingFaceToken } from "./HuggingFaceToken";

const GIB = 1024 ** 3;
const gib = (bytes: number) => (bytes / GIB).toFixed(1);
const fill = (text: string, values: Record<string, string | number>) =>
  Object.entries(values).reduce((out, [key, value]) => out.replace(`{${key}}`, String(value)), text);
const TASKS: Task[] = ["everyday", "coding", "longDocuments", "vision"];

type Pending = { title?: string; body: string; run: () => Promise<void> };

function Fit({ estimate }: { estimate: Estimate }) {
  return <p className="help">
    <span className="tag">{copy.models.estimate}</span> {copy.models.fit[estimate.fit]}{" "}
    {fill(copy.models.needs, { gib: gib(estimate.needed), ctx: estimate.ctx })}
    {estimate.notes.includes("moeSplitFaster") ? ` ${copy.models.moeSplit}` : ""}
  </p>;
}

function LabelLine({ view }: { view: LabelView }) {
  const date = view.checkedAt ? new Date(view.checkedAt * 1000).toLocaleDateString() : "";
  const reasons = view.reasons.filter((reason) => reason !== "notEnoughMemory" || view.estimated);
  return <div className="model-label">
    <p><b data-label={view.label}>{copy.models.label[view.label]}</b>{" · "}
      {view.estimated
        ? <span className="tag" title={copy.models.estimateHelp}>{copy.models.estimate}</span>
        : <span>{fill(copy.models.checkedOn, { date })}{view.llamaBuild ? ` ${fill(copy.models.withBuild, { build: view.llamaBuild })}` : ""}</span>}
    </p>
    <p className="help">{(view.estimated ? copy.models.estimateLabelHelp : copy.models.labelHelp)[view.label]}</p>
    {reasons.length ? <ul className="help">{reasons.map((reason) => <li key={reason}>{copy.models.reason[reason]}</li>)}</ul> : null}
    {view.results?.speed?.genTps ? <p className="help">{fill(copy.models.speed, { tps: view.results.speed.genTps.toFixed(1) })}</p> : null}
    {!view.estimated ? <p className="help">{copy.models.limits}</p> : null}
    {view.stale ? <p className="help">{copy.models.stale}</p> : null}
  </div>;
}

export function ModelsPage() {
  const client = useClient();
  const agents = useAppStore((state) => state.agents);
  const [hardware, setHardware] = useState<HardwareReport | null>(null);
  const [llama, setLlama] = useState<LlamaStatus | null>(null);
  const [installed, setInstalled] = useState<InstalledModel[]>([]);
  const [labels, setLabels] = useState<LabelView[]>([]);
  const [task, setTask] = useState<Task>("everyday");
  const [suggestions, setSuggestions] = useState<Suggestion[] | null>(null);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<RepoSummary[] | null>(null);
  const [repo, setRepo] = useState<RepoView | null>(null);
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [checking, setChecking] = useState<{ agent: string; step: CheckStep } | null>(null);
  const [download, setDownload] = useState<ModelDownloadNotice | null>(null);
  const [withVision, setWithVision] = useState(false);
  const [pending, setPending] = useState<Pending | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [installing, setInstalling] = useState(false);
  const [hasToken, setHasToken] = useState(false);

  const refresh = async () => {
    const [list, views] = await Promise.all([
      client.call<InstalledModel[]>("models.installed", {}),
      client.call<LabelView[]>("models.labels", {}),
    ]);
    setInstalled(list); setLabels(views);
  };
  const run = async (key: string, work: () => Promise<void>) => {
    setBusy(key); setError(null);
    try { await work(); } catch (caught) { setError((caught as Error).message); } finally { setBusy(null); }
  };

  useEffect(() => {
    let live = true;
    Promise.all([client.call<HardwareReport>("hardware.get", {}), client.call<LlamaStatus>("llama.status", {})])
      .then(([report, status]) => { if (live) { setHardware(report); setLlama(status); } })
      .catch((caught) => { if (live) setError(caught.message); });
    refresh().catch((caught) => { if (live) setError(caught.message); });
    const off = client.on((notice) => {
      if (notice.method === "model_download") setDownload(notice.params.state === "progress" ? notice.params : null);
      if (notice.method === "model_check") setChecking(notice.params.step === "done" ? null : { agent: notice.params.agent, step: notice.params.step });
      if (notice.method === "model_server") void refresh().catch(() => undefined);
    });
    return () => { live = false; off(); };
  }, [client]);

  const startDownload = (name: string, repoId: string, variant: string, vision: boolean) =>
    run(name, async () => {
      await client.call("models.download", { repo: repoId, variant, mmproj: vision });
      await refresh();
      useAppStore.setState({ agents: await client.call<string[]>("agents.list", {}) });
    });

  const check = (view: LabelView) => {
    const local = view.agent.startsWith("local:");
    const go = () => run(view.agent, async () => {
      setChecking({ agent: view.agent, step: "load" });
      try { await client.call("models.check", { agent: view.agent, consentCost: !local }); } finally { setChecking(null); }
      await refresh();
    });
    if (local) void go();
    else setPending({ title: copy.models.costTitle, body: fill(copy.models.costBody, { host: view.agent }), run: go });
  };

  const [tab, setTab] = useState<"all" | "recommended" | "browse" | "downloaded">("all");
  const useInChat = (agentName: string) => {
    useAppStore.setState({ preferredAgent: agentName, activeSessionId: null, settingsPage: null });
  };

  const gpu = hardware?.gpus.find((candidate) => !candidate.integrated);
  return <section className="transfer-page models-page"><h3>{copy.models.tab}</h3>
    {error ? <p role="alert">{error}</p> : null}
    {pending ? <div className="confirm" role="alertdialog" aria-label={pending.title ?? copy.models.download}>
      {pending.title ? <b>{pending.title}</b> : null}
      <p>{pending.body}</p>
      <div className="row">
        <button type="button" className="primary" onClick={() => { const next = pending; setPending(null); void next.run(); }}>{copy.models.confirm}</button>
        <button type="button" onClick={() => setPending(null)}>{copy.models.cancel}</button>
      </div>
    </div> : null}

    <div className="model-hub-tabs" role="tablist">
      <button type="button" role="tab" aria-selected={tab === "all"} className={tab === "all" ? "active" : ""} onClick={() => setTab("all")}>{copy.picker.all}</button>
      <button type="button" role="tab" aria-selected={tab === "recommended"} className={tab === "recommended" ? "active" : ""} onClick={() => setTab("recommended")}>{copy.models.tabRecommended}</button>
      <button type="button" role="tab" aria-selected={tab === "browse"} className={tab === "browse" ? "active" : ""} onClick={() => setTab("browse")}>{copy.models.tabBrowse}</button>
      <button type="button" role="tab" aria-selected={tab === "downloaded"} className={tab === "downloaded" ? "active" : ""} onClick={() => setTab("downloaded")}>{copy.models.tabDownloaded} {installed.length > 0 ? `(${installed.length})` : ""}</button>
    </div>

    <section aria-label={copy.models.computer} className="computer-badge"><h4>{copy.models.computer}</h4>
      {hardware ? <p>
        {gpu ? `${gpu.name}: ${gib(gpu.free ?? gpu.total)} GB ${gpu.free != null ? copy.models.free : copy.models.total}` : copy.models.noGpu}
        {` · ${copy.models.ram} ${gib(hardware.ram.available)} / ${gib(hardware.ram.total)} GB`}
      </p> : <p role="status">…</p>}
      {gpu?.source === "system" ? <p className="help">{copy.models.seenByWindows}</p> : null}
      {llama ? llama.installed
        ? <p className="help">{fill(copy.models.llamaFound, { build: llama.install.build ?? "?" })}: {llama.install.caps.fit ? copy.models.llamaFit : copy.models.llamaNoFit}</p>
        : <div><p>{copy.models.llamaMissing}</p><pre>{llama.installCommand}</pre><button type="button" onClick={() => setInstalling(true)}>{copy.modelSetup.installLlama}</button></div> : null}
      {hardware ? <details><summary>{copy.details}</summary>
        <p className="help">{copy.models.cpu}: {hardware.cpu.name ?? "?"} ({hardware.cpu.cores ?? "?"} / {hardware.cpu.threads ?? "?"})</p>
        {hardware.gpus.map((card) => <p className="help" key={card.name + card.backend}>{card.backend ?? copy.models.gpu}: {card.name}, {gib(card.total)} GB</p>)}
        {hardware.diskFree != null ? <p className="help">{copy.models.disk}: {gib(hardware.diskFree)} GB</p> : null}
      </details> : null}
    </section>

    {download ? <p role="status" className="download-progress-banner">{fill(copy.models.downloading, { file: download.progress?.file ?? "" })} {download.progress ? `${gib(download.progress.bytes)} / ${gib(download.progress.total)} GB` : ""}
      <button type="button" onClick={() => void client.call("models.download_cancel", {})}>{copy.models.cancelDownload}</button></p> : null}

    {(tab === "all" || tab === "downloaded") ? (
      <section aria-label={copy.models.yours}><h4>{copy.models.yours}</h4>
        {labels.length === 0 ? <p className="help">{copy.models.noneYet}</p> : null}
        {labels.map((view) => {
          const model = installed.find((candidate) => candidate.agent === view.agent);
          return <article className="extension-card model-card" key={view.agent}>
            <div className="model-card-header">
              <div className="model-title-row">
                <b>{view.agent}</b>
                {model?.running ? <span className="tag running-tag">{copy.models.running}</span> : null}
              </div>
              <button type="button" className="primary use-chat-btn" onClick={() => useInChat(view.agent)}>{copy.models.useInChat}</button>
            </div>
            {model?.missing ? <p role="alert">{copy.models.missingFile}</p> : null}
            <LabelLine view={view} />
            {checking?.agent === view.agent ? <p role="status">{copy.models.checking} {copy.models.steps[checking.step]}</p> : null}
            <div className="row">
              <button type="button" disabled={busy !== null} onClick={() => check(view)}>{copy.models.check}</button>
              {model ? model.running
                ? <button type="button" disabled={busy !== null} onClick={() => void run(view.agent, async () => { await client.call("models.stop", {}); await refresh(); })}>{copy.models.stop}</button>
                : <button type="button" disabled={busy !== null || model.missing} onClick={() => void run(view.agent, async () => { await client.call("models.start", { slug: model.slug }); await refresh(); })}>{copy.models.start}</button> : null}
              {model ? <button type="button" disabled={busy !== null} onClick={() => setPending({ body: fill(copy.models.removeBody, { name: view.agent }), run: () => run(view.agent, async () => {
                await client.call("models.remove", { slug: model.slug }); await refresh();
                useAppStore.setState({ agents: await client.call<string[]>("agents.list", {}) });
              }) })}>{copy.models.remove}</button> : null}
            </div>
            <ParityResults agent={view.agent} />
          </article>;
        })}
        {agents.filter((agent) => !labels.some((view) => view.agent === agent)).map((agent) => <article className="extension-card model-card" key={agent}><b>{agent}</b><ParityResults agent={agent} engine={false} /></article>)}
      </section>
    ) : null}

    {(tab === "all" || tab === "recommended") ? (
      <section aria-label={copy.models.suggested}><h4>{copy.models.suggested}</h4>
        <div className="row">
          <label>{copy.models.task} <select value={task} onChange={(event) => { setTask(event.target.value as Task); setSuggestions(null); }}>
            {TASKS.map((name) => <option key={name} value={name}>{copy.models.tasks[name]}</option>)}
          </select></label>
          <button type="button" disabled={busy !== null} onClick={() => void run("suggest", async () => setSuggestions(await client.call<Suggestion[]>("models.suggest", { task })))}>{copy.models.show}</button>
        </div>
        {suggestions ? <p className="help">{copy.models.pickedBy}</p> : null}
        {suggestions?.map((item) => {
          const mine = download?.repo === item.repo && download?.variant === item.variant;
          const getting = busy === item.repo || mine;
          return (
            <article className="extension-card model-card" key={item.repo}>
              <div className="model-card-header">
                <b>{item.family} · {item.variant} · {gib(item.size)} GB</b>
                {getting ? (
                  <div className="download-inline">
                    {mine && download.progress ? `${gib(download.progress.bytes)} / ${gib(download.progress.total)} GB` : copy.models.downloading}
                    <button type="button" onClick={() => void client.call("models.download_cancel", {})}>{copy.models.cancelDownload}</button>
                  </div>
                ) : (
                  <button type="button" className="primary download-btn" disabled={busy !== null || download !== null} onClick={() => startDownload(item.repo, item.repo, item.variant, task === "vision")}>{copy.models.download}</button>
                )}
              </div>
              <small className="help">{item.repo}</small>
              <p className="help">{item.note}</p>
              <Fit estimate={item.estimate} />
              {item.lowQuality ? <p className="help">{copy.models.lowQuality}</p> : null}
              {item.unlockVramGib ? <p className="help">{fill(copy.models.unlock, { gib: item.unlockVramGib })}</p> : null}
            </article>
          );
        })}
      </section>
    ) : null}

    {(tab === "all" || tab === "browse") ? (
      <>
        <section aria-label={copy.models.search}><h4>{copy.models.search}</h4>
          <HuggingFaceToken onChanged={setHasToken} />
          <form className="row" onSubmit={(event) => { event.preventDefault(); void run("search", async () => { setRepo(null); setResults(await client.call<RepoSummary[]>("models.search", { query })); }); }}>
            <input aria-label={copy.models.search} placeholder={copy.models.searchPlaceholder} value={query} onChange={(event) => setQuery(event.target.value)} />
            <button type="submit" disabled={!query.trim() || busy !== null}>{copy.models.searchButton}</button>
          </form>
          {repo ? <article className="extension-card model-card">
            <b>{repo.repo}</b>
            {repo.gated ? <p className="help">{copy.models.gated}</p> : null}
            <LabelLine view={{ agent: repo.repo, label: repo.label, reasons: repo.reasons, estimated: true, checkedAt: null, llamaBuild: null, stale: false, results: null }} />
            {repo.variants.some((variant) => variant.mmproj.length) ? <label><input type="checkbox" checked={withVision} onChange={(event) => setWithVision(event.target.checked)} /> {copy.models.vision}</label> : null}
            <h5>{copy.models.variants}</h5>
            {repo.variants.map((variant) => <div className="variant" key={variant.name}>
              <b>{variant.name} · {gib(variant.size)} GB</b>
              <Fit estimate={variant.estimate} />
              {variant.lowQuality ? <p className="help">{copy.models.lowQuality}</p> : null}
              {(() => {
                const mine = download?.repo === repo.repo && download?.variant === variant.name;
                const getting = busy === repo.repo || mine;
                return getting ? (
                  <div className="download-inline">
                    {mine && download.progress ? `${gib(download.progress.bytes)} / ${gib(download.progress.total)} GB` : copy.models.downloading}
                    <button type="button" onClick={() => void client.call("models.download_cancel", {})}>{copy.models.cancelDownload}</button>
                  </div>
                ) : (
                  <button type="button" disabled={(repo.gated && !hasToken) || busy !== null || download !== null} onClick={() => startDownload(repo.repo, repo.repo, variant.name, withVision)}>{copy.models.download}</button>
                );
              })()}
            </div>)}
          </article> : results?.map((item) => <button type="button" className="repo" key={item.id} disabled={busy !== null} onClick={() => void run(item.id, async () => setRepo(await client.call<RepoView>("models.files", { repo: item.id })))}>
            <b>{item.id}</b> <small className="help">{fill(copy.models.downloads, { count: item.downloads.toLocaleString() })}{item.gated ? ` · ${copy.models.gated}` : ""}</small>
          </button>)}
        </section>

        <section aria-label={copy.models.addFile}><h4>{copy.models.addFile}</h4>
          <p className="help">{copy.models.addFileHelp}</p>
          <form className="row" onSubmit={(event) => { event.preventDefault(); void run("add", async () => {
            await client.call("models.add_path", { path: path.trim() }); setPath(""); await refresh();
            useAppStore.setState({ agents: await client.call<string[]>("agents.list", {}) });
          }); }}>
            <input aria-label={copy.models.addFile} value={path} placeholder={"C:\\models\\model.gguf"} onChange={(event) => setPath(event.target.value)} />
            {isTauri() ? <button type="button" onClick={() => void run("browse", async () => { const selected = await open({ multiple: false, filters: [{ name: "GGUF", extensions: ["gguf"] }] }); if (typeof selected === "string") setPath(selected); })}>{copy.modelSetup.browse}</button> : null}
            <button type="submit" disabled={!path.trim() || busy !== null}>{copy.models.add}</button>
          </form>
        </section>
      </>
    ) : null}
    {installing ? <DependencyDialog id="ggml.llamacpp" onClose={(done) => { setInstalling(false); if (done) client.call<LlamaStatus>("llama.status", {}).then(setLlama).catch((caught) => setError(caught.message)); }} /> : null}
  </section>;
}
