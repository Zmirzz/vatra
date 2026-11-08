import { Cpu, Download, KeyRound, UserRound } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { copy } from "../copy";
import { openExternal } from "../hub/openExternal";
import { useClient } from "../rpc/context";
import type { DetectedAgents, Provider, ProviderModel, SaveResult } from "../rpc/types";
import { useAppStore } from "../state/store";
import { AgentForm, RestartNotice } from "./AgentForm";

type Step = "choose" | "account" | "key" | "done";

function Card({ icon, title, help, onClick }: { icon: ReactNode; title: string; help: string; onClick: () => void }) {
  return (
    <button type="button" className="add-card" onClick={onClick}>
      <span className="add-card-icon" aria-hidden="true">{icon}</span>
      <span><b>{title}</b><small>{help}</small></span>
    </button>
  );
}

/** The way to add a model or an account from the model list: one choice, then only what that choice needs. */
export function AddModelDialog({ onClose }: { onClose: () => void }) {
  const client = useClient();
  const agents = useAppStore((state) => state.agents);
  const [step, setStep] = useState<Step>("choose");
  const [detected, setDetected] = useState<DetectedAgents | null>(null);
  const [result, setResult] = useState<SaveResult | null>(null);
  const [busy, setBusy] = useState<"claude" | "chatgpt" | "key" | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => { if (event.key === "Escape") onClose(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  useEffect(() => {
    let live = true;
    client.call<DetectedAgents>("agents.detect", {}).then((found) => { if (live) setDetected(found); }).catch(() => undefined);
    return () => { live = false; };
  }, [client]);

  const open = (page: "models" | "import") => { useAppStore.setState({ settingsPage: page }); onClose(); };
  const saved = (saved: SaveResult) => { setResult(saved); setStep("done"); setBusy(null); };
  const save = async (form: object, which: "claude" | "chatgpt") => {
    setBusy(which); setError(null);
    try { saved(await client.call<SaveResult>("agents.save", form)); }
    catch (caught) { setError((caught as Error).message); setBusy(null); }
  };

  const claudeReady = Boolean(detected?.claude);
  const chatgptReady = Boolean(detected?.node && detected.acpScripts.length);
  const claudeAdded = agents.includes("claude");
  const chatgptAdded = agents.includes("codex");

  return (
    <div className="dialog-backdrop">
      <div role="dialog" aria-modal="true" aria-labelledby="add-title" className="dialog add-dialog">
        <h2 id="add-title">{step === "account" ? copy.add.whichAccount : step === "done" ? copy.add.doneTitle : copy.add.title}</h2>

        {step === "choose" ? (
          <>
            <p className="help">{copy.add.intro}</p>
            <div className="add-cards">
              <Card icon={<UserRound size={18} />} title={copy.add.accountCard.title} help={copy.add.accountCard.help} onClick={() => setStep("account")} />
              <Card icon={<KeyRound size={18} />} title={copy.add.keyCard.title} help={copy.add.keyCard.help} onClick={() => setStep("key")} />
              <Card icon={<Cpu size={18} />} title={copy.add.localCard.title} help={copy.add.localCard.help} onClick={() => open("models")} />
              <Card icon={<Download size={18} />} title={copy.add.bringCard.title} help={copy.add.bringCard.help} onClick={() => open("import")} />
            </div>
          </>
        ) : null}

        {step === "account" ? (
          <>
            <ul className="account-list">
              <li>
                <div><b>{copy.add.claudeName}</b><small>{claudeReady ? copy.add.claudeReady : copy.add.claudeMissing}</small></div>
                <button type="button" className="primary" disabled={!claudeReady || claudeAdded || busy !== null} onClick={() => detected?.claude && void save({ kind: "claude", binary: detected.claude }, "claude")}>
                  {claudeAdded ? copy.add.alreadyAdded : busy === "claude" ? copy.add.adding : copy.add.useAccount}
                </button>
              </li>
              <li>
                <div><b>{copy.add.chatgptName}</b><small>{chatgptReady ? copy.add.chatgptReady : copy.add.chatgptMissing}</small></div>
                <button type="button" className="primary" disabled={!chatgptReady || chatgptAdded || busy !== null} onClick={() => detected?.node && void save({ kind: "acp", name: "codex", node: detected.node, script: detected.acpScripts[0][1], subscription: true, managedAccount: false, codexBinary: null }, "chatgpt")}>
                  {chatgptAdded ? copy.add.alreadyAdded : busy === "chatgpt" ? copy.add.adding : copy.add.useAccount}
                </button>
              </li>
            </ul>
            {error ? <p role="alert">{error}</p> : null}
            <details>
              <summary>{copy.add.moreOptions}</summary>
              <AgentForm kinds={["claude", "acp"]} onSaved={saved} />
            </details>
          </>
        ) : null}

        {step === "key" ? <KeyServiceForm onSaved={saved} /> : null}

        {step === "done" ? (
          <>
            <p className="help">{copy.add.doneBody}</p>
            {result ? <RestartNotice runningTurns={result.runningTurns} onLater={onClose} /> : null}
          </>
        ) : null}

        <div className="dialog-actions">
          {step === "account" || step === "key" ? <button type="button" className="quiet" onClick={() => { setStep("choose"); setError(null); }}>{copy.add.back}</button> : null}
          <button type="button" className="quiet" onClick={onClose}>{copy.add.close}</button>
        </div>
      </div>
    </div>
  );
}

/** A service with a key: the service, the key, and a model already picked. The address and name wait under Details. */
function KeyServiceForm({ onSaved }: { onSaved: (result: SaveResult) => void }) {
  const client = useClient();
  const [providers, setProviders] = useState<Provider[]>([]);
  const [provider, setProvider] = useState("openrouter");
  const [models, setModels] = useState<ProviderModel[]>([]);
  const [model, setModel] = useState("");
  const [key, setKey] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    client.call<Provider[]>("agents.providers", {}).then((list) => { if (live) setProviders(list); }).catch(() => undefined);
    return () => { live = false; };
  }, [client]);
  const chosen = providers.find((candidate) => candidate.id === provider);
  useEffect(() => {
    setModels([]); setModel("");
    if (!chosen?.modelsUrl) return;
    let live = true;
    client.call<ProviderModel[]>("agents.provider_models", { provider: chosen.id }).then((list) => {
      if (!live) return;
      const usable = list.filter((item) => item.tools).sort((a, b) => Number(b.free) - Number(a.free) || a.name.localeCompare(b.name));
      setModels(usable);
      setModel(usable[0]?.id ?? "");
    }).catch(() => undefined);
    return () => { live = false; };
  }, [client, chosen]);

  const copilot = chosen?.driver === "copilot";
  useEffect(() => {
    if (!copilot || !key) return;
    let live = true;
    client.call<ProviderModel[]>("copilot.models", { key }).then((list) => {
      if (!live) return;
      const usable = list.filter((item) => item.tools);
      setModels(usable);
      setModel(usable[0]?.id ?? "");
    }).catch((caught: Error) => { if (live) setError(caught.message); });
    return () => { live = false; };
  }, [client, copilot, key]);

  const needsAddress = Boolean(chosen && !chosen.baseUrl && chosen.driver !== "anthropic");
  const ready = Boolean(model.trim() && (!needsAddress || baseUrl.trim()) && (!copilot || key));
  const submit = async () => {
    setBusy(true); setError(null);
    try {
      onSaved(await client.call<SaveResult>("agents.save", { kind: "model", name: name.trim() || provider, provider, baseUrl: chosen?.baseUrl ? undefined : baseUrl || undefined, model, key: key || undefined }));
      setKey("");
    } catch (caught) { setError((caught as Error).message); setBusy(false); }
  };

  return (
    <form className="agent-form" onSubmit={(event) => { event.preventDefault(); void submit(); }}>
      <label>{copy.add.service}
        <select value={provider} onChange={(event) => setProvider(event.target.value)}>
          {providers.map((item) => <option key={item.id} value={item.id}>{item.id === "custom" ? copy.add.otherService : item.name}</option>)}
        </select>
      </label>
      {copilot ? <CopilotSignIn signedIn={Boolean(key)} onToken={setKey} /> : <label>{copy.add.key}
        <input type="password" aria-label={copy.add.key} autoComplete="new-password" value={key} onChange={(event) => setKey(event.target.value)} />
        <small>{copy.add.keyHelp} {chosen?.keyUrl ? <a href={chosen.keyUrl} target="_blank" rel="noreferrer">{copy.add.getKey}</a> : null}</small>
      </label>}
      <label>{copy.add.model}
        <input list="add-provider-models" aria-label={copy.add.model} value={model} onChange={(event) => setModel(event.target.value)} />
        <datalist id="add-provider-models">{models.slice(0, 400).map((item) => <option key={item.id} value={item.id}>{item.free ? `${item.name} (${copy.picker.tagFree})` : item.name}</option>)}</datalist>
        {models.length ? <small>{copy.add.modelHelp}</small> : null}
      </label>
      <details>
        <summary>{copy.add.details}</summary>
        {needsAddress ? <label>{copy.add.address}<input value={baseUrl} placeholder="https://…/v1" onChange={(event) => setBaseUrl(event.target.value)} /><small>{copy.add.addressHelp}</small></label> : null}
        <label>{copy.add.name}<input value={name} placeholder={provider} onChange={(event) => setName(event.target.value)} /><small>{copy.add.nameHelp}</small></label>
      </details>
      {error ? <p role="alert">{error}</p> : null}
      <div className="dialog-actions">
        <button type="submit" className="primary" disabled={!ready || busy}>{busy ? copy.add.adding : copy.add.addModel}</button>
      </div>
    </form>
  );
}

