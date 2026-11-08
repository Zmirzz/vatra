import {CalendarClock, CalendarDays, Command, CornerDownLeft, Files, FolderOpen, ListChecks, Mail, MessageCircle, Moon, Plus, Scale, Search, Settings2, type LucideIcon} from "lucide-react";
import {useEffect, useMemo, useRef, useState} from "react";
import {useMore} from "../hub/moreCopy";
import {useHubFocus} from "../hub/focus";
import {useClient} from "../rpc/context";
import {toggleMode, usePreferences} from "../settings/preferences";
import {openSession, useAppStore} from "../state/store";
import {navigateWorkspace, useWorkspace} from "../workspace/store";
import type {WorkspaceArea} from "../workspace/types";

type Item = {id: string; group: string; label: string; hint?: string; icon: LucideIcon; run: () => void | Promise<void>};

const SETTINGS: [string, string, string][] = [
  ["general", "Appearance", "Aspect"], ["models", "Models", "Modele"], ["agents", "Agents", "Agenți"], ["accounts", "Accounts", "Conturi"], ["status", "Status", "Stare"],
  ["market", "Connections", "Conexiuni"], ["mcp-market", "Tool store (MCP)", "Magazin de unelte (MCP)"], ["mcp-installed", "Installed MCP servers", "Servere MCP instalate"],
  ["skills", "Skills", "Skills"], ["acp-market", "ACP marketplace", "Magazin ACP"], ["import", "Import", "Import"],
];

const AREAS: [WorkspaceArea, string, string, LucideIcon][] = [
  ["conversations", "Conversations", "Conversații", MessageCircle], ["documents", "Documents", "Documente", Files], ["tasks", "Tasks", "Sarcini", ListChecks],
  ["schedule", "Scheduled", "Programate", CalendarClock], ["calendar", "Calendar", "Calendar", CalendarDays], ["email", "Email", "Email", Mail],
  ["research", "Research", "Cercetare", Search], ["compare", "Compare", "Compară", Scale],
];

