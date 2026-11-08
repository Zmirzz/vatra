import {Check, CircleAlert, X} from "lucide-react";
import {create} from "zustand";

type Toast = {id: number; text: string; kind: "ok" | "error"};

type ToastState = {toasts: Toast[]; push: (text: string, kind?: Toast["kind"]) => void; dismiss: (id: number) => void};

let next = 1;

/** Short messages that confirm something happened and go away by themselves. */
export const useToasts = create<ToastState>((set, get) => ({
  toasts: [],
  push: (text, kind = "ok") => {
    const id = next++;
    set((s) => ({toasts: [...s.toasts.slice(-3), {id, text, kind}]}));
    setTimeout(() => get().dismiss(id), kind === "error" ? 7000 : 3800);
  },
  dismiss: (id) => set((s) => ({toasts: s.toasts.filter((t) => t.id !== id)})),
}));

export const toast = (text: string, kind: Toast["kind"] = "ok") => useToasts.getState().push(text, kind);

export function ToastHost() {
  const toasts = useToasts((s) => s.toasts);
  const dismiss = useToasts((s) => s.dismiss);
  return (
    <div className="toast-host" role="region" aria-label="Notifications" aria-live="polite">
      {toasts.map((t) => (
        <div key={t.id} className="toast" data-kind={t.kind} role={t.kind === "error" ? "alert" : "status"}>
          {t.kind === "error" ? <CircleAlert size={15} aria-hidden="true" /> : <Check size={15} aria-hidden="true" />}
          <span>{t.text}</span>
          <button type="button" aria-label="Dismiss" onClick={() => dismiss(t.id)}><X size={13} aria-hidden="true" /></button>
        </div>
      ))}
    </div>
  );
}
