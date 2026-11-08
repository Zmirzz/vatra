import { copy } from "../copy";

const STORAGE_KEY = "vatra.subscription-notice";

export function hasAcceptedSubscriptionNotice(agent="claude"): boolean {
  try {
    return localStorage.getItem(agent==="claude"?STORAGE_KEY:`${STORAGE_KEY}:${agent}`) === "accepted";
  } catch {
    return false;
  }
}

function rememberAccepted(agent:string): void {
  try {
    localStorage.setItem(agent==="claude"?STORAGE_KEY:`${STORAGE_KEY}:${agent}`, "accepted");
  } catch {
    // Without storage the notice just appears again next time.
  }
}

export function SubscriptionNotice({ onAccept, onCancel,agent="claude" }: { onAccept: () => void; onCancel: () => void;agent?:string }) {
  return (
    <div className="dialog-backdrop">
      <div role="dialog" aria-modal="true" aria-labelledby="subscription-title" className="dialog">
        <h2 id="subscription-title">{agent==="claude"?copy.subscriptionTitle:copy.accountNoticeTitle}</h2>
        <p>{agent==="claude"?copy.subscriptionBody:copy.accountNoticeBody}</p>
        <div className="dialog-actions">
          <button type="button" className="quiet" onClick={onCancel}>
            {copy.cancel}
          </button>
          <button
            type="button"
            className="primary"
            onClick={() => {
              rememberAccepted(agent);
              onAccept();
            }}
          >
            {copy.subscriptionAccept}
          </button>
        </div>
      </div>
    </div>
  );
}
