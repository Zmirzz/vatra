import { Check, ChevronDown, Plus, Search } from "lucide-react";
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { agentLabel } from "../conversation/labels";
import { copy } from "../copy";
import { useClient } from "../rpc/context";
import type { AgentOptions, AgentView, Provider, TurnOptions } from "../rpc/types";
import { useAppStore } from "../state/store";
import { AddModelDialog } from "./AddModelDialog";
import { ProviderIcon } from "./ProviderIcon";
import { pickerPlacement } from "./pickerPlacement";

type GroupId = "claude" | "chatgpt" | "local" | "other" | `service:${string}` | `agent:${string}`;
type Entry = { value: string; label: string; detail: string; group: GroupId; model?: string };

const FIXED_GROUPS: GroupId[] = ["claude", "chatgpt", "local"];

/** Where an agent belongs in the list, in words a person knows: an account, this computer, or a service. */
export function groupOf(name: string, view: AgentView | undefined): GroupId {
  if (name.startsWith("local:")) return "local";
  if (view?.kind === "claude") return "claude";
  if (view?.kind === "acp") return (view.provider === "openai" || /codex|chatgpt/i.test(name)) ? "chatgpt" : `agent:${name}`;
  if (view?.kind === "model" && view.provider) return `service:${view.provider}`;
  return "other";
}

/** The provider a group shows the mark of: a service shows its own, an account shows its company's. */
const markOf = (group: GroupId): string => (group.startsWith("service:") ? group.slice(8) : group.startsWith("agent:") ? group.slice(6) : group);

