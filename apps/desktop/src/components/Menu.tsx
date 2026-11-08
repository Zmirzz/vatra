import { ChevronDown } from "lucide-react";
import { useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from "react";

type Option = { value: string; label: string };
type Action = { label: string; onSelect: () => void };

type Props = {
  label: string;
  value: string;
  options: Option[];
  onSelect: (value: string) => void;
  icon?: ReactNode;
  actions?: Action[];
  align?: "start" | "end";
};

const ITEM_SELECTOR = 'button[role="menuitemradio"], button[role="menuitem"]';

export function Menu({ label, value, options, onSelect, icon, actions = [], align = "start" }: Props) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const chip = useRef<HTMLButtonElement>(null);
  const popup = useRef<HTMLDivElement>(null);
  const current = options.find((option) => option.value === value)?.label ?? value;

  useEffect(() => {
    if (!open) {
      return;
    }
    // The keyboard starts on the chosen item, or the first one, so arrow keys work at once.
    const items = popupItems(popup.current);
    (items.find((item) => item.getAttribute("aria-checked") === "true") ?? items[0])?.focus();
    const onPointer = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        setOpen(false);
        chip.current?.focus();
      }
    };
    document.addEventListener("pointerdown", onPointer);
    window.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("pointerdown", onPointer);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [open]);

  const choose = (run: () => void) => {
    setOpen(false);
    chip.current?.focus();
    run();
  };

  const onPopupKey = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const items = popupItems(popup.current);
    const index = items.indexOf(document.activeElement as HTMLButtonElement);
    const move = (next: number) => {
      event.preventDefault();
      items[(next + items.length) % items.length]?.focus();
    };
    if (event.key === "ArrowDown") move(index + 1);
    else if (event.key === "ArrowUp") move(index - 1);
    else if (event.key === "Home") move(0);
    else if (event.key === "End") move(items.length - 1);
    else if (event.key === "Tab") setOpen(false);
  };

  return (
    <div className="menu" ref={root}>
      <button
        ref={chip}
        type="button"
        className="chip"
        aria-label={`${label}: ${current}`}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        {icon}
        {current}
        <ChevronDown size={13} aria-hidden="true" />
      </button>
      {open ? (
        <div ref={popup} role="menu" className="menu-popup" data-align={align} onKeyDown={onPopupKey}>
          {options.map((option) => (
            <button
              key={option.value}
              type="button"
              role="menuitemradio"
              aria-checked={option.value === value}
              className="menu-item"
              onClick={() => choose(() => onSelect(option.value))}
            >
              {option.label}
            </button>
          ))}
          {actions.map((action) => (
            <button key={action.label} type="button" role="menuitem" className="menu-item menu-action" onClick={() => choose(action.onSelect)}>
              {action.label}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

function popupItems(popup: HTMLDivElement | null): HTMLButtonElement[] {
  return popup ? Array.from(popup.querySelectorAll<HTMLButtonElement>(ITEM_SELECTOR)) : [];
}
