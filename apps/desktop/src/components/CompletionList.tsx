import { copy } from "../copy";
import type { CompletionItem } from "../rpc/types";

type Props = {
  items: CompletionItem[];
  active: number;
  onPick: (item: CompletionItem) => void;
};

/** The suggestions above the message box. The field keeps the focus, so a pick is made on mouse down. */
export function CompletionList({ items, active, onPick }: Props) {
  return (
    <div role="listbox" aria-label={copy.completionList} className="completion">
      {items.map((item, index) => (
        <div
          key={`${item.kind}:${item.insert}`}
          role="option"
          aria-selected={index === active}
          className="completion-item"
          onMouseDown={(event) => {
            event.preventDefault();
            onPick(item);
          }}
        >
          <span className="completion-label">{item.label}</span>
          <span className="completion-kind">{copy.completionKind[item.kind]}</span>
          {item.detail ? <span className="completion-detail">{item.detail}</span> : null}
        </div>
      ))}
    </div>
  );
}
