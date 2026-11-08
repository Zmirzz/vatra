import { ChevronRight } from "lucide-react";
import { useState } from "react";
import { capText, type WorkItem } from "../conversation/group";
import { copy } from "../copy";
import { ToolRow } from "./ToolRow";

export function WorkGroup({ items, technical }: { items: WorkItem[]; technical: string[] }) {
  const [open, setOpen] = useState(false);
  const running = items.some((item) => item.output === undefined && !item.denied && !item.interrupted);
  return (
    <div className="work">
      <button type="button" className="work-head" aria-expanded={open} onClick={() => setOpen(!open)}>
        {running ? copy.inProgress(items.length) : copy.worked(items.length)}
        <ChevronRight className="work-chevron" size={13} aria-hidden="true" />
      </button>
      {open ? (
        <div className="work-body">
          {items.map((item) => (
            <ToolRow key={item.id} item={item} />
          ))}
          {technical.length > 0 ? (
            <details className="technical">
              <summary>{copy.technicalDetails}</summary>
              <pre>{capText(technical.join("\n"))}</pre>
            </details>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
