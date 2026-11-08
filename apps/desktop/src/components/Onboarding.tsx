import { useEffect, useState } from "react";
import { copy } from "../copy";
import { useClient } from "../rpc/context";
import type { DetectedAgents, HardwareReport, LlamaStatus, ModelDownloadNotice, Suggestion } from "../rpc/types";
import { openSession, refreshSessions, useAppStore } from "../state/store";
import { AgentForm } from "./AgentForm";
import { DependencyDialog } from "./DependencyDialog";
import { Mark } from "./Mark";
import { ImportPage } from "./ImportPage";
import { MarketPage } from "./MarketPage";

const GIB = 1024 ** 3;
const fill = (text: string, values: Record<string, string | number>) =>
  Object.entries(values).reduce((out, [key, value]) => out.replace(`{${key}}`, String(value)), text);

type Step = "computer" | "import" | "model" | "tools" | "ready";

/** One stitch per step, sewn as the person moves on: the app's mark, used as the progress of setup. */
function Stitches({ steps, current }: { steps: Step[]; current: number }) {
  return <ol className="stitches" aria-label={`${current + 1} / ${steps.length}`}>
    {steps.map((step, index) => <li key={step} data-state={index < current ? "done" : index === current ? "current" : "todo"}>
      <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M5 0.6 9.4 5 5 9.4 0.6 5Z" /></svg>
    </li>)}
  </ol>;
}