/** The model picker: one place to choose a model and to add one, in the order of the work a person does. */
export function ModelPicker({ value, onSelect, options = {}, onOptionsChange, projectId = 0, disabled = false, align = "end" }: {
  value: string; onSelect: (value: string, options?: TurnOptions) => void; options?: TurnOptions;
  onOptionsChange?: (options: TurnOptions) => void; projectId?: number; disabled?: boolean; align?: "start" | "end";
}) {
  const client = useClient();
  const agents = useAppStore((state) => state.agents);
  const [open, setOpen] = useState(false);
  const [catalogues, setCatalogues] = useState<Record<string, AgentOptions>>({});
  const [pending, setPending] = useState<string[]>([]);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [picked, setPicked] = useState<TurnOptions>(options);
  const [effortLoading, setEffortLoading] = useState(false);
  const [adding, setAdding] = useState(false);
  const [query, setQuery] = useState("");
  const [rail, setRail] = useState<"all" | GroupId>("all");
  const [active, setActive] = useState(0);
  const [views, setViews] = useState<AgentView[]>([]);
  const [providers, setProviders] = useState<Provider[]>([]);
  const root = useRef<HTMLDivElement>(null);
  const popup = useRef<HTMLDivElement>(null);
  const [placement, setPlacement] = useState<ReturnType<typeof pickerPlacement>>();
  const chip = useRef<HTMLButtonElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const listId = useId();

  useLayoutEffect(() => {
    if (!open) return;
    const update = () => {
      if (chip.current) setPlacement(pickerPlacement(chip.current.getBoundingClientRect(), { width: window.innerWidth, height: window.innerHeight }, align));
    };
    update();
    window.addEventListener("resize", update);
    const onScroll = (event: Event) => {
      if (!(event.target instanceof Node) || !popup.current?.contains(event.target)) update();
    };
    window.addEventListener("scroll", onScroll, true);
    const observer = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(update);
    if (chip.current) observer?.observe(chip.current);
    return () => {
      window.removeEventListener("resize", update);
      window.removeEventListener("scroll", onScroll, true);
      observer?.disconnect();
    };
  }, [open, align]);

  // What each agent is comes from the service, asked when the list opens, so a model added a moment ago shows up.
  useEffect(() => {
    let live = true;
    client.call<{ agents: AgentView[] }>("agents.configured", {}).then((result) => { if (live) setViews(result?.agents ?? []); }).catch(() => undefined);
    client.call<Provider[]>("agents.providers", {}).then((list) => { if (live) setProviders(Array.isArray(list) ? list : []); }).catch(() => undefined);
    return () => { live = false; };
  }, [client, open, agents]);

  useEffect(() => { setPicked(options); }, [value, options.model, options.effort]);
  useEffect(() => {
    if (!open || !onOptionsChange) return;
    let live = true;
    const names = agents.filter((name) => !name.startsWith("local:"));
    setPending(names); setErrors({}); setCatalogues({});
    for (const name of names) {
      client.call<AgentOptions>("agents.options", { name, project_id: projectId }).then((found) => {
        if (live && Array.isArray(found?.models)) setCatalogues((old) => ({ ...old, [name]: found }));
      }).catch(() => { if (live) setErrors((old) => ({ ...old, [name]: copy.picker.loadFailed })); })
        .finally(() => { if (live) setPending((old) => old.filter((item) => item !== name)); });
    }
    return () => { live = false; };
  }, [client, open, agents, projectId, !!onOptionsChange]);

  const acpSelected = views.some((view) => view.name === value && view.kind === "acp");
  useEffect(() => {
    if (!open || !acpSelected || !picked.model || !onOptionsChange) return;
    let live = true;
    setEffortLoading(true);
    client.call<AgentOptions>("agents.options", { name: value, project_id: projectId, model: picked.model }).then((found) => {
      if (live && Array.isArray(found?.models)) setCatalogues((old) => ({ ...old, [value]: found }));
    }).catch(() => { if (live) setErrors((old) => ({ ...old, [value]: copy.picker.loadFailed })); })
      .finally(() => { if (live) setEffortLoading(false); });
    return () => { live = false; };
  }, [client, open, acpSelected, picked.model, value, projectId]);

  const entries: Entry[] = useMemo(() => agents.flatMap((name) => {
    const view = views.find((candidate) => candidate.name === name);
    const group = groupOf(name, view);
    const detail = view?.kind === "model" ? view.model ?? "" : "";
    const models = catalogues[name]?.models ?? [];
    const entries: Entry[] = models.map((model) => ({ value: name, label: model.name, detail: model.id, group, model: model.id }));
    // Keep the configured/default route available, even if discovery fails or its model was retired.
    return [{ value: name, label: agentLabel(name).replace(/^local:/, ""), detail, group }, ...entries];
  }), [agents, views, catalogues]);

  const groupName = (group: GroupId): string => {
    if (group.startsWith("service:")) return providers.find((item) => item.id === group.slice(8))?.name ?? group.slice(8);
    if (group.startsWith("agent:")) return agentLabel(group.slice(6));
    return copy.picker.groups[group as "claude" | "chatgpt" | "local" | "other"];
  };
  const present = useMemo(() => {
    const ids = new Set(entries.map((entry) => entry.group));
    const services = [...ids].filter((id) => id.startsWith("service:") || id.startsWith("agent:")).sort();
    return [...FIXED_GROUPS.filter((id) => ids.has(id)), ...services, ...(ids.has("other") ? ["other" as GroupId] : [])];
  }, [entries]);

  const needle = query.trim().toLowerCase();
  const shown = entries.filter((entry) => (rail === "all" || entry.group === rail) && (!needle || `${entry.label} ${entry.detail}`.toLowerCase().includes(needle)));
  const sections = present.map((group) => ({ group, items: shown.filter((entry) => entry.group === group) })).filter((section) => section.items.length > 0);
  const flat = sections.flatMap((section) => section.items);
  const selected = (entry: Entry) => entry.value === value && entry.model === picked.model;
  const current = entries.find(selected) ?? entries.find((entry) => entry.value === value);
  const catalogue = catalogues[value];
  const levels = effortLoading ? [] : (catalogue?.models.find((model) => model.id === (picked.model ?? catalogue.current_model))?.efforts ?? catalogue?.efforts) ?? [];
  const effortName = (id: string, fallback: string) => copy.picker.effortNames[id as keyof typeof copy.picker.effortNames] ?? fallback;

  useEffect(() => { setActive(Math.max(0, flat.findIndex((entry) => entry.value === value))); }, [open, query, rail]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (!open) return;
    search.current?.focus();
    const onPointer = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node) && !popup.current?.contains(event.target as Node)) setOpen(false); };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") { event.stopPropagation(); setOpen(false); chip.current?.focus(); }
    };
    document.addEventListener("pointerdown", onPointer);
    window.addEventListener("keydown", onKey, true);
    return () => { document.removeEventListener("pointerdown", onPointer); window.removeEventListener("keydown", onKey, true); };
  }, [open]);

  useEffect(() => {
    if (open) document.getElementById(listId + "-" + active)?.scrollIntoView?.({ block: "nearest" });
  }, [open, active, listId]);

  const choose = (entry: Entry) => {
    if (entry.value === value && onOptionsChange) {
      const next = entry.model ? { model: entry.model } : {};
      setPicked(next); onOptionsChange(next); return;
    }
    setOpen(false); setQuery(""); setRail("all"); chip.current?.focus();
    if (entry.model) onSelect(entry.value, { model: entry.model }); else onSelect(entry.value);
  };
  const onKeys = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.target !== search.current) return;
    if (flat.length === 0) return;
    if (event.key === "ArrowDown") { event.preventDefault(); setActive((index) => (index + 1) % flat.length); }
    else if (event.key === "ArrowUp") { event.preventDefault(); setActive((index) => (index - 1 + flat.length) % flat.length); }
    else if (event.key === "Enter" && document.activeElement === search.current) { event.preventDefault(); choose(flat[Math.min(active, flat.length - 1)]); }
  };

  const label = picked.model ? (entries.find(selected)?.label ?? picked.model) : current?.label ?? (value ? agentLabel(value) : copy.picker.addFirst);

  return (
    <div className="menu model-picker" ref={root}>
      <button
        disabled={disabled}
        ref={chip}
        type="button"
        className="chip"
        aria-label={`${copy.picker.label}: ${label}`}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => (agents.length === 0 ? setAdding(true) : setOpen(!open))}
      >
        <ProviderIcon provider={markOf(current?.group ?? "other")} size={14} />
        <span className="picker-trigger-label">{label}</span>
        {picked.effort ? <span className="picker-trigger-effort">{" · " + effortName(picked.effort, picked.effort)}</span> : null}
        <ChevronDown size={13} aria-hidden="true" />
      </button>
      {open ? createPortal(
        <div ref={popup} role="dialog" aria-label={copy.picker.label} className="picker-popup" style={placement} onKeyDown={onKeys}>
          <div className="picker-body">
            <nav className="picker-rail" aria-label={copy.picker.providers}>
              <button type="button" aria-pressed={rail === "all"} title={copy.picker.all} aria-label={copy.picker.all} onClick={() => setRail("all")}><ProviderIcon provider="other" /></button>
              {present.map((group) => (
                <button key={group} type="button" aria-pressed={rail === group} title={groupName(group)} aria-label={groupName(group)} onClick={() => setRail(rail === group ? "all" : group)}><ProviderIcon provider={markOf(group)} /></button>
              ))}
            </nav>
            <div className="picker-main">
              <label className="picker-search">
                <Search size={14} aria-hidden="true" />
                <input ref={search} role="combobox" aria-expanded="true" aria-controls={listId} aria-activedescendant={flat[active] ? `${listId}-${active}` : undefined} aria-label={copy.picker.search} placeholder={copy.picker.search} value={query} onChange={(event) => setQuery(event.target.value)} />
              </label>
              <div id={listId} role="listbox" aria-label={copy.picker.label} className="picker-list">
                {sections.length === 0 ? <p className="help picker-empty">{copy.picker.empty}</p> : null}
                {sections.map((section) => (
                  <div key={section.group} role="group" aria-label={groupName(section.group)}>
                    <div className="picker-group">{groupName(section.group)}</div>
                    {section.items.map((entry) => {
                      const index = flat.indexOf(entry);
                      return (
                        <button key={entry.model ? `${entry.value}:model:${entry.model}` : `${entry.value}:route`} id={`${listId}-${index}`} type="button" role="option" aria-selected={selected(entry)} data-active={index === active} className="picker-item" onMouseEnter={() => setActive(index)} onClick={() => choose(entry)}>
                          <ProviderIcon provider={markOf(entry.group)} />
                          <span className="picker-item-label" title={entry.detail || entry.label}>{entry.label}</span>
                          {selected(entry) ? <Check size={14} className="picker-check" aria-hidden="true" /> : null}
                        </button>
                      );
                    })}
                  </div>
                ))}
              </div>
            </div>
          </div>
          {pending.length || effortLoading ? <p role="status" className="help picker-status">{copy.picker.loading}</p> : null}
          {errors[value] ? <p role="alert" className="help picker-status">{errors[value]}</p> : null}
          {onOptionsChange ? <div className="picker-settings">
            <button type="button" className="quiet" onClick={() => { setPicked({}); onOptionsChange({}); }}>{copy.picker.defaults}</button>
            {levels.length ? <label className="picker-efforts"><span>{copy.picker.effort}</span>
              <select value={picked.effort ?? ""} onChange={(event) => {
                const next = event.target.value ? { ...picked, effort: event.target.value } : picked.model ? { model: picked.model } : {};
                setPicked(next); onOptionsChange(next);
              }}>
                <option value="">{copy.picker.automatic}</option>
                {levels.map((level) => <option key={level.id} value={level.id}>{effortName(level.id, level.name)}</option>)}
              </select>
            </label> : null}
          </div> : null}
          <button type="button" className="picker-add" onClick={() => { setOpen(false); setAdding(true); }}>
            <span aria-hidden="true"><Plus size={14} /></span>
            {copy.picker.add}
          </button>
        </div>, document.body
      ) : null}
      {adding ? <AddModelDialog onClose={() => setAdding(false)} /> : null}
    </div>
  );
}
