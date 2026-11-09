import { RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { formatWhen } from "../hub/time";
import { useClient } from "../rpc/context";
import { usePreferences } from "../settings/preferences";
import { ProviderIcon } from "./ProviderIcon";

type Window = { key: string; left: number; resets_at?: number | null; resets_text?: string | null };
type ProviderLimits = { id: string; name: string; refreshable: boolean; updated?: number | null; windows: Window[] };

const text = {
  en: {
    intro: "How much of each subscription you have left.",
    claude: "Claude's numbers update each time you send a prompt to Claude, so they show the moment of your last message.",
    refreshable: "Codex is read when you open this page or press Refresh. No prompt is sent to a model.",
    left: "left", resets: "Resets", in: "in", updated: "Updated", refresh: "Refresh", refreshing: "Refreshing…",
    claudeEmpty: "Nothing yet. Send a prompt to Claude and its limits will show up here.",
    empty: "This agent hasn't reported its limits yet.",
    failed: "Couldn't read the limits. Try again.",
    names: { five_hour: "Session (5 hours)", seven_day: "Weekly", seven_day_opus: "Weekly · Opus", seven_day_sonnet: "Weekly · Sonnet", "5h limit": "Session (5 hours)", "Weekly limit": "Weekly" } as Record<string, string>,
  },
  ro: {
    intro: "Cât a mai rămas din fiecare abonament.",
    claude: "Cifrele Claude se actualizează de fiecare dată când trimiți un mesaj către Claude, deci arată momentul ultimului tău mesaj.",
    refreshable: "Codex se citește când deschizi această pagină sau apeși Reîmprospătează. Nu se trimite niciun mesaj către un model.",
    left: "rămas", resets: "Se resetează", in: "peste", updated: "Actualizat", refresh: "Reîmprospătează", refreshing: "Se reîmprospătează…",
    claudeEmpty: "Încă nimic. Trimite un mesaj către Claude și limitele apar aici.",
    empty: "Acest agent nu și-a raportat încă limitele.",
    failed: "Nu am putut citi limitele. Încearcă din nou.",
    names: { five_hour: "Sesiune (5 ore)", seven_day: "Săptămânal", seven_day_opus: "Săptămânal · Opus", seven_day_sonnet: "Săptămânal · Sonnet", "5h limit": "Sesiune (5 ore)", "Weekly limit": "Săptămânal" } as Record<string, string>,
  },
};

function untilReset(at: number, word: string): string {
  const seconds = Math.max(0, at - Math.floor(Date.now() / 1000));
  const days = Math.floor(seconds / 86_400), hours = Math.floor((seconds % 86_400) / 3600), minutes = Math.floor((seconds % 3600) / 60);
  const span = days > 0 ? `${days}d ${hours}h` : hours > 0 ? `${hours}h ${minutes}m` : `${minutes}m`;
  return `${word} ${span}`;
}

/** What is left of each subscription: Claude from its last turn, Codex asked on request. */
export function LimitsPage() {
  const rpc = useClient();
  const language = usePreferences((state) => state.preferences.language);
  const t = language === "ro" ? text.ro : text.en;
  const [providers, setProviders] = useState<ProviderLimits[]>([]);
  const [busy, setBusy] = useState<string[]>([]);
  const [failed, setFailed] = useState<string[]>([]);

  const load = useCallback(async () => {
    try { setProviders((await rpc.call<{ providers: ProviderLimits[] }>("limits.get", {})).providers); } catch { /* the page keeps what it has */ }
  }, [rpc]);

  const refresh = useCallback(async (id: string) => {
    setBusy((old) => [...old, id]);
    try {
      setProviders((await rpc.call<{ providers: ProviderLimits[] }>("limits.refresh", { provider: id })).providers);
      setFailed((old) => old.filter((item) => item !== id));
    } catch { setFailed((old) => [...old.filter((item) => item !== id), id]); }
    setBusy((old) => old.filter((item) => item !== id));
  }, [rpc]);

  useEffect(() => {
    let live = true;
    (async () => {
      try {
        const first = (await rpc.call<{ providers: ProviderLimits[] }>("limits.get", {})).providers;
        if (!live) return;
        setProviders(first);
        for (const provider of first) if (provider.refreshable && live) void refresh(provider.id);
      } catch { /* shown empty */ }
    })();
    // Claude's numbers change when a turn ends, so the stored reading is looked at again while the page is open.
    const timer = setInterval(() => void load(), 5000);
    return () => { live = false; clearInterval(timer); };
  }, [rpc, load, refresh]);

  const label = (key: string) => t.names[key] ?? key.replace(/_/g, " ");
  return (
    <section className="limits-page">
      <p className="help">{t.intro}</p>
      <p className="limits-note" role="note">{t.claude}</p>
      {providers.some((provider) => provider.refreshable) ? <p className="help">{t.refreshable}</p> : null}
      {providers.map((provider) => (
        <article key={provider.id} className="limits-card" aria-label={provider.name}>
          <header>
            <h3><ProviderIcon provider={provider.id === "codex" ? "openai" : provider.id} size={16} />{provider.name}</h3>
            {provider.updated ? <span className="help">{t.updated} {formatWhen(provider.updated, language)}</span> : null}
            {provider.refreshable ? <button type="button" className="quiet" disabled={busy.includes(provider.id)} onClick={() => void refresh(provider.id)}>
              <RefreshCw size={13} aria-hidden="true" />{busy.includes(provider.id) ? t.refreshing : t.refresh}</button> : null}
          </header>
          {failed.includes(provider.id) ? <p role="alert">{t.failed}</p> : null}
          {provider.windows.length === 0 ? <p className="help">{provider.id === "claude" ? t.claudeEmpty : t.empty}</p> : null}
          {provider.windows.map((window) => {
            const percent = Math.round(window.left * 100);
            const level = percent >= 40 ? "ok" : percent >= 15 ? "low" : "critical";
            const reset = window.resets_at ? untilReset(window.resets_at, t.in) : window.resets_text ?? "";
            return (
              <div className="limit-row" key={window.key}>
                <div className="limit-name">
                  <b>{label(window.key)}</b>
                  <span className="limit-percent"><strong>{percent}%</strong> {t.left}</span>
                </div>
                <div className="limit-bar" role="progressbar" aria-label={label(window.key)} aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent} data-level={level}>
                  <span style={{ width: `${percent}%` }} />
                </div>
                {reset ? <span className="limit-reset">{t.resets} {reset}</span> : null}
              </div>
            );
          })}
        </article>
      ))}
    </section>
  );
}
