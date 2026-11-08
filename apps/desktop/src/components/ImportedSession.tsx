import { copy } from "../copy";
import type { ImportedConversation } from "../rpc/types";
import { Markdown } from "./Markdown";
export function ImportedSession({ session }: { session: ImportedConversation }) {
  return <div className="conversation"><div className="column">
    <p className="help">{copy.transfer.readOnly} · {session.origin}</p>
    <h2>{session.title}</h2>{session.cwd ? <p className="help">{session.cwd}</p> : null}
    {session.messages.map((message, index) => <article key={index} className="imported-message"><small className="help">{message.role === "user" ? copy.transfer.user : message.role === "assistant" ? copy.transfer.assistant : message.role}{message.at ? ` · ${message.at}` : ""}</small>{message.role === "user" ? <div className="you">{message.text}</div> : <Markdown text={message.text} />}</article>)}
  </div></div>;
}
