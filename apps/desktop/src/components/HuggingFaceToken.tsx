import { useEffect, useState } from "react";
import { copy } from "../copy";
import { useClient } from "../rpc/context";

export function HuggingFaceToken({ onChanged }: { onChanged: (hasToken: boolean) => void }) {
  const client = useClient();
  const [token, setToken] = useState("");
  const [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    client.call<{ hasToken: boolean }>("hf.status", {}).then((status) => { if (live) { setSaved(status.hasToken); onChanged(status.hasToken); } }).catch(() => undefined);
    return () => { live = false; };
  }, [client, onChanged]);
  const save = async (value: string | null) => {
    setBusy(true); setError(null);
    try { const status = await client.call<{ hasToken: boolean }>("hf.set_token", { token: value }); setToken(""); setSaved(status.hasToken); onChanged(status.hasToken); }
    catch (caught) { setError((caught as Error).message); }
    finally { setBusy(false); }
  };
  return <details><summary>{copy.modelSetup.token}</summary>
    <p className="help">{copy.modelSetup.tokenHelp}</p>
    {saved ? <p>{copy.modelSetup.tokenSaved}</p> : null}
    <form className="row" onSubmit={(event) => { event.preventDefault(); void save(token); }}>
      <input type="password" aria-label={copy.modelSetup.token} autoComplete="new-password" value={token} onChange={(event) => setToken(event.target.value)} />
      <button type="submit" disabled={busy || !token.trim()}>{copy.modelSetup.saveToken}</button>
      {saved ? <button type="button" disabled={busy} onClick={() => void save(null)}>{copy.modelSetup.removeToken}</button> : null}
    </form>
    {error ? <p role="alert">{error}</p> : null}
  </details>;
}
