import { Pin, PinOff, Plus, Trash2, Pencil, Check, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { useClient } from "../rpc/context";
import { usePreferences } from "../settings/preferences";

type Item = { id: number; text: string; category: string; pinned: boolean; source: string; created_at: number; updated_at: number };
type Memory = { enabled: boolean; items: Item[]; limit: number; max_chars: number };

const CATEGORIES = ["about", "preferences", "dislikes", "work", "other"] as const;

const text = {
  en: {
    intro: "What your assistants know about you. Every assistant, Claude, Codex or a local model, reads this list, so you only say it once.",
    use: "Let assistants use and add to this list",
    useHelp: "When this is off, nothing here is sent to any assistant and nothing new is saved.",
    add: "Add", placeholder: "Something worth knowing about you…", category: "Kind",
    empty: "Nothing yet. Add a fact above, or just tell an assistant something about yourself and it will offer to remember it.",
    you: "Added by you", by: "Saved by", pin: "Always keep at the top", unpin: "Stop keeping at the top",
    edit: "Edit", remove: "Remove", save: "Save", cancel: "Cancel",
    clear: "Remove everything", clearAsk: "Remove all facts about you? This can't be undone.", clearYes: "Yes, remove all", keep: "Keep them",
    failed: "That didn't work. Try again.",
    count: (n: number, limit: number) => `${n} of ${limit}`,
    kinds: { about: "About you", preferences: "Things you like", dislikes: "Things you dislike", work: "Your work", other: "Other" } as Record<string, string>,
  },
  ro: {
    intro: "Ce știu asistenții tăi despre tine. Toți asistenții, Claude, Codex sau un model local, citesc această listă, așa că o spui o singură dată.",
    use: "Lasă asistenții să folosească și să adauge în această listă",
    useHelp: "Când e oprit, nimic de aici nu se trimite vreunui asistent și nu se salvează nimic nou.",
    add: "Adaugă", placeholder: "Ceva ce merită știut despre tine…", category: "Fel",
    empty: "Încă nimic. Adaugă un fapt mai sus sau spune-i unui asistent ceva despre tine și îl va reține.",
    you: "Adăugat de tine", by: "Salvat de", pin: "Ține-l mereu sus", unpin: "Nu-l mai ține sus",
    edit: "Modifică", remove: "Șterge", save: "Salvează", cancel: "Renunță",
    clear: "Șterge tot", clearAsk: "Ștergi toate faptele despre tine? Nu se poate anula.", clearYes: "Da, șterge tot", keep: "Le păstrez",
    failed: "N-a mers. Încearcă din nou.",
    count: (n: number, limit: number) => `${n} din ${limit}`,
    kinds: { about: "Despre tine", preferences: "Ce îți place", dislikes: "Ce nu-ți place", work: "Munca ta", other: "Altele" } as Record<string, string>,
  },
};

/** The facts every assistant knows about the person: listed, added, changed and removed here. */
export function MemoryPage() {
  const rpc = useClient();
  const language = usePreferences((state) => state.preferences.language);
  const t = language === "ro" ? text.ro : text.en;
  const [memory, setMemory] = useState<Memory | null>(null);
  const [draft, setDraft] = useState("");
  const [kind, setKind] = useState<string>("about");
  const [editing, setEditing] = useState<{ id: number; text: string } | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);
  const [problem, setProblem] = useState(false);

  const call = useCallback(async (method: string, params: Record<string, unknown> = {}) => {
    try { setMemory(await rpc.call<Memory>(method, params)); setProblem(false); return true; } catch { setProblem(true); return false; }
  }, [rpc]);

  useEffect(() => {
    void call("memory.list");
    // An assistant can save a fact while this page is open.
    const timer = setInterval(() => { if (!editing) void call("memory.list"); }, 4000);
    return () => clearInterval(timer);
  }, [call, editing]);

  const add = async () => {
    const value = draft.trim();
    if (!value) return;
    if (await call("memory.add", { text: value, category: kind })) setDraft("");
  };

  const items = memory?.items ?? [];
  return (
    <section className="memory-page">
      <p className="help">{t.intro}</p>
      <label className="memory-switch">
        <input type="checkbox" role="switch" checked={memory?.enabled ?? true} onChange={(event) => void call("memory.set_enabled", { enabled: event.target.checked })} />
        <span><b>{t.use}</b><small>{t.useHelp}</small></span>
      </label>

      <form className="memory-add" onSubmit={(event) => { event.preventDefault(); void add(); }}>
        <input aria-label={t.placeholder} value={draft} maxLength={memory?.max_chars ?? 400} placeholder={t.placeholder} onChange={(event) => setDraft(event.target.value)} />
        <select aria-label={t.category} value={kind} onChange={(event) => setKind(event.target.value)}>
          {CATEGORIES.map((id) => <option key={id} value={id}>{t.kinds[id]}</option>)}
        </select>
        <button type="submit" className="primary" disabled={!draft.trim()}><Plus size={14} aria-hidden="true" />{t.add}</button>
      </form>
      {problem ? <p role="alert">{t.failed}</p> : null}

      {memory && items.length === 0 ? <p className="memory-empty">{t.empty}</p> : null}
      {CATEGORIES.map((id) => {
        const group = items.filter((item) => item.category === id);
        if (group.length === 0) return null;
        return (
          <section key={id} className="memory-group" aria-label={t.kinds[id]}>
            <h3>{t.kinds[id]}</h3>
            <ul>
              {group.map((item) => (
                <li key={item.id} className="memory-item" data-pinned={item.pinned}>
                  {editing?.id === item.id ? (
                    <form className="memory-edit" onSubmit={async (event) => { event.preventDefault(); if (await call("memory.update", { id: item.id, text: editing.text })) setEditing(null); }}>
                      <input autoFocus aria-label={t.edit} value={editing.text} maxLength={memory?.max_chars ?? 400} onChange={(event) => setEditing({ id: item.id, text: event.target.value })} />
                      <button type="submit" className="icon" aria-label={t.save} title={t.save}><Check size={15} aria-hidden="true" /></button>
                      <button type="button" className="icon" aria-label={t.cancel} title={t.cancel} onClick={() => setEditing(null)}><X size={15} aria-hidden="true" /></button>
                    </form>
                  ) : (
                    <>
                      <div className="memory-text">
                        <span>{item.text}</span>
                        <small>{item.source === "you" ? t.you : `${t.by} ${item.source}`}</small>
                      </div>
                      <div className="memory-actions">
                        <button type="button" className="icon" aria-pressed={item.pinned} aria-label={item.pinned ? t.unpin : t.pin} title={item.pinned ? t.unpin : t.pin} onClick={() => void call("memory.update", { id: item.id, pinned: !item.pinned })}>
                          {item.pinned ? <PinOff size={15} aria-hidden="true" /> : <Pin size={15} aria-hidden="true" />}
                        </button>
                        <button type="button" className="icon" aria-label={`${t.edit}: ${item.text}`} title={t.edit} onClick={() => setEditing({ id: item.id, text: item.text })}><Pencil size={15} aria-hidden="true" /></button>
                        <button type="button" className="icon remove" aria-label={`${t.remove}: ${item.text}`} title={t.remove} onClick={() => void call("memory.delete", { id: item.id })}><Trash2 size={15} aria-hidden="true" /></button>
                      </div>
                    </>
                  )}
                </li>
              ))}
            </ul>
          </section>
        );
      })}

      {items.length > 0 ? (
        <footer className="memory-footer">
          <span className="help">{t.count(items.length, memory?.limit ?? 300)}</span>
          {confirmClear ? (
            <div className="line-confirm" role="alertdialog" aria-label={t.clearAsk}>
              <span>{t.clearAsk}</span>
              <div className="row">
                <button type="button" onClick={() => setConfirmClear(false)}>{t.keep}</button>
                <button type="button" className="danger" onClick={async () => { await call("memory.clear"); setConfirmClear(false); }}>{t.clearYes}</button>
              </div>
            </div>
          ) : <button type="button" className="quiet" onClick={() => setConfirmClear(true)}>{t.clear}</button>}
        </footer>
      ) : null}
    </section>
  );
}
