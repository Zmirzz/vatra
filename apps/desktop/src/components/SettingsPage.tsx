import { Activity, BookOpen, Bot, Brain, Gauge, KeyRound, Boxes, Download, Flame, FolderInput, Palette, Plug, Search, Store, UserRound, X, type LucideIcon } from "lucide-react";
import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { copy, LANGUAGES, type Language } from "../copy";
import { COMMAND_IDS, comboFromEvent, displayCombo, findConflict, type CommandId } from "../settings/keybindings";
import { resetPreferences, updatePreferences, usePreferences, type Preferences } from "../settings/preferences";
import { themes, type ThemeId } from "../settings/themes";
import { ImportPage } from "./ImportPage";
import { ChatgptAccount } from "./ChatgptAccount";
import { ExtensionsPage } from "./ExtensionsPage";
import { ModelsPage } from "./ModelsPage";
import { LimitsPage } from "./LimitsPage";
import { MemoryPage } from "./MemoryPage";
import { MarketPage } from "./MarketPage";
import { AgentsPage } from "./AgentsPage";
import { RegistryPage } from "./RegistryPage";
import { SkillsPage } from "./SkillsPage";
import { InstalledMcpPage } from "./InstalledMcpPage";
import { UpdateStatus } from "./UpdateStatus";
import { AccountsPage } from "./hub/AccountsPage";
import { StatusPage } from "./hub/StatusPage";
import { useMore } from "../hub/moreCopy";
import { McpMarketPage } from "./hub/McpMarketPage";
import { hubCopy } from "../hub/copy";
import { useAppStore } from "../state/store";

const THEME_IDS = Object.keys(themes) as ThemeId[];

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="setting">
      <span>{label}</span>
      {children}
    </label>
  );
}

function ThemeSelect({ label, value, onChange }: { label: string; value: ThemeId; onChange: (id: ThemeId) => void }) {
  return (
    <Row label={label}>
      <select value={value} onChange={(event) => onChange(event.target.value as ThemeId)}>
        {THEME_IDS.map((id) => (
          <option key={id} value={id}>
            {copy.themes[id]}
          </option>
        ))}
      </select>
    </Row>
  );
}

function ShortcutRow({ command }: { command: CommandId }) {
  const bindings = usePreferences((state) => state.preferences.keybindings);
  const [recording, setRecording] = useState(false);
  const [conflict, setConflict] = useState<string | null>(null);

  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (!recording) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Escape") {
      setRecording(false);
      setConflict(null);
      return;
    }
    const combo = comboFromEvent(event.nativeEvent);
    if (combo === null) {
      return;
    }
    const taken = findConflict(bindings, command, combo);
    setRecording(false);
    if (taken !== null) {
      setConflict(copy.conflict(copy.commands[taken]));
      return;
    }
    setConflict(null);
    updatePreferences({ keybindings: { ...bindings, [command]: combo } });
  };

  return (
    <div>
      <button
        type="button"
        className="setting shortcut"
        aria-label={`${copy.commands[command]}: ${displayCombo(bindings[command])}`}
        data-recording={recording}
        onClick={() => setRecording(true)}
        onKeyDown={onKeyDown}
      >
        <span>{copy.commands[command]}</span>
        <kbd>{recording ? copy.pressKeys : displayCombo(bindings[command])}</kbd>
      </button>
      {conflict ? <div role="alert">{conflict}</div> : null}
    </div>
  );
}

