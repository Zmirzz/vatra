import { useState } from "react";
import { copy } from "../copy";
import { useClient } from "../rpc/context";
import type { QuestionItem, QuestionRequest } from "../rpc/types";

/** The questions come from the agent unchanged, so anything without its text is skipped. */
export function questionItems(value: unknown): QuestionItem[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value
    .filter((item): item is QuestionItem => typeof item === "object" && item !== null && typeof (item as { question?: unknown }).question === "string")
    .map((item) => ({
      ...item,
      options: Array.isArray(item.options) ? item.options.filter((option) => typeof option?.label === "string") : [],
    }));
}

/** The answer for one question: what the user typed wins, otherwise the chosen options. */
function answerFor(item: QuestionItem, typed: string, chosen: string[]): string | string[] | null {
  if (typed.trim() !== "") {
    return typed.trim();
  }
  if (chosen.length === 0) {
    return null;
  }
  return item.multiSelect ? chosen : chosen[0];
}

export function QuestionCard({ request }: { request: QuestionRequest }) {
  const client = useClient();
  const questions = questionItems(request.questions);
  const [chosen, setChosen] = useState<Record<number, string[]>>({});
  const [typed, setTyped] = useState<Record<number, string>>({});
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const answers = questions.map((item, index) => answerFor(item, typed[index] ?? "", chosen[index] ?? []));
  const complete = questions.length > 0 && answers.every((answer) => answer !== null);

  const choose = (index: number, label: string, multiSelect: boolean) => {
    setChosen((current) => {
      const picked = current[index] ?? [];
      if (!multiSelect) {
        return { ...current, [index]: [label] };
      }
      const next = picked.includes(label) ? picked.filter((value) => value !== label) : [...picked, label];
      return { ...current, [index]: next };
    });
  };

  const send = async () => {
    setError(null);
    setSending(true);
    // Keyed by the question's own text, so the model can tell which answer belongs to which question.
    const payload = Object.fromEntries(questions.map((item, index) => [item.question, answers[index]]));
    try {
      await client.call("question.answer", { id: request.id, answers: payload });
    } catch (caught) {
      setError((caught as Error).message);
    } finally {
      setSending(false);
    }
  };

  return (
    <div className="question-card">
      <strong>{copy.questions.heading}</strong>
      {questions.map((item, index) => {
        const multiSelect = item.multiSelect === true;
        const picked = chosen[index] ?? [];
        return (
          <div key={index} className="question">
            {item.header ? <div className="question-header">{item.header}</div> : null}
            <p className="question-text">{item.question}</p>
            <div className="question-options">
              {(item.options ?? []).map((option, optionIndex) => {
                const hintId = `question-${request.id}-${index}-${optionIndex}`;
                const selected = picked.includes(option.label);
                return (
                  <div key={optionIndex} className="question-option">
                    {multiSelect ? (
                      <label>
                        <input type="checkbox" checked={selected} aria-describedby={option.description ? hintId : undefined} onChange={() => choose(index, option.label, true)} />
                        {option.label}
                      </label>
                    ) : (
                      <button
                        type="button"
                        className="choice"
                        aria-pressed={selected}
                        aria-describedby={option.description ? hintId : undefined}
                        onClick={() => choose(index, option.label, false)}
                      >
                        {option.label}
                      </button>
                    )}
                    {option.description ? (
                      <span id={hintId} className="question-hint">
                        {option.description}
                      </span>
                    ) : null}
                  </div>
                );
              })}
            </div>
            <input
              type="text"
              className="question-typed"
              aria-label={copy.questions.other}
              placeholder={copy.questions.otherPlaceholder}
              value={typed[index] ?? ""}
              onChange={(event) => setTyped((current) => ({ ...current, [index]: event.target.value }))}
            />
          </div>
        );
      })}
      {error ? <div role="alert">{error}</div> : null}
      <div className="approval-actions">
        <button type="button" className="approve" disabled={!complete || sending} onClick={send}>
          {copy.questions.send}
        </button>
      </div>
    </div>
  );
}
