import { capText } from "../conversation/group";
import type { DiffLine } from "../conversation/diff";
import { copy } from "../copy";

// A long edit would otherwise become thousands of nodes, which freezes the window.
const MAX_LINES = 400;
const SIGNS: Record<DiffLine["kind"], string> = { same: " ", add: "+", remove: "-" };

export function DiffView({ lines }: { lines: DiffLine[] }) {
  const shown = lines.slice(0, MAX_LINES);
  return (
    <div className="diff" data-testid="diff">
      {shown.map((line, index) => (
        <div key={index} className={`diff-line diff-${line.kind}`}>
          <span className="diff-sign" aria-hidden="true">
            {SIGNS[line.kind]}
          </span>
          <span className="diff-text">{capText(line.text)}</span>
        </div>
      ))}
      {lines.length > MAX_LINES ? <div className="diff-more">{copy.moreLines(lines.length - MAX_LINES)}</div> : null}
    </div>
  );
}
