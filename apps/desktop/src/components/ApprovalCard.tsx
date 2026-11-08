import { useState } from "react";
import { answerApproval } from "../conversation/answer";
import { approvalHeading, exactCommand, extraArguments, visibleText } from "../conversation/labels";
import { copy } from "../copy";
import { useClient } from "../rpc/context";
import type { ApprovalRequest } from "../rpc/types";

export function ApprovalCard({ request, agent }: { request: ApprovalRequest; agent: string }) {
  const client = useClient();
  const [error, setError] = useState<string | null>(null);
  const command = exactCommand(request.arguments);
  const extras = command === null ? {} : extraArguments(request.arguments);

  const answer = async (allow: boolean) => {
    setError(null);
    try {
      await answerApproval(client, request.id, allow);
    } catch (caught) {
      setError((caught as Error).message);
    }
  };

  return (
    <div className="approval">
      <strong>{approvalHeading(agent, request.tool)}</strong>
      {command !== null ? (
        <>
          <pre data-testid="approval-command">{visibleText(command)}</pre>
          {Object.keys(extras).length > 0 ? <pre data-testid="approval-arguments">{JSON.stringify(extras, null, 2)}</pre> : null}
        </>
      ) : (
        <pre data-testid="approval-arguments">{JSON.stringify(request.arguments, null, 2)}</pre>
      )}
      {error ? <div role="alert">{error}</div> : null}
      <div className="approval-actions">
        <button type="button" className="approve" onClick={() => answer(true)}>
          {copy.approvals.allow}
        </button>
        <button type="button" className="refuse" onClick={() => answer(false)}>
          {copy.approvals.refuse}
        </button>
      </div>
    </div>
  );
}
