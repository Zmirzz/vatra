import { useState } from "react";
import { agentLabel } from "../conversation/labels";
import { copy } from "../copy";
import { useClient } from "../rpc/context";
import type { PermissionMode, TurnOptions } from "../rpc/types";
import { openSession, refreshSessions,useAppStore } from "../state/store";
import { hasAcceptedSubscriptionNotice, SubscriptionNotice } from "./SubscriptionNotice";

type Props = { sessionId: number; to: string; mode: PermissionMode; onClose: () => void; options?: TurnOptions };

export function HandoffDialog({ sessionId, to, mode, onClose, options = {} }: Props) {
  const client = useClient();
  const subscriptionAgents=useAppStore(state=>state.subscriptionAgents);
  const [task, setTask] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [noticeOpen, setNoticeOpen] = useState(false);

  const start = async () => {
    setError(null);
    try {
      const { session_id } = await client.call<{ session_id: number }>("handoff.start", {
        from: sessionId,
        to,
        task: task.trim(),
        mode,
        ...options,
      });
      useAppStore.setState((state) => ({ turnOptions: { ...state.turnOptions, [session_id]: options } }));
      await refreshSessions(client);
      openSession(client, session_id);
      onClose();
    } catch (caught) {
      setError((caught as Error).message);
    }
  };

  const submit = () => {
    if (!task.trim()) {
      setError(copy.handoffTaskRequired);
    } else if (subscriptionAgents.includes(to) && !hasAcceptedSubscriptionNotice(to)) {
      setNoticeOpen(true);
    } else {
      void start();
    }
  };

  return (
    <div className="dialog-backdrop">
      <div role="dialog" aria-modal="true" aria-labelledby="handoff-title" className="dialog">
        <h2 id="handoff-title">{copy.handoffTitle(agentLabel(to))}</h2>
        <label>
          {copy.handoffTask}
          <textarea rows={3} value={task} autoFocus onChange={(event) => setTask(event.target.value)} />
        </label>
        {error ? <div role="alert">{error}</div> : null}
        <div className="dialog-actions">
          <button type="button" className="quiet" onClick={onClose}>
            {copy.cancel}
          </button>
          <button type="button" className="primary" onClick={submit}>
            {copy.handoffStart}
          </button>
        </div>
      </div>
      {noticeOpen ? (
        <SubscriptionNotice
          agent={to}
          onCancel={() => setNoticeOpen(false)}
          onAccept={() => {
            setNoticeOpen(false);
            void start();
          }}
        />
      ) : null}
    </div>
  );
}