/** A search box over everything: pages, conversations, projects, settings and the common actions. Opens with Ctrl+Shift+P. */
export function CommandPalette({onClose}: {onClose: () => void}) {
  const m = useMore();
  const ro = usePreferences((s) => s.preferences.language) === "ro";
  const client = useClient();
  const {cache} = useWorkspace();
  const sessions = useAppStore((s) => s.sessions);
  const projects = useAppStore((s) => s.projects);
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const [meaning, setMeaning] = useState<Item[]>([]);
  const [searching, setSearching] = useState(false);
  const [semantic, setSemantic] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLUListElement>(null);

  const go = (area: WorkspaceArea, projectId?: number | null) =>
    navigateWorkspace({projectId: projectId ?? cache.selection.projectId ?? projects[0]?.id ?? null, area, itemId: null});

  const items = useMemo<Item[]>(() => {
    const all: Item[] = [];
    for (const [area, en, rom, Icon] of AREAS) all.push({id: `area-${area}`, group: m.groupGo, label: ro ? rom : en, icon: Icon, run: () => void go(area)});
    all.push(
      {id: "act-new-chat", group: m.groupActions, label: m.actionNewConversation, icon: Plus, run: async () => { if (await go("conversations")) useAppStore.setState({activeSessionId: null, importedSession: null, openingImportedId: null}); }},
      {id: "act-new-research", group: m.groupActions, label: m.actionNewResearch, icon: Search, run: async () => { useHubFocus.setState({researchId: null}); await go("research"); }},
      {id: "act-new-event", group: m.groupActions, label: m.actionNewEvent, icon: CalendarDays, run: () => void go("calendar")},
      {id: "act-theme", group: m.groupActions, label: m.actionToggleTheme, icon: Moon, run: () => toggleMode()},
      {id: "act-settings", group: m.groupActions, label: m.actionOpenSettings, icon: Settings2, run: () => useAppStore.setState({settingsPage: "general"})},
    );
    for (const [id, en, rom] of SETTINGS) all.push({id: `set-${id}`, group: m.groupSettings, label: ro ? rom : en, icon: Settings2, run: () => useAppStore.setState({settingsPage: id as never})});
    for (const p of projects) all.push({id: `proj-${p.id}`, group: m.groupProjects, label: p.name, icon: FolderOpen, run: () => void go("documents", p.id)});
    for (const s of sessions.slice(0, 80)) all.push({id: `sess-${s.id}`, group: m.groupConversations, label: s.title, icon: MessageCircle, run: async () => { if (await go("conversations")) openSession(client, s.id); }});
    return all;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [m, ro, projects, sessions, client, cache.selection.projectId]);

  const shown = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return items.filter((i) => i.group !== m.groupConversations).slice(0, 40);
    const scored = items
      .map((i) => ({i, at: i.label.toLowerCase().indexOf(needle)}))
      .filter((x) => x.at >= 0)
      .sort((a, b) => (a.at === 0 ? 0 : 1) - (b.at === 0 ? 0 : 1) || a.i.label.length - b.i.label.length);
    return [...scored.map((x) => x.i).slice(0, 40), ...meaning.filter((i) => !scored.some((x) => x.i.label === i.label))];
  }, [items, query, m, meaning]);

  useEffect(() => { input.current?.focus(); }, []);
  // When an embedding account exists, what the person types is also looked up by meaning.
  useEffect(() => {
    void client.call<{configured: boolean}>("semantic.status", {}).then((s) => setSemantic(Boolean(s?.configured))).catch(() => undefined);
  }, [client]);
  useEffect(() => {
    const text = query.trim();
    if (!semantic || text.length < 3) { setMeaning([]); setSearching(false); return; }
    setSearching(true);
    let live = true;
    const timer = setTimeout(() => {
      void client.call<{kind: string; ref_id: number; project_id: number | null; title: string; snippet: string}[]>("semantic.search", {query: text, limit: 6})
        .then((hits) => {
          if (!live) return;
          setMeaning(hits.map((h) => ({
            id: `sem-${h.kind}-${h.ref_id}`, group: m.foundByMeaning, label: h.title, hint: h.snippet, icon: h.kind === "research" ? Search : Files,
            run: async () => {
              if (h.kind === "research") { useHubFocus.setState({researchId: h.ref_id}); await go("research"); }
              else await navigateWorkspace({projectId: h.project_id ?? cache.selection.projectId ?? projects[0]?.id ?? null, area: "documents", itemId: h.ref_id});
            },
          })));
        })
        .catch(() => { if (live) setMeaning([]); })
        .finally(() => { if (live) setSearching(false); });
    }, 350);
    return () => { live = false; clearTimeout(timer); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query, semantic, client, m]);
  useEffect(() => { setIndex(0); }, [query]);
  useEffect(() => { list.current?.querySelector('[aria-selected="true"]')?.scrollIntoView?.({block: "nearest"}); }, [index]);

  const choose = (item: Item | undefined) => {
    if (!item) return;
    onClose();
    void item.run();
  };
  const onKey = (event: React.KeyboardEvent) => {
    if (event.key === "ArrowDown") { event.preventDefault(); setIndex((i) => Math.min(i + 1, shown.length - 1)); }
    else if (event.key === "ArrowUp") { event.preventDefault(); setIndex((i) => Math.max(i - 1, 0)); }
    else if (event.key === "Enter") { event.preventDefault(); choose(shown[index]); }
    else if (event.key === "Escape") { event.preventDefault(); onClose(); }
  };

  let lastGroup = "";
  return (
    <div className="palette-backdrop" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div className="palette" role="dialog" aria-modal="true" aria-label={m.paletteTitle} onKeyDown={onKey}>
        <label className="palette-search">
          <Command size={16} aria-hidden="true" />
          <input ref={input} role="combobox" aria-expanded="true" aria-controls="palette-list" aria-activedescendant={shown[index] ? `palette-${shown[index].id}` : undefined}
            aria-label={m.paletteTitle} placeholder={m.palettePlaceholder} value={query} onChange={(e) => setQuery(e.target.value)} />
        </label>
        <ul id="palette-list" ref={list} role="listbox" className="palette-list">
          {shown.length === 0 ? <li className="palette-empty" role="presentation">{searching ? m.paletteMeaning : m.paletteEmpty}</li> : null}
          {shown.map((item, i) => {
            const header = item.group !== lastGroup ? item.group : null;
            lastGroup = item.group;
            const Icon = item.icon;
            return (
              <li key={item.id} role="presentation">
                {header ? <div className="palette-group" role="presentation">{header}</div> : null}
                <div id={`palette-${item.id}`} role="option" aria-selected={i === index} className="palette-item" onMouseMove={() => setIndex(i)} onClick={() => choose(item)}>
                  <Icon size={15} aria-hidden="true" /><span>{item.label}{item.hint ? <small className="palette-hint">{item.hint}</small> : null}</span>{i === index ? <CornerDownLeft size={13} aria-hidden="true" /> : null}
                </div>
              </li>
            );
          })}
        </ul>
        <footer className="palette-foot">{m.paletteHint}</footer>
      </div>
    </div>
  );
}
