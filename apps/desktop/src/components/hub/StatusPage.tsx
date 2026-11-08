import {CircleAlert, CircleCheck, CircleDashed, RefreshCw} from "lucide-react";
import {useCallback, useEffect, useState} from "react";
import {useMore} from "../../hub/moreCopy";
import {formatWhen} from "../../hub/time";
import type {HealthItem, HealthStatus} from "../../hub/types";
import {useRefresh} from "../../hub/useRefresh";
import {useClient} from "../../rpc/context";
import {usePreferences} from "../../settings/preferences";

/** What works and what doesn't, with the reason for each failure and a button to check again. */
export function StatusPage() {
  const m = useMore();
  const rpc = useClient();
  const language = usePreferences((s) => s.preferences.language);
  const [status, setStatus] = useState<HealthStatus | null>(null);
  const [checking, setChecking] = useState<string | null>(null);
  const [error, setError] = useState(false);

  const load = useCallback(async () => {
    try { setStatus(await rpc.call<HealthStatus>("health.status", {})); setError(false); } catch { setError(true); }
  }, [rpc]);
  useEffect(() => { void load(); }, [load]);
  useRefresh(rpc, ["schedule_changed", "research_changed", "calendar_changed"], () => void load());

  const check = async (item: HealthItem) => {
    if (!item.retry) return;
    setChecking(item.id);
    try { await rpc.call(item.retry.method, item.retry.params); } catch { /* the failure is recorded by the service and shown below */ }
    await load();
    setChecking(null);
  };

  const groups = ["accounts", "search", "models"] as const;
  const icon = (state: HealthItem["state"]) => (state === "ok" ? <CircleCheck size={16} aria-hidden="true" /> : state === "down" ? <CircleAlert size={16} aria-hidden="true" /> : <CircleDashed size={16} aria-hidden="true" />);
  const label = (state: HealthItem["state"]) => (state === "ok" ? m.statusOk : state === "down" ? m.statusDown : m.statusUnknown);

  return (
    <section className="status-page">
      <p className="hub-note">{m.statusHelp}</p>
      {error ? <p role="alert" className="hub-error">!</p> : null}
      {status ? <p role="status" className="status-summary" data-down={status.down > 0}>{status.down > 0 ? m.problems(status.down) : m.allGood}</p> : null}
      {status && status.items.length === 0 ? <p className="hub-meta">{m.nothing}</p> : null}
      {groups.map((group) => {
        const items = status?.items.filter((i) => i.group === group) ?? [];
        return items.length ? (
          <div key={group} className="status-group" role="group" aria-label={m.groups[group]}>
            <h3 className="hub-sub">{m.groups[group]}</h3>
            <ul className="hub-list">
              {items.map((item) => (
                <li key={item.id} className="hub-card status-item" data-state={item.state}>
                  <span className="status-icon" data-state={item.state}>{icon(item.state)}</span>
                  <div className="hub-card-main">
                    <b>{item.label === "Local model server" ? m.localServer : item.label}</b>
                    <span className="hub-meta">{m.healthKinds[item.kind] ?? item.kind} · {label(item.state)}{item.at ? ` · ${m.lastChecked(formatWhen(item.at, language))}` : ""}</span>
                    {item.detail ? <span className="status-detail">{item.detail}</span> : null}
                  </div>
                  {item.retry ? (
                    <div className="hub-actions">
                      <button type="button" disabled={checking === item.id} onClick={() => void check(item)}><RefreshCw size={13} aria-hidden="true" />{checking === item.id ? m.checking : m.checkAgain}</button>
                    </div>
                  ) : null}
                </li>
              ))}
            </ul>
          </div>
        ) : null;
      })}
    </section>
  );
}
