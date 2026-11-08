import { beforeEach, expect, test } from "vitest";
import { setLanguage } from "../copy";
import { attachClient, initialState, openSession, useAppStore } from "../state/store";
import { RpcClient, RpcError, type SocketLike } from "./client";
import { FakeService, flush } from "./fake";
import type { StoredEvent } from "./types";

const stored = (seq: number): StoredEvent => ({ seq, session_id: 7, event: { type: "assistant_text", text: `t${seq}` } });

beforeEach(() => useAppStore.setState(initialState()));

test("a call resolves with the result and rejects with the service error", async () => {
  const service = new FakeService();
  service.open();
  service.results["agents.list"] = ["local"];
  await expect(service.client.call("agents.list", {})).resolves.toEqual(["local"]);
  service.failures["turn.start"] = { code: 1001, message: "ocupată" };
  const error = (await service.client.call("turn.start", {}).catch((caught: unknown) => caught)) as RpcError;
  expect(error).toBeInstanceOf(RpcError);
  expect(error.code).toBe(1001);
});

test("the token travels as a subprotocol, never in the url", () => {
  const seen: { url: string; protocols: string[] }[] = [];
  const service = new FakeService();
  const factory = (service.client as unknown as { socketFactory: (url: string, protocols: string[]) => unknown });
  const original = factory.socketFactory;
  factory.socketFactory = (url, protocols) => {
    seen.push({ url, protocols });
    return original(url, protocols);
  };
  service.open();
  expect(seen[0].url).not.toContain("token");
  expect(seen[0].protocols).toEqual(["vatra.bearer.token"]);
});

test("reconnect asks for events after the last seq it saw", async () => {
  const service = new FakeService();
  attachClient(service.client);
  useAppStore.setState({ eventsBySession: { 7: [stored(10)] }, loadedSessions: [7] });
  service.open();
  await flush();
  service.sent = [];
  service.drop();
  service.reconnect();
  await flush();
  expect(service.calls("sessions.events")).toContainEqual({ session_id: 7, after_seq: 10 });
  expect(service.calls("sessions.list")).toHaveLength(1);
  expect(service.calls("approvals.list")).toHaveLength(1);
});

test("events recovered after a reconnect aren't duplicated", async () => {
  const service = new FakeService();
  attachClient(service.client);
  useAppStore.setState({ eventsBySession: { 7: [stored(10)] }, loadedSessions: [7] });
  service.results["sessions.events"] = [stored(10), stored(11)];
  service.open();
  await flush();
  expect(useAppStore.getState().eventsBySession[7].map((event) => event.seq)).toEqual([10, 11]);
});

test("reconnect backs off and pending calls reject instead of hanging", async () => {
  const service = new FakeService();
  service.open();
  service.silent = true;
  const pending = service.client.call("agents.list", {});
  service.drop();
  await expect(pending).rejects.toThrow();
  service.reconnect();
  service.drop();
  service.scheduled.shift()?.();
  service.drop();
  expect(service.retryDelays[0]).toBe(250);
  expect(service.retryDelays[1]).toBeLessThanOrEqual(service.retryDelays[2]);
  expect(service.retryDelays.every((delay) => delay <= 10_000)).toBe(true);
});

test("a failed attempt keeps backing off instead of retrying instantly", () => {
  const service = new FakeService();
  service.client.connect();
  service.drop();
  service.scheduled.shift()?.();
  service.drop();
  expect(service.retryDelays).toEqual([250, 500]);
});

test("status follows the connection", () => {
  const service = new FakeService();
  const seen: string[] = [];
  service.client.onStatus((status) => seen.push(status));
  service.open();
  service.drop();
  service.reconnect();
  expect(seen).toEqual(["connecting", "connected", "reconnecting", "reconnecting", "connected"]);
});

test("garbage from the socket is ignored", () => {
  const service = new FakeService();
  service.open();
  expect(() => service.socket.onmessage?.({ data: "nu e json" })).not.toThrow();
});

test("calling without a connection rejects at once", async () => {
  const service = new FakeService();
  await expect(service.client.call("agents.list", {})).rejects.toThrow();
});

test("a restarted service with a new port and token is found again", async () => {
  const seen: { url: string; protocols: string[] }[] = [];
  const sockets: SocketLike[] = [];
  const scheduled: (() => void)[] = [];
  let target = { url: "ws://127.0.0.1:1/rpc", token: "vechi" };
  const client = new RpcClient(target.url, target.token, {
    socketFactory: (url, protocols) => {
      seen.push({ url, protocols });
      const socket: SocketLike = { onopen: null, onclose: null, onmessage: null, send: () => undefined, close: () => undefined };
      sockets.push(socket);
      return socket;
    },
    schedule: (callback) => scheduled.push(callback),
    refreshTarget: async () => target,
  });
  client.connect();
  sockets[0].onopen?.();

  target = { url: "ws://127.0.0.1:2/rpc", token: "nou" };
  sockets[0].onclose?.();
  scheduled.shift()?.();
  await flush();
  expect(seen.at(-1)).toEqual({ url: "ws://127.0.0.1:2/rpc", protocols: ["vatra.bearer.nou"] });
});

test("a failed refresh keeps retrying instead of giving up", async () => {
  const scheduled: (() => void)[] = [];
  const sockets: SocketLike[] = [];
  const client = new RpcClient("ws://x", "t", {
    socketFactory: () => {
      const socket: SocketLike = { onopen: null, onclose: null, onmessage: null, send: () => undefined, close: () => undefined };
      sockets.push(socket);
      return socket;
    },
    schedule: (callback) => scheduled.push(callback),
    refreshTarget: async () => {
      throw new Error("serviciul nu pornește");
    },
  });
  client.connect();
  sockets[0].onclose?.();
  scheduled.shift()?.();
  await flush();
  expect(scheduled).toHaveLength(1);
});

test("opening a session that already received live events still loads its history", async () => {
  const service = new FakeService();
  attachClient(service.client);
  service.results["sessions.events"] = [stored(1), stored(2)];
  service.open();
  await flush();
  service.notify("event", stored(5));
  openSession(service.client, 7);
  await flush();
  expect(service.calls("sessions.events")).toContainEqual({ session_id: 7, after_seq: 0 });
  expect(useAppStore.getState().eventsBySession[7].map((event) => event.seq)).toEqual([1, 2, 5]);
});

test("reopening a loaded session doesn't reload it", async () => {
  const service = new FakeService();
  attachClient(service.client);
  service.open();
  openSession(service.client, 7);
  await flush();
  service.sent = [];
  openSession(service.client, 7);
  await flush();
  expect(service.calls("sessions.events")).toHaveLength(0);
});

test("a live event arriving during the resync doesn't hide what was missed", async () => {
  const service = new FakeService();
  attachClient(service.client);
  useAppStore.setState({ eventsBySession: { 7: [stored(10)] }, loadedSessions: [7] });
  service.open();
  await flush();
  service.sent = [];
  service.drop();
  service.scheduled.shift()?.();
  service.notify("event", stored(20));
  service.socket.onopen?.();
  await flush();
  expect(service.calls("sessions.events")).toContainEqual({ session_id: 7, after_seq: 10 });
});

test("connection errors follow the chosen language", async () => {
  const service = new FakeService();
  setLanguage("en");
  await expect(service.client.call("agents.list", {})).rejects.toThrow("isn't connected");
  setLanguage("ro");
  await expect(service.client.call("agents.list", {})).rejects.toThrow("Nu există conexiune");
  setLanguage("en");
});