/** Signing in to GitHub Copilot: a short code to type on github.com, then the token comes back on its own. */
function CopilotSignIn({ signedIn, onToken }: { signedIn: boolean; onToken: (token: string) => void }) {
  const client = useClient();
  const [code, setCode] = useState<{ deviceCode: string; userCode: string; verificationUri: string; interval: number } | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!code) return;
    let live = true;
    let timer: ReturnType<typeof setTimeout>;
    const ask = async (wait: number) => {
      timer = setTimeout(async () => {
        try {
          const answer = await client.call<{ state: string; token?: string; interval?: number }>("copilot.poll", { deviceCode: code.deviceCode });
          if (!live) return;
          if (answer.state === "waiting") void ask((answer.interval ?? wait) * 1000);
          else if (answer.state === "done" && answer.token) { onToken(answer.token); setCode(null); setMessage(copy.add.copilotDone); }
          else { setCode(null); setMessage(answer.state === "denied" ? copy.add.copilotDenied : copy.add.copilotExpired); }
        } catch (caught) { if (live) { setCode(null); setMessage((caught as Error).message); } }
      }, wait);
    };
    void ask(code.interval * 1000);
    return () => { live = false; clearTimeout(timer); };
  }, [client, code, onToken]);

  const start = async () => {
    setBusy(true); setMessage(null);
    try {
      const started = await client.call<{ deviceCode: string; userCode: string; verificationUri: string; interval: number }>("copilot.start", {});
      setCode(started);
      void openExternal(started.verificationUri).catch(() => undefined);
    } catch (caught) { setMessage((caught as Error).message); }
    setBusy(false);
  };

  return (
    <div className="copilot-signin">
      <small>{copy.add.copilotHelp}</small>
      {code ? (
        <>
          <p><span>{copy.add.copilotCode}</span> <b className="copilot-code" aria-live="polite">{code.userCode}</b></p>
          <p className="help">{copy.add.copilotWait} <a href={code.verificationUri} target="_blank" rel="noreferrer">{copy.add.copilotOpen}</a></p>
        </>
      ) : (
        <button type="button" className={signedIn ? "quiet" : "primary"} disabled={busy} onClick={() => void start()}>{signedIn ? copy.add.copilotAgain : copy.add.copilotSignIn}</button>
      )}
      {signedIn && !code ? <p className="help">{copy.add.copilotDone}</p> : message ? <p role="alert">{message}</p> : null}
    </div>
  );
}
