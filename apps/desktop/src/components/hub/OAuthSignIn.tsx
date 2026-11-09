import {useEffect, useRef, useState} from "react";
import {openExternal} from "../../hub/openExternal";
import {usePreferences} from "../../settings/preferences";
import {useClient} from "../../rpc/context";

type ProviderState = {id: string; ready: boolean; email: boolean; calendar: boolean};

const en = {
  title: "Sign in with your account", google: "Sign in with Google", microsoft: "Sign in with Microsoft",
  waiting: "Finish signing in in your browser, then come back here…", done: (address: string) => `Added ${address}.`,
  or: "Or use an app password", setup: "Set up sign-in", setupHelp: "Sign-in needs an ID from the provider's developer console. Paste it once; Vatra remembers it.",
  notReady: (name: string) => `Signing in with ${name} isn't available in this version yet. Use an app password below instead.`,
  own: "I have my own sign-in ID",
  clientId: "Client ID", clientSecret: "Client secret (Google only)", saveSetup: "Save the ID", cancel: "Cancel", failed: "Signing in didn't work. Try again.",
};
const ro = {
  title: "Conectează-te cu contul tău", google: "Conectează-te cu Google", microsoft: "Conectează-te cu Microsoft",
  waiting: "Termină conectarea în browser, apoi revino aici…", done: (address: string) => `Am adăugat ${address}.`,
  or: "Sau folosește o parolă de aplicație", setup: "Pregătește conectarea",
  notReady: (name: string) => `Conectarea cu ${name} nu este încă disponibilă în această versiune. Folosește mai jos o parolă de aplicație.`,
  own: "Am propriul meu ID de conectare", setupHelp: "Conectarea are nevoie de un ID din consola pentru dezvoltatori a furnizorului. Lipește-l o dată; Vatra îl ține minte.",
  clientId: "ID client", clientSecret: "Secret client (doar Google)", saveSetup: "Salvează ID-ul", cancel: "Renunță", failed: "Conectarea nu a mers. Încearcă din nou.",
};

/** One-click sign-in for mail and calendar: the person approves in their browser and the account is added. */
export function OAuthSignIn({purpose, name, onSaved}: {purpose: "email" | "calendar"; name: string; onSaved: () => void}) {
  const rpc = useClient();
  const c = usePreferences((state) => state.preferences.language) === "ro" ? ro : en;
  const [providers, setProviders] = useState<ProviderState[]>([]);
  const [message, setMessage] = useState<string | null>(null);
  const [waiting, setWaiting] = useState(false);
  const [setup, setSetup] = useState<string | null>(null);
  const [unavailable, setUnavailable] = useState<string | null>(null);
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  const alive = useRef(true);

  const load = () => { void rpc.call<ProviderState[]>("oauth.providers", {}).then(setProviders).catch(() => undefined); };
  useEffect(() => { alive.current = true; load(); return () => { alive.current = false; }; }, [rpc]);

  const begin = async (provider: string) => {
    setMessage(null); setWaiting(true);
    try {
      const flow = await rpc.call<{flowId: string; url: string}>("oauth.start", {provider, purpose});
      await openExternal(flow.url);
      for (;;) {
        await new Promise((resolve) => setTimeout(resolve, 1500));
        if (!alive.current) return;
        const poll = await rpc.call<{state: string; address?: string; error?: string}>("oauth.poll", {flowId: flow.flowId});
        if (poll.state === "pending") continue;
        if (poll.state === "failed") { setMessage(poll.error ?? c.failed); break; }
        const saved = await rpc.call<{address: string}>("oauth.finish", {flowId: flow.flowId, purpose, name: name.trim() || undefined});
        setMessage(c.done(saved.address));
        onSaved();
        break;
      }
    } catch (caught) { if (alive.current) setMessage(caught instanceof Error && caught.message ? caught.message : c.failed); }
    if (alive.current) setWaiting(false);
  };

  const saveSetup = async () => {
    if (!setup) return;
    try {
      await rpc.call("oauth.configure", {provider: setup, clientId, clientSecret: setup === "google" ? clientSecret : undefined});
      setSetup(null); setClientId(""); setClientSecret(""); load();
    } catch (caught) { setMessage(caught instanceof Error ? caught.message : c.failed); }
  };

  const shown = providers.filter((p) => (purpose === "email" ? p.email : p.calendar));
  if (shown.length === 0) return null;
  return (
    <div className="oauth-signin" role="group" aria-label={c.title}>
      <h4 className="hub-sub">{c.title}</h4>
      <div className="hub-actions">
        {shown.map((p) => (
          <button key={p.id} type="button" className="primary" disabled={waiting}
            onClick={() => (p.ready ? void begin(p.id) : setUnavailable(p.id))}>
            {p.id === "google" ? c.google : c.microsoft}
          </button>
        ))}
      </div>
      {waiting ? <p role="status" className="hub-meta">{c.waiting}</p> : null}
      {message ? <p role="status" className="hub-meta">{message}</p> : null}
      {unavailable && !setup ? (
        <p role="status" className="hub-meta">{c.notReady(unavailable === "google" ? "Google" : "Microsoft")}{" "}
          <button type="button" className="link" onClick={() => { setSetup(unavailable); setUnavailable(null); }}>{c.own}</button></p>
      ) : null}
      {setup ? (
        <div className="hub-form" aria-label={c.setup}>
          <p className="hub-meta">{c.setupHelp}</p>
          <label>{c.clientId}<input value={clientId} autoComplete="off" onChange={(e) => setClientId(e.target.value)} /></label>
          {setup === "google" ? <label>{c.clientSecret}<input type="password" value={clientSecret} autoComplete="off" onChange={(e) => setClientSecret(e.target.value)} /></label> : null}
          <div className="hub-actions"><button type="button" onClick={() => setSetup(null)}>{c.cancel}</button><button type="button" className="primary" disabled={!clientId.trim()} onClick={() => void saveSetup()}>{c.saveSetup}</button></div>
        </div>
      ) : null}
      <p className="hub-meta">{c.or}</p>
    </div>
  );
}
