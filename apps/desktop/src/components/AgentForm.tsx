import { useEffect, useMemo, useState } from "react";
import { copy } from "../copy";
import { useClient } from "../rpc/context";
import { usePreferences } from "../settings/preferences";
import type { AgentForm as Form, AgentKind, DetectedAgents, Provider, ProviderModel, SaveResult } from "../rpc/types";

const fill = (text: string, values: Record<string, string | number>) =>
  Object.entries(values).reduce((out, [key, value]) => out.replace(`{${key}}`, String(value)), text);

/** The short form for adding an agent: first what kind, then only the fields that kind needs. */
export function AgentForm({ onSaved, onCancel, kinds = ["model", "claude", "acp"], initial }: {
  onSaved: (result: SaveResult) => void;
  onCancel?: () => void;
  kinds?: AgentKind[];
  initial?: Form;
}) {
  const client = useClient();
  const ro = usePreferences((state) => state.preferences.language === "ro");
  const [kind, setKind] = useState<AgentKind>(initial?.kind ?? kinds[0]);
  const [providers, setProviders] = useState<Provider[]>([]);
  const [detected, setDetected] = useState<DetectedAgents | null>(null);
  const [provider, setProvider] = useState(initial?.kind === "model" ? initial.provider : "openrouter");
  const [models, setModels] = useState<ProviderModel[]>([]);
  const [name, setName] = useState(initial && "name" in initial ? initial.name : "");
  const [baseUrl, setBaseUrl] = useState(initial?.kind === "model" ? initial.baseUrl ?? "" : "");
  const [model, setModel] = useState(initial?.kind === "model" ? initial.model : "");
  const [key, setKey] = useState("");
  const [script, setScript] = useState(initial?.kind === "acp" ? initial.script ?? "" : "");
  const [customCommand, setCustomCommand] = useState(initial?.kind === "acp" && initial.args !== undefined);
  const [binary, setBinary] = useState(initial?.kind === "acp" ? initial.node : "");
  const [argumentsJson, setArgumentsJson] = useState(JSON.stringify(initial?.kind === "acp" ? initial.args ?? [] : [], null, 2));
  const [startupTimeout, setStartupTimeout] = useState(initial?.kind === "acp" ? initial.startupTimeoutSec ?? 30 : 30);
  const [turnTimeout, setTurnTimeout] = useState(initial?.kind === "acp" ? initial.turnTimeoutSec ?? 900 : 900);
  let argumentsArray: string[] | null = null;
  try { const parsed: unknown = JSON.parse(argumentsJson); if (Array.isArray(parsed) && parsed.length <= 128 && parsed.every((item) => typeof item === "string")) argumentsArray = parsed; } catch { /* invalid input keeps saving disabled */ }
  const [subscription, setSubscription] = useState(initial?.kind === "acp" ? initial.subscription : true);
  const [managed, setManaged] = useState(initial?.kind === "acp" ? Boolean(initial.managedAccount) : false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    client.call<Provider[]>("agents.providers", {}).then((list) => { if (live) setProviders(list); }).catch(() => undefined);
    client.call<DetectedAgents>("agents.detect", {}).then((found) => {
      if (!live) return;
      setDetected(found);
      if (!initial && found.acpScripts.length) setScript(found.acpScripts[0][1]);
    }).catch(() => undefined);
    return () => { live = false; };
  }, [client]);

  const chosen = providers.find((candidate) => candidate.id === provider);
  useEffect(() => {
    setModels([]);
    if (!chosen?.modelsUrl) return;
    let live = true;
    client.call<ProviderModel[]>("agents.provider_models", { provider: chosen.id })
      .then((list) => { if (live) setModels(list.filter((item) => item.tools).sort((a, b) => Number(b.free) - Number(a.free) || a.name.localeCompare(b.name))); })
      .catch(() => undefined);
    return () => { live = false; };
  }, [client, chosen]);
  // A name follows the service until the person types one.
  const [nameTouched, setNameTouched] = useState(Boolean(initial));
  const suggestedName = useMemo(() => (kind === "model" ? provider : kind === "acp" ? customCommand ? "external" : "codex" : "claude"), [kind, provider, customCommand]);
  const shownName = nameTouched ? name : suggestedName;

  const form = (): Form | null => {
    if (kind === "model") return { kind, name: shownName, provider, baseUrl: chosen?.baseUrl ? undefined : baseUrl, model, key: key || undefined };
    if (kind === "claude") { const binary = initial?.kind === "claude" ? initial.binary : detected?.claude; return binary ? { kind, binary } : null; }
    const node = initial?.kind === "acp" ? initial.node : detected?.node;
    const codexBinary = initial?.kind === "acp" ? initial.codexBinary ?? detected?.codex : detected?.codex;
    if (managed && !codexBinary) return null;
    if (customCommand) {
      if (!binary.trim() || !argumentsArray || !Number.isInteger(startupTimeout) || startupTimeout < 1 || startupTimeout > 86400 || !Number.isInteger(turnTimeout) || turnTimeout < 1 || turnTimeout > 86400) return null;
      return { kind, name: shownName, node: binary, args: argumentsArray, startupTimeoutSec: startupTimeout, turnTimeoutSec: turnTimeout, subscription: subscription || managed, managedAccount: managed, codexBinary: managed ? codexBinary : null };
    }
    return node && script ? { kind, name: shownName, node, script, subscription: subscription || managed, managedAccount: managed, codexBinary: managed ? codexBinary : null } : null;
  };
  const submit = async () => {
    const value = form();
    if (!value) return;
    setBusy(true); setError(null);
    try { onSaved(await client.call<SaveResult>("agents.save", value)); setKey(""); }
    catch (caught) { setError((caught as Error).message); }
    finally { setBusy(false); }
  };
  const ready = kind === "model" ? Boolean(model.trim() && (chosen?.baseUrl || chosen?.driver === "anthropic" || baseUrl.trim())) : Boolean(form());
  const picked = models.find((item) => item.id === model);

  return <form className="agent-form" onSubmit={(event) => { event.preventDefault(); void submit(); }}>
    {kinds.length > 1 ? <div className="kind-choice" role="radiogroup" aria-label={copy.agents.add}>
      {kinds.map((option) => <button key={option} type="button" role="radio" aria-checked={kind === option} onClick={() => { setKind(option); setError(null); }}>
        <b>{copy.agents.kinds[option]}</b><span>{copy.agents.kindHelp[option]}</span>
      </button>)}
    </div> : null}

    {kind === "model" ? <>
      <label>{copy.agents.provider}<select value={provider} onChange={(event) => { setProvider(event.target.value); setModel(""); }}>
        {providers.map((item) => <option key={item.id} value={item.id}>{item.name}</option>)}
      </select></label>
      {chosen && !chosen.baseUrl && chosen.driver !== "anthropic" ? <label>{copy.agents.address}
        <input value={baseUrl} placeholder="https://…/v1" onChange={(event) => setBaseUrl(event.target.value)} />
        <small>{copy.agents.addressHelp}</small></label> : null}
      <label>{copy.agents.model}
        <input list="provider-models" value={model} placeholder={copy.agents.modelPlaceholder} onChange={(event) => setModel(event.target.value)} />
        <datalist id="provider-models">{models.slice(0, 400).map((item) => <option key={item.id} value={item.id}>{item.free ? `${item.name} (${copy.agents.freeModel})` : item.name}</option>)}</datalist>
        {chosen && !chosen.modelsUrl ? <small>{copy.agents.keyHasNoProvider}</small> : null}
        {picked?.free ? <small>{copy.agents.freeModel}</small> : null}
      </label>
      <label>{copy.agents.key}
        <input type="password" aria-label={copy.agents.key} autoComplete="new-password" value={key} onChange={(event) => setKey(event.target.value)} />
        <small>{initial ? copy.agents.keySaved : copy.agents.keyHelp} {chosen?.keyUrl ? <a href={chosen.keyUrl} target="_blank" rel="noreferrer">{copy.agents.getKey}</a> : null}</small>
      </label>
    </> : null}

    {kind === "claude" ? <p className="help">{detected?.claude ? fill(copy.agents.found, { path: detected.claude }) : copy.agents.notFound}</p> : null}
    {kind === "claude" || (kind === "acp" && subscription) ? <p className="help">{copy.accountNoticeBody}</p> : null}

    {kind === "acp" ? <>
      <label className="check"><input type="checkbox" checked={customCommand} onChange={(event) => setCustomCommand(event.target.checked)} />{ro ? "Comandă ACP personalizată" : "Custom ACP command"}</label>
      {customCommand ? <>
        <label>{ro ? "Executabil" : "Executable"}<input value={binary} onChange={(event) => setBinary(event.target.value)} placeholder="C:\\Tools\\agent.exe" />
          <small>{ro ? "Folosește executabilul real. Pentru adaptoare npm: node.exe și fișierul JavaScript în argumente." : "Use the actual executable. For npm adapters: node.exe with the JavaScript entry point in arguments."}</small>
        </label>
        <label>{ro ? "Argumente (listă JSON)" : "Arguments (JSON array)"}<textarea value={argumentsJson} onChange={(event) => setArgumentsJson(event.target.value)} spellCheck={false} rows={4} /></label>
        {!argumentsArray ? <p role="alert">{ro ? "Introdu o listă JSON de texte." : "Enter a JSON array of strings."}</p> : null}
        <label>{ro ? "Limită pornire (secunde)" : "Startup timeout (seconds)"}<input type="number" min={1} max={86400} value={startupTimeout} onChange={(event) => setStartupTimeout(Number(event.target.value))} /></label>
        <label>{ro ? "Limită tură (secunde)" : "Turn timeout (seconds)"}<input type="number" min={1} max={86400} value={turnTimeout} onChange={(event) => setTurnTimeout(Number(event.target.value))} /></label>
        <p className="help">{ro ? "Nu introduce chei sau tokenuri în argumente. Salvarea nu lansează comanda; autentificarea rămâne la agent." : "Keep keys and tokens out of arguments. Saving does not launch the command; authentication remains with the agent."}</p>
      </> : <>
      {detected && !detected.acpScripts.length ? <p className="help">{copy.agents.noAdapters}</p> : null}
      {detected?.acpScripts.length ? <label>{copy.agents.adapter}<select value={script} onChange={(event) => setScript(event.target.value)}>
        {detected.acpScripts.map(([packageName, path]) => <option key={path} value={path}>{packageName}</option>)}
      </select></label> : null}
      {detected && !detected.node ? <p className="help">{copy.agents.notFound} (Node.js)</p> : null}
      </>}
      <label className="check"><input type="checkbox" checked={subscription || managed} disabled={managed} onChange={(event) => setSubscription(event.target.checked)} /> {copy.agents.subscription}</label>
      <label className="check"><input type="checkbox" checked={managed} onChange={(event) => setManaged(event.target.checked)} /> {copy.agents.managedAccount}</label>
      <p className="help">{managed ? (detected && !detected.codex ? copy.agents.managedNoCodex : copy.agents.managedOn) : copy.agents.managedOff}</p>
    </> : null}

    {kind !== "claude" ? <details><summary>{copy.agents.name}</summary>
      <label>{copy.agents.name}<input value={shownName} disabled={Boolean(initial)} onChange={(event) => { setNameTouched(true); setName(event.target.value); }} /><small>{copy.agents.nameHelp}</small></label>
    </details> : null}

    {error ? <p role="alert">{error}</p> : null}
    <div className="dialog-actions">
      {onCancel ? <button type="button" onClick={onCancel}>{copy.agents.cancel}</button> : null}
      <button type="submit" className="primary" disabled={!ready || busy}>{busy ? copy.agents.saving : copy.agents.save}</button>
    </div>
  </form>;
}

/** After a change: the service reads its agents again when it restarts. */
export function RestartNotice({ runningTurns, onLater }: { runningTurns: number; onLater: () => void }) {
  const client = useClient();
  return <div className="restart-notice" role="status">
    <b>{copy.agents.restartTitle}</b>
    <p>{runningTurns ? fill(copy.agents.restartBusy, { count: runningTurns }) : copy.agents.restartBody}</p>
    <div className="dialog-actions">
      <button type="button" onClick={onLater}>{copy.agents.later}</button>
      <button type="button" className="primary" onClick={() => { client.call("service.shutdown", {}).catch(() => undefined); onLater(); }}>{copy.agents.restart}</button>
    </div>
  </div>;
}