export function SettingsPage({ onClose, onRestartService }: { onClose: () => void; onRestartService?: () => void }) {
  const dialog = useRef<HTMLDivElement>(null);
  const closeRef = useRef(onClose);
  useEffect(() => { closeRef.current = onClose; }, [onClose]);
  const [page, changePage] = useState<NonNullable<ReturnType<typeof useAppStore.getState>["settingsPage"]>>(useAppStore.getState().settingsPage ?? "general");
  const [query, setQuery] = useState("");
  const setPage = (next: typeof page) => { changePage(next); useAppStore.setState({ settingsPage: next }); };
  const preferences = usePreferences((state) => state.preferences);
  const set = (change: Partial<Preferences>) => updatePreferences(change);
  const ro = preferences.language === "ro";
  const moreCopy = useMore();
  const groups: { title: string; pages: { id: typeof page; title: string; keywords: string[] }[] }[] = [
    { title: ro ? "GENERAL" : "GENERAL", pages: [
      { id: "general" as const, title: copy.appearance, keywords: [copy.language, copy.mode, copy.keyboard, copy.sendShortcut, copy.fontScale, copy.density, copy.service, copy.restartService, "shortcuts", "keybindings"] },
    ] },
    { title: ro ? "AI ȘI AGENȚI" : "AI & AGENTS", pages: [
      { id: "models" as const, title: copy.models.tab, keywords: ["llama.cpp", "inference", "download", "GGUF"] },
      { id: "agents" as const, title: copy.agents.tab, keywords: ["ACP", "Claude", "Codex", "providers", "API"] },
      { id: "acp-market" as const, title: "ACP Marketplace", keywords: ["registry", "agents", "discover", "install"] },
      { id: "account" as const, title: copy.chatgpt.title, keywords: ["authentication", "login", "OAuth"] },
      { id: "memory" as const, title: ro ? "Memorie" : "Memory", keywords: ["about me", "facts", "remember", "profile", "preferences", "despre mine", "reține", "profil"] },
      { id: "limits" as const, title: ro ? "Limite" : "Limits", keywords: ["usage", "quota", "subscription", "weekly", "session", "Claude", "Codex", "utilizare", "cotă"] },
    ] },
    { title: ro ? "SERVICII" : "SERVICES", pages: [
      { id: "accounts" as const, title: hubCopy().accountsTitle, keywords: ["email", "IMAP", "calendar", "CalDAV", "Telegram", "search", "Brave", "password"] },
    ] },
    { title: ro ? "EXTENSII" : "EXTENSIONS", pages: [
      { id: "market" as const, title: copy.market.tab, keywords: ["MCP", "marketplace", "connections"] },
      { id: "mcp-market" as const, title: ro ? "Magazin de unelte (MCP)" : "Tool store (MCP)", keywords: ["MCP", "marketplace", "registry", "servers", "tools", "discover", "install", "store"] },
      { id: "mcp-installed" as const, title: ro ? "Servere MCP instalate" : "Installed MCP Servers", keywords: ["connect", "configure", "OAuth", "tools", "resources", "prompts"] },
      { id: "skills" as const, title: "Skills Manager", keywords: ["SKILL.md", "directories", "instructions", "install"] },
      { id: "extensions" as const, title: copy.transfer.extensions, keywords: ["skills", "MCP", "permissions", "tools"] },
    ] },
    { title: ro ? "SISTEM" : "SYSTEM", pages: [
      { id: "status" as const, title: moreCopy.statusTitle, keywords: ["health", "problems", "errors", "diagnostics", "working"] },
      { id: "import" as const, title: copy.transfer.tab, keywords: ["history", "data", "migration", "transfer"] },
    ] },
  ];
  const matches = (item: { title: string; keywords: string[] }) => [item.title, ...item.keywords].join(" ").toLocaleLowerCase().includes(query.trim().toLocaleLowerCase());
  const pageTitle = groups.flatMap((group) => group.pages).find((item) => item.id === page)?.title ?? copy.settings;
  const icons: Record<typeof page, LucideIcon> = {
    general: Palette, agents: Bot, account: UserRound, models: Boxes,
    import: FolderInput, market: Plug, extensions: Download,
    "acp-market": Store, "mcp-market": Store, "mcp-installed":Plug, skills: BookOpen, accounts: KeyRound, status: Activity, limits: Gauge, memory: Brain,
  };

  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const focusable = () => Array.from(dialog.current?.querySelectorAll<HTMLElement>(
      'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), a[href], summary, [tabindex="0"]',
    ) ?? []).filter((element) => {
      if (element.closest('[hidden], [inert]')) return false;
      const closed = element.closest('details:not([open])');
      return !closed || element === closed.querySelector('summary');
    });
    focusable()[0]?.focus();
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.defaultPrevented) return;
      if (event.key === "Escape") closeRef.current();
      if (event.key === "Tab") {
        const controls = focusable();
        const first = controls[0], last = controls.at(-1);
        if (event.shiftKey && (document.activeElement === first || !dialog.current?.contains(document.activeElement))) {
          event.preventDefault(); last?.focus();
        } else if (!event.shiftKey && (document.activeElement === last || !dialog.current?.contains(document.activeElement))) {
          event.preventDefault(); first?.focus();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => { window.removeEventListener("keydown", onKey); if (previous?.isConnected) previous.focus(); };
  }, []);

  return (
    <div className="dialog-backdrop">
      <div ref={dialog} role="dialog" aria-modal="true" aria-labelledby="settings-title" className="settings">
        <header className="settings-head">
          <h2 id="settings-title"><Flame size={17} aria-hidden="true" /><span className="settings-brand" aria-hidden="true">Vatra / </span>{copy.settings}</h2>
          <button type="button" className="icon" aria-label={copy.close} title={copy.close} onClick={onClose}>
            <X size={16} aria-hidden="true" />
          </button>
        </header>
        <div className="settings-layout">
          <nav className="settings-nav" aria-label={copy.settings}>
            <div className="settings-search"><Search size={14} aria-hidden="true" /><input type="search" aria-label={ro ? "Caută în setări" : "Search settings"} placeholder={ro ? "Caută în setări…" : "Search settings…"} value={query} onChange={(event) => setQuery(event.target.value)} /></div>
            {groups.map((group) => {
              const pages = group.pages.filter(matches);
              return pages.length ? <div className="settings-nav-group" key={group.title}>
                <h3>{group.title}</h3>
                {pages.map((item) => { const Icon = icons[item.id]; return <button key={item.id} type="button" aria-current={page === item.id ? "page" : undefined} aria-controls="settings-content" onClick={() => setPage(item.id)}><Icon size={15} aria-hidden="true" />{item.title}</button>; })}
              </div> : null;
            })}
            {!groups.some((group) => group.pages.some(matches)) ? <p role="status" className="help">{ro ? "Nicio setare găsită." : "No matching settings."}</p> : null}
          </nav>
          <div className="settings-body" id="settings-content" role="region" aria-label={pageTitle} key={page} tabIndex={0}>
          <h2 className="settings-page-title">{pageTitle}</h2>
          {page === "memory" ? <MemoryPage /> : page === "limits" ? <LimitsPage /> : page === "status" ? <StatusPage /> : page === "accounts" ? <AccountsPage /> : page === "mcp-installed" ? <InstalledMcpPage /> : page === "skills" ? <SkillsPage /> : page === "acp-market" ? <RegistryPage ecosystem="acp" /> : page === "mcp-market" ? <McpMarketPage onManage={() => setPage("mcp-installed")} /> : page === "agents" ? <AgentsPage /> : page === "account" ? <ChatgptAccount /> : page === "import" ? <ImportPage onMarket={() => setPage("market")} /> : page === "market" ? <MarketPage onManage={() => setPage("extensions")} /> : page === "extensions" ? <ExtensionsPage /> : page === "models" ? <ModelsPage /> : <>
          <section>
            <h3>{copy.appearance}</h3>
            <Row label={copy.language}>
              <select value={preferences.language} onChange={(event) => set({ language: event.target.value as Language })}>
                {LANGUAGES.map((language) => (
                  <option key={language.id} value={language.id}>
                    {language.label}
                  </option>
                ))}
              </select>
            </Row>
            <Row label={copy.mode}>
              <select value={preferences.mode} onChange={(event) => set({ mode: event.target.value as Preferences["mode"] })}>
                <option value="system">{copy.modeSystem}</option>
                <option value="light">{copy.modeLight}</option>
                <option value="dark">{copy.modeDark}</option>
              </select>
            </Row>
            <ThemeSelect label={copy.lightTheme} value={preferences.lightTheme} onChange={(lightTheme) => set({ lightTheme })} />
            <ThemeSelect label={copy.darkTheme} value={preferences.darkTheme} onChange={(darkTheme) => set({ darkTheme })} />
            <div className="setting">
              <span>{copy.accent}</span>
              <span className="accent-row">
                {preferences.accent !== null ? (
                  <button type="button" className="link" onClick={() => set({ accent: null })}>
                    {copy.accentDefault}
                  </button>
                ) : null}
                <input type="color" aria-label={copy.accent} value={preferences.accent ?? "#d0213b"} onChange={(event) => set({ accent: event.target.value })} />
              </span>
            </div>
            <Row label={copy.fontScale}>
              <input
                type="range"
                min="0.85"
                max="1.4"
                step="0.05"
                value={preferences.fontScale}
                onChange={(event) => set({ fontScale: Number(event.target.value) })}
              />
            </Row>
            <Row label={copy.density}>
              <select value={preferences.density} onChange={(event) => set({ density: event.target.value as Preferences["density"] })}>
                <option value="comfortable">{copy.densityComfortable}</option>
                <option value="compact">{copy.densityCompact}</option>
              </select>
            </Row>
          </section>
          <section>
            <h3>{copy.keyboard}</h3>
            <Row label={copy.sendShortcut}>
              <select value={preferences.sendShortcut} onChange={(event) => set({ sendShortcut: event.target.value as Preferences["sendShortcut"] })}>
                <option value="enter">Enter</option>
                <option value="mod+enter">Ctrl Enter</option>
              </select>
            </Row>
            {COMMAND_IDS.map((command) => (
              <ShortcutRow key={command} command={command} />
            ))}
          </section>
          <section>
            <h3>{copy.service}</h3>
            <p className="help">{copy.restartServiceHelp}</p>
            <button type="button" onClick={() => { useAppStore.setState({ setupRequested: true, settingsPage: null }); onClose(); }}>{copy.onboarding.reopen}</button>
            <UpdateStatus />
            <div>
              <button type="button" className="quiet" onClick={onRestartService}>
                {copy.restartService}
              </button>
            </div>
          </section>
          <div>
            <button type="button" className="link" onClick={resetPreferences}>
              {copy.resetDefaults}
            </button>
          </div>
          </>}
          </div>
        </div>
      </div>
    </div>
  );
}
