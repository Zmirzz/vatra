import { ArrowUp, Square,Paperclip } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { findTrigger } from "../composer/trigger";
import { copy } from "../copy";
import { useClient } from "../rpc/context";
import type { CompletionItem } from "../rpc/types";
import { usePreferences } from "../settings/preferences";
import { CompletionList } from "./CompletionList";
import {fileCopy} from "../files/copy";

const MAX_HEIGHT_PX = 260;
/** Typing fast sends one request for the last query, not one per key. */
const COMPLETE_DELAY_MS = 80;

type Props = {
  value: string;
  onChange: (value: string) => void;
  onSubmit: () => void;
  placeholder: string;
  rows?: number;
  running?: boolean;
  onStop?: () => void;
  left?: ReactNode;
  right?: ReactNode;
  /** The project whose skills, prompts and files `/`, `$` and `@` offer. */
  projectId?: number;
  onFiles?:(files:File[])=>void;
  attachments?:ReactNode;
  sendDisabled?:boolean;
};

function fitHeight(element: HTMLTextAreaElement): void {
  element.style.height = "auto";
  element.style.height = `${Math.min(element.scrollHeight, MAX_HEIGHT_PX)}px`;
}

export function PromptBox({
  value,
  onChange,
  onSubmit,
  placeholder,
  rows = 2,
  running = false,
  onStop,
  left,
  right,
  projectId = 0,
  onFiles,attachments,sendDisabled=false,
}: Props) {
  const client = useClient();
  const sendShortcut = usePreferences((state) => state.preferences.sendShortcut);
  const field = useRef<HTMLTextAreaElement>(null);
  const fileInput=useRef<HTMLInputElement>(null);
  const [caret, setCaret] = useState(0);
  const [composing, setComposing] = useState(false);
  // Escape closes the list for the trigger it was open on. A new trigger, at another place, opens it again.
  const [dismissedKey, setDismissedKey] = useState("");
  const [list, setList] = useState<{ key: string; items: CompletionItem[] }>({ key: "", items: [] });
  const [active, setActive] = useState(0);
  const [note, setNote] = useState<string | null>(null);
  const requests = useRef(0);
  const pendingCaret = useRef<number | null>(null);
  const lastWidth = useRef<number | null>(null);

  const trigger = composing ? null : findTrigger(value, caret);
  const key = trigger ? `${trigger.trigger}@${trigger.start}` : "";
  const dismissed = key !== "" && dismissedKey === key;

  // Once no trigger is active, the dismissal is over: the next `/` at the start opens the list again.
  useEffect(() => {
    if (key === "") {
      setDismissedKey("");
    }
  }, [key]);
  const items = list.key === key ? list.items : [];
  const open = !dismissed && items.length > 0;
  const triggerChar = trigger?.trigger ?? null;
  const triggerQuery = trigger?.query ?? "";

  useLayoutEffect(() => {
    const element = field.current;
    if (element) {
      fitHeight(element);
      if (pendingCaret.current !== null) {
        element.setSelectionRange(pendingCaret.current, pendingCaret.current);
        pendingCaret.current = null;
      }
    }
  }, [value]);

  // The field is measured again when its width changes, since the text wraps differently at each width.
  useEffect(() => {
    const element = field.current;
    if (!element || typeof ResizeObserver === "undefined") {
      return;
    }
    const observer = new ResizeObserver((entries) => {
      const width = entries[0]?.contentRect.width ?? null;
      if (width !== lastWidth.current) {
        lastWidth.current = width;
        fitHeight(element);
      }
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const id = ++requests.current;
    if (triggerChar === null || dismissed) {
      return;
    }
    const timer = window.setTimeout(() => {
      client
        .call<CompletionItem[]>("composer.complete", { project_id: projectId, trigger: triggerChar, query: triggerQuery })
        .then((found) => {
          // An answer to a query that has since changed is dropped.
          if (requests.current === id) {
            setList({ key, items: found });
            setActive(0);
          }
        })
        .catch(() => {
          if (requests.current === id) {
            setList({ key, items: [] });
          }
        });
    }, COMPLETE_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [client, projectId, key, triggerChar, triggerQuery, dismissed]);

  const pick = (item: CompletionItem) => {
    if (!trigger) {
      return;
    }
    const insert = `${item.insert} `;
    const next = value.slice(0, trigger.start) + insert + value.slice(caret);
    const position = trigger.start + insert.length;
    pendingCaret.current = position;
    setCaret(position);
    onChange(next);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    // While an IME composes, its keys belong to the composition: they neither pick nor send.
    if (event.nativeEvent.isComposing) {
      return;
    }
    if (open) {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        setActive((index) => (index + 1) % items.length);
        return;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        setActive((index) => (index - 1 + items.length) % items.length);
        return;
      }
      if ((event.key === "Enter" && !event.shiftKey) || event.key === "Tab") {
        event.preventDefault();
        pick(items[active]);
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        setDismissedKey(key);
        return;
      }
    }
    if (event.key !== "Enter" || event.shiftKey) {
      return;
    }
    if (event.ctrlKey || event.metaKey || sendShortcut === "enter") {
      event.preventDefault();
      if (running) {
        setNote(copy.enterWhileRunning);
      } else if(!sendDisabled) {
        onSubmit();
      }
    }
  };

  return (
    <div className="prompt" onDragOver={event=>{if(onFiles&&event.dataTransfer.types.includes("Files")){event.preventDefault();event.dataTransfer.dropEffect="copy";}}} onDrop={event=>{if(onFiles&&event.dataTransfer.files.length){event.preventDefault();onFiles(Array.from(event.dataTransfer.files));}}}>
      {attachments}
      {open ? <CompletionList items={items} active={active} onPick={pick} /> : null}
      <textarea
        ref={field}
        aria-label={placeholder}
        placeholder={placeholder}
        rows={rows}
        value={value}
        onPaste={event=>{const files=Array.from(event.clipboardData.items).filter(item=>item.kind==="file").map(item=>item.getAsFile()).filter((file):file is File=>file!==null);if(files.length&&onFiles){event.preventDefault();onFiles(files);}}}
        onChange={(event) => {
          onChange(event.target.value);
          setCaret(event.target.selectionStart);
          setComposing((event.nativeEvent as InputEvent).isComposing === true);
          setNote(null);
        }}
        onSelect={(event) => setCaret(event.currentTarget.selectionStart)}
        onCompositionStart={() => setComposing(true)}
        onCompositionEnd={() => setComposing(false)}
        onKeyDown={onKeyDown}
      />
      {note ? (
        <p role="status" className="help">
          {note}
        </p>
      ) : null}
      <div className="prompt-bar">
        {onFiles?<><input ref={fileInput} hidden type="file" multiple onChange={event=>{if(event.target.files)onFiles(Array.from(event.target.files));event.target.value="";}}/><button className="icon attach-button" type="button" aria-label={fileCopy().attach} title={fileCopy().attach} onClick={()=>fileInput.current?.click()}><Paperclip size={17}/></button></>:null}
        {left}
        <span className="spacer" />
        {right}
        {running ? (
          <button type="button" className="round" aria-label={copy.stop} title={copy.stop} onClick={onStop}>
            <Square size={11} fill="currentColor" aria-hidden="true" />
          </button>
        ) : (
          <button type="button" className="round" aria-label={copy.send} title={copy.send} disabled={sendDisabled} onClick={onSubmit}>
            <ArrowUp size={15} aria-hidden="true" />
          </button>
        )}
      </div>
    </div>
  );
}