export function Onboarding({ sources, onDone }: { sources: boolean; onDone: () => void }) {
  const client = useClient();
  const agents = useAppStore((state) => state.agents);
  const steps: Step[] = sources ? ["computer", "import", "model", "tools", "ready"] : ["computer", "model", "tools", "ready"];
  const [index, setIndex] = useState(0);
  const [hardware, setHardware] = useState<HardwareReport | null>(null);
  const [llama, setLlama] = useState<LlamaStatus | null>(null);
  const [detected, setDetected] = useState<DetectedAgents | null>(null);
  const [suggestion, setSuggestion] = useState<Suggestion | null | undefined>(undefined);
  const [choice, setChoice] = useState<"local" | "online">("local");
  const [installing, setInstalling] = useState(false);
  const [download, setDownload] = useState<ModelDownloadNotice | null>(null);
  const [agent, setAgent] = useState<string>("");
  const [error, setError] = useState<string | null>(null);
  const [importing, setImporting] = useState(false);
  const [confirmDownload, setConfirmDownload] = useState(false);
  const step = steps[index];
  useEffect(() => client.onStatus((status) => { if (status === "connected") void refreshAgents().catch(() => undefined); }), [client]);

  const refreshAgents = async () => {
    const list = await client.call<string[]>("agents.list", {});
    useAppStore.setState({ agents: list });
    if (list.length && !agent) setAgent(list[0]);
  };

  useEffect(() => {
    let live = true;
    client.call<HardwareReport>("hardware.get", {}).then((report) => { if (live) setHardware(report); }).catch(() => undefined);
    client.call<LlamaStatus>("llama.status", {}).then((status) => { if (live) setLlama(status); }).catch(() => undefined);
    client.call<DetectedAgents>("agents.detect", {}).then((found) => { if (live) setDetected(found); }).catch(() => undefined);
    client.call<Suggestion[]>("models.suggest", { task: "everyday" })
      .then((list) => { if (live) setSuggestion(list[0] ?? null); })
      .catch(() => { if (live) setSuggestion(null); });
    const off = client.on((notice) => {
      if (notice.method === "model_download") setDownload(notice.params.state === "progress" ? notice.params : null);
    });
    return () => { live = false; off(); };
  }, [client]);

  const next = () => setIndex((current) => Math.min(current + 1, steps.length - 1));
  const finish = async () => {
    try { await client.call("onboarding.done", {}); onDone(); }
    catch (caught) { setError((caught as Error).message); }
  };
  const startConversation = async () => {
    const selected = agent || agents[0];
    if (!selected) return;
    try {
      const project = useAppStore.getState().projects[0] ?? await client.call<{ id: number; name: string; path: string | null }>("projects.create", { name: copy.defaultProject, path: null });
      const session = await client.call<{ id: number }>("sessions.create", { project_id: project.id, agent: selected });
      useAppStore.setState({ preferredAgent: selected });
      await refreshSessions(client);
      openSession(client, session.id);
      await finish();
    } catch (caught) { setError((caught as Error).message); }
  };
  const downloadSuggestion = async () => {
    if (!suggestion) return;
    setError(null);
    try {
      await client.call("models.download", { repo: suggestion.repo, variant: suggestion.variant, mmproj: false });
      await refreshAgents();
      next();
    } catch (caught) { setError((caught as Error).message); }
  };
  const gpu = hardware?.gpus.find((candidate) => !candidate.integrated);
  const found = (label: string, present: boolean) => <li data-found={present}>
    <span>{label}</span><span className="help">{present ? copy.onboarding.found : copy.onboarding.missing}</span>
  </li>;

  return <div className="onboarding" role="dialog" aria-modal="true" aria-labelledby="onboarding-title">
    <div className="onboarding-column">
      <Mark size={40} stitching={installing || download !== null} />
      <h1 id="onboarding-title">{copy.onboarding.welcome}</h1>
      <p className="help">{copy.onboarding.welcomeBody}</p>
      <Stitches steps={steps} current={index} />

      <section className="onboarding-step" aria-live="polite">
        {step === "computer" ? <>
          <h2>{copy.onboarding.stepComputer}</h2>
          <p className="help">{copy.onboarding.computerBody}</p>
          <ul className="found-list">
            <li><span>{gpu ? gpu.name : copy.models.noGpu}</span><span className="help">{hardware ? `${(hardware.ram.total / GIB).toFixed(0)} GB RAM` : "…"}</span></li>
            {found("Claude Code", Boolean(detected?.claude))}
            {found("Codex", Boolean(detected?.codex))}
            {found("llama.cpp", Boolean(llama?.installed))}
          </ul>
          {llama && !llama.installed ? <button type="button" onClick={() => setInstalling(true)}>{fill(copy.deps.title, { name: "llama.cpp" }).replace("?", "")}</button> : null}
        </> : null}

        {step === "import" ? <>
          <h2>{copy.onboarding.stepImport}</h2>
          <p className="help">{copy.onboarding.importBody}</p>
          <button type="button" className="primary" onClick={() => setImporting(!importing)}>{copy.onboarding.openImport}</button>
          {importing ? <ImportPage /> : null}
        </> : null}

        {step === "model" ? <>
          <h2>{copy.onboarding.stepModel}</h2>
          <p className="help">{copy.onboarding.modelBody}</p>
          <div className="kind-choice" role="radiogroup" aria-label={copy.onboarding.stepModel}>
            <button type="button" role="radio" aria-checked={choice === "local"} onClick={() => setChoice("local")}><b>{copy.onboarding.local}</b><span>{copy.onboarding.localHelp}</span></button>
            <button type="button" role="radio" aria-checked={choice === "online"} onClick={() => setChoice("online")}><b>{copy.onboarding.online}</b><span>{copy.onboarding.onlineHelp}</span></button>
          </div>
          {choice === "local" && llama && !llama.installed ? <button type="button" className="primary" onClick={() => setInstalling(true)}>{copy.modelSetup.installLlama}</button> : null}
          {choice === "local" ? <div className="suggestion">
            {suggestion === undefined ? <p role="status">…</p> : suggestion === null ? <p className="help">{copy.onboarding.noSuggestion}</p> : <>
              <b>{suggestion.family}, {(suggestion.size / GIB).toFixed(1)} GB</b>
              <p className="help">{suggestion.note} {copy.models.fit[suggestion.estimate.fit]}</p>
              {download
                  ? <p role="status">{fill(copy.onboarding.downloading, { name: suggestion.family })} {download.progress ? `${Math.round((100 * download.progress.bytes) / download.progress.total)}%` : ""}</p>
                  : <button type="button" className="primary" onClick={() => setConfirmDownload(true)}>{copy.models.download}</button>}
              {confirmDownload ? <div role="alertdialog" aria-label={copy.models.download}>
                <p>{fill(copy.models.downloadBody, { name: `${suggestion.repo} ${suggestion.variant}`, size: `${(suggestion.size / GIB).toFixed(1)} GB`, free: "" })}</p>
                <button type="button" onClick={() => { setConfirmDownload(false); void downloadSuggestion(); }}>{copy.models.confirm}</button>
                <button type="button" onClick={() => setConfirmDownload(false)}>{copy.models.cancel}</button>
              </div> : null}
            </>}
          </div> : <AgentForm kinds={["model"]} onSaved={() => {
            // The service reads new agents when it starts again; nothing is running yet during setup.
            client.call("service.shutdown", {}).catch(() => undefined);
            next();
          }} />}
        </> : null}

        {step === "tools" ? <>
          <h2>{copy.onboarding.stepTools}</h2>
          <p className="help">{copy.onboarding.toolsBody}</p>
          <MarketPage />
        </> : null}

        {step === "ready" ? <>
          <h2>{copy.onboarding.stepReady}</h2>
          <p className="help">{copy.onboarding.readyBody}</p>
          {agents.length ? <select aria-label={copy.agents.tab} value={agent || agents[0]} onChange={(event) => setAgent(event.target.value)}>
            {agents.map((name) => <option key={name} value={name}>{name}</option>)}
          </select> : null}
          <button type="button" className="primary" disabled={!agents.length} onClick={() => void startConversation()}>{copy.onboarding.start}</button>
        </> : null}
        {error ? <p role="alert">{error}</p> : null}
      </section>

      <nav className="onboarding-nav">
        {index > 0 ? <button type="button" onClick={() => setIndex(index - 1)}>{copy.onboarding.back}</button> : <span />}
        {step === "ready"
          ? <button type="button" className="quiet" onClick={() => void finish()}>{copy.onboarding.skip}</button>
          : <button type="button" className="quiet" onClick={next}>{step === "computer" ? copy.onboarding.next : copy.onboarding.skip}</button>}
      </nav>
    </div>
    {installing ? <DependencyDialog id="ggml.llamacpp" onClose={(done) => {
      setInstalling(false);
      if (done) client.call<LlamaStatus>("llama.status", {}).then(setLlama).catch(() => undefined);
    }} /> : null}
  </div>;
}
