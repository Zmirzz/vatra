import type { RpcClient } from "../rpc/client";

// After an answer, the next request moves into the same spot under the cursor. A double click
// must not approve something that was never read, so a pause follows every answer.
const PAUSE_AFTER_ANSWER_MS = 600;

let lastAnswerAt = Number.NEGATIVE_INFINITY;

export function resetAnswerGuard(): void {
  lastAnswerAt = Number.NEGATIVE_INFINITY;
}

export async function answerApproval(client: RpcClient, id: string, allow: boolean): Promise<void> {
  if (performance.now() - lastAnswerAt < PAUSE_AFTER_ANSWER_MS) {
    return;
  }
  lastAnswerAt = performance.now();
  try {
    await client.call("approvals.resolve", { id, allow });
  } catch (error) {
    resetAnswerGuard();
    throw error;
  }
}
