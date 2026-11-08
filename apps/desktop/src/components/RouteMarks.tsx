import { useState } from "react";
import { copy } from "../copy";
import type { Route, RouteVerdicts, Verdict } from "../rpc/types";

export function verdictText(verdict: Verdict | undefined): string {
  if (!verdict) return copy.transfer.unchecked;
  if (verdict === "works") return copy.transfer.verdict.works;
  const [key, reason] = Object.entries(verdict)[0];
  return `${copy.transfer.verdict[key as Exclude<keyof typeof copy.transfer.verdict, "works">]}: ${reason}`;
}

export function RouteMarks({ routes }: { routes: RouteVerdicts }) {
  const [expanded, setExpanded] = useState<string | null>(null);
  return <div className="route-marks">{([[copy.transfer.routes.local, "engine"], [copy.transfer.routes.api, "engine"], [copy.transfer.routes.claude, "claude"], [copy.transfer.routes.acp, "acp"]] as [string, Route][]).map(([label, route]) => {
    const verdict = routes.find(([name]) => name === route)?.[1];
    const reason = verdictText(verdict);
    return <span key={label}><button type="button" title={reason} aria-label={`${label}: ${reason}`} aria-expanded={expanded === label} data-ready={verdict === "works"} onClick={() => setExpanded(expanded === label ? null : label)}>{verdict === "works" ? "✓" : "·"} {label}</button>{expanded === label ? <small>{reason}</small> : null}</span>;
  })}</div>;
}
