import { useState } from "react";
import { copy } from "../copy";
export function ExtensionNotice({ reasons }: { reasons: string[] }) {
  const [open, setOpen] = useState(false);
  if (reasons.length === 0) return null;
  return <div className="notice notice-warning" role="status">{copy.transfer.limited} <button type="button" className="link" aria-expanded={open} onClick={() => setOpen(!open)}>{copy.details}</button>{open ? <ul>{[...new Set(reasons)].map((reason) => <li key={reason}>{reason}</li>)}</ul> : null}</div>;
}
