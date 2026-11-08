import { Cpu, KeyRound, LayoutGrid } from "lucide-react";
import claude from "../assets/providers/claude.svg";
import openai from "../assets/providers/openai.svg";
import openrouter from "../assets/providers/openrouter.svg";
import antigravity from "../assets/providers/antigravity.svg";

/** Brand artwork is bundled locally; see assets/providers/README.md for sources and licence. */
export function ProviderIcon({ provider, size = 15 }: { provider: string; size?: number }) {
  const name = provider.toLowerCase();
  const artwork = name === "claude" || name === "anthropic" ? claude
    : name === "chatgpt" || name === "openai" || name.includes("codex") ? openai
    : name === "openrouter" ? openrouter : name.includes("antigravity") ? antigravity : null;
  if (artwork) return <span aria-hidden="true" className="provider-icon" style={{
    width: size, height: size, maskImage: 'url("' + artwork + '")', WebkitMaskImage: 'url("' + artwork + '")',
    color: artwork === claude ? "#d97757" : undefined,
  }} />;
  if (provider === "local") return <Cpu size={size} aria-hidden="true" color="#5fb98a" />;
  if (provider === "other") return <LayoutGrid size={size} aria-hidden="true" />;
  return <KeyRound size={size} aria-hidden="true" />;
}
