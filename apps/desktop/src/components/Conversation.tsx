import { useEffect, useMemo, useRef, type ReactNode } from "react";
import { capText, groupEvents, type Block } from "../conversation/group";
import { agentLabel } from "../conversation/labels";
import { copy } from "../copy";
import { useAppStore } from "../state/store";
import { Mark } from "./Mark";
import { Markdown } from "./Markdown";
import { WorkGroup } from "./WorkGroup";
import { ExtensionNotice } from "./ExtensionNotice";
import { verdictText } from "./RouteMarks";
import {ConversationFiles,MessageActions} from "../files/ConversationFiles";
import {MessageAttachments} from "./MessageAttachment";

const STICK_TO_BOTTOM_PX = 80;
const NO_PROBLEMS: string[] = [];

function quietLine(event: Extract<Block, { kind: "quiet" }>["event"]): ReactNode {
  switch (event.type) {
    case "compacted":
      return (
        <div className="quiet">
          {copy.quiet.compacted}
          <details>
            <summary>{copy.details}</summary>
            <pre>{capText(event.summary)}</pre>
          </details>
        </div>
      );
    case "background_result":
      return (
        <div className="quiet">
          {copy.quiet.background}
          <details>
            <summary>{copy.details}</summary>
            <pre>{capText(event.text)}</pre>
          </details>
        </div>
      );
    case "skill_loaded":
      return (
        <div className="quiet">
          {copy.quiet.skill(event.name)}
          <details>
            <summary>{copy.details}</summary>
            <pre>{capText(event.text)}</pre>
          </details>
        </div>
      );
    case "rule_added":
      return (
        <div className="quiet">
          {copy.quiet.rule(event.label)}
          <details>
            <summary>{copy.details}</summary>
            <pre>{capText(event.text)}</pre>
          </details>
        </div>
      );
  }
}

function renderBlock(block: Block, agent: string): ReactNode {
  switch (block.kind) {
    case "user":
      return (
        <>
          <div className="user-attachments-top">
            <MessageAttachments seq={block.seq} />
            <ConversationFiles seq={block.seq} />
          </div>
          <div className="you">{block.text}</div>
          <div className="user-actions">
            <MessageActions seq={block.seq} text={block.text} user />
          </div>
        </>
      );
    case "assistant":
      return (
        <>
          <div className="message-head">
            <span className="message-model">{agentLabel(agent).replace(/^local:/, "")}</span>
          </div>
          <div className="assistant"><Markdown text={block.text}/></div>
          <div className="user-actions"><MessageActions seq={block.seq} text={block.text} /></div>
        </>
      );
    case "quiet":
      return quietLine(block.event);
    case "work":
      return (
        <>
          <WorkGroup items={block.items} technical={block.technical} />
          <div className="user-actions">
            <ConversationFiles seq={block.seq} toolIds={block.items.map((item) => item.id)} />
            <MessageActions seq={block.seq} text="" />
          </div>
        </>
      );
    case "failure":
      return (
        <div role="alert" className="notice notice-danger">
          {block.message}
        </div>
      );
    case "warning":
      return <div className="notice notice-warning">{block.message}</div>;
    case "limit":
      return <div className="notice notice-warning">{copy.rateLimited}</div>;
    case "handoff":
      return (
        <div className="notice">
          <div>{copy.handedOffTo(agentLabel(block.agent))}</div>
          {block.brief.degraded ? <div className="notice-warning-text">{copy.degradedBrief}</div> : null}
        </div>
      );
    case "handoff_result":
      return (
        <div className="notice">
          <div className="notice-title">{copy.handoffResult}</div>
          <Markdown text={block.text} />
        </div>
      );
    case "technical":
      return (
        <details className="technical">
          <summary>{copy.technicalDetails}</summary>
          <pre>{capText(block.lines.join("\n"))}</pre>
        </details>
      );
  }
}

export function Conversation() {
  const session = useAppStore((state) => state.sessions.find((candidate) => candidate.id === state.activeSessionId));
  const agent = session?.agent ?? "";
  const catalog = useAppStore((state) => state.catalog);
  const limited = useMemo(() => session?.extension_route ? catalog.filter((entry) => entry.enabled && (entry.scope === "global" || typeof entry.scope === "object" && entry.scope.project === session.project_id)).flatMap((entry) => {
    const verdict = entry.routes?.find(([route]) => route === session.extension_route)?.[1];
    return verdict && verdict !== "works" ? [`${entry.name}: ${verdictText(verdict)}`] : [];
  }) : [], [catalog, session]);
  const events = useAppStore((state) => (state.activeSessionId === null ? undefined : state.eventsBySession[state.activeSessionId]));
  const running = useAppStore((state) => state.sessions.find((session) => session.id === state.activeSessionId)?.running ?? false);
  const waiting = useAppStore(
    (state) =>
      state.approvals.some((approval) => approval.session_id === state.activeSessionId) ||
      state.questions.some((request) => request.session_id === state.activeSessionId),
  );
  const streamed = useAppStore((state) => (state.activeSessionId === null ? "" : (state.streaming[state.activeSessionId] ?? "")));
  const problems = useAppStore((state) => (state.activeSessionId === null ? NO_PROBLEMS : (state.problems[state.activeSessionId] ?? NO_PROBLEMS)));
  const blocks = useMemo(() => groupEvents(events ?? []), [events]);
  const scroller = useRef<HTMLDivElement>(null);
  const stuck = useRef(true);

  useEffect(() => {
    if (stuck.current && scroller.current) {
      scroller.current.scrollTop = scroller.current.scrollHeight;
    }
  }, [blocks, running, streamed]);

  return (
    <div
      ref={scroller}
      className="conversation"
      onScroll={(event) => {
        const element = event.currentTarget;
        stuck.current = element.scrollHeight - element.scrollTop - element.clientHeight < STICK_TO_BOTTOM_PX;
      }}
    >
      <div className="column">
        <ExtensionNotice reasons={limited} />
        {blocks.map((block) => (
          <div key={block.seq}>{renderBlock(block, agent)}</div>
        ))}
        {streamed !== "" ? <Markdown text={streamed} /> : null}
        {problems.map((message) => (
          <div key={message} className="notice notice-warning">
            {message}
          </div>
        ))}
        {running && !waiting ? (
          <div role="status" className="working-now">
            <Mark size={16} stitching />
            {copy.working}
          </div>
        ) : null}
      </div>
    </div>
  );
}
