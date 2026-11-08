import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, test } from "vitest";
import { ClientProvider } from "../rpc/context";
import { FakeService, flush } from "../rpc/fake";
import type { QuestionRequest } from "../rpc/types";
import { attachClient, initialState, useAppStore } from "../state/store";
import { Composer } from "./Composer";
import { QuestionCard } from "./QuestionCard";

let service: FakeService;

const request: QuestionRequest = {
  id: "q1",
  session_id: 1,
  questions: [
    {
      question: "Which design?",
      header: "Design",
      options: [{ label: "Compact", description: "Less space" }, { label: "Roomy" }],
    },
    { question: "Which features?", multiSelect: true, options: [{ label: "Search" }, { label: "Export" }] },
  ],
};

const ui = (item: QuestionRequest) => (
  <ClientProvider client={service.client}>
    <QuestionCard request={item} />
  </ClientProvider>
);

beforeEach(() => {
  useAppStore.setState(initialState());
  service = new FakeService();
  service.open();
  service.results["question.answer"] = {};
});

test("answering sends each choice by its position", async () => {
  render(ui(request));
  expect(screen.getByText("Which design?")).toBeTruthy();
  await userEvent.click(screen.getByRole("button", { name: "Compact" }));
  await userEvent.click(screen.getByRole("checkbox", { name: "Search" }));
  await userEvent.click(screen.getByRole("checkbox", { name: "Export" }));
  await userEvent.click(screen.getByRole("button", { name: "Send answer" }));
  await flush();
  expect(service.calls("question.answer")).toEqual([
    { id: "q1", answers: { "Which design?": "Compact", "Which features?": ["Search", "Export"] } },
  ]);
});

test("an empty question list is never sent", () => {
  render(ui({ id: "q3", session_id: 1, questions: [] }));
  expect(screen.getByRole("button", { name: "Send answer" }).hasAttribute("disabled")).toBe(true);
  expect(service.calls("question.answer")).toEqual([]);
});

test("a typed answer is sent instead of the options", async () => {
  render(ui({ id: "q2", session_id: 1, questions: [{ question: "Numele?" }] }));
  await userEvent.type(screen.getByRole("textbox", { name: "Something else" }), "Vatra");
  await userEvent.click(screen.getByRole("button", { name: "Send answer" }));
  await flush();
  expect(service.calls("question.answer")).toEqual([{ id: "q2", answers: { "Numele?": "Vatra" } }]);
});

test("the answer waits until every question has one", () => {
  render(ui(request));
  expect(screen.getByRole("button", { name: "Send answer" }).hasAttribute("disabled")).toBe(true);
});

test("a question still waiting after a reload is shown again", async () => {
  service.results["questions.list"] = [request];
  attachClient(service.client);
  useAppStore.setState({
    sessions: [{ id: 1, project_id: 1, agent: "claude", title: "s", updated_seq: 1, last_turn: "none", running: true }],
    activeSessionId: 1,
  });
  render(
    <ClientProvider client={service.client}>
      <Composer sessionId={1} />
    </ClientProvider>,
  );
  service.open();
  await flush();
  expect(await screen.findByText("Which design?")).toBeTruthy();
});
