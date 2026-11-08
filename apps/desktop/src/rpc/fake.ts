import { RpcClient, type SocketLike } from "./client";

/** A service stand-in for tests: answers calls from a table and records what it was asked. */
export class FakeService {
  sockets: SocketLike[] = [];
  sent: { method: string; params: Record<string, unknown> }[] = [];
  retryDelays: number[] = [];
  results: Record<string, unknown> = {
    "import.scan": { items: [], alternatives: [], conflicts: [], problems: [] },
    "import.apply": { results: [] },
    "import.report": null,
    "import.history": [],
    "market.list": [],
    "agents.configured": { agents: [], summarizer: null },
    "catalog.list": [],
    "mcp.status": [],
    "history.search": [],
  };
  failures: Record<string, { code: number; message: string; data?:unknown }> = {};
  silent = false;
  scheduled: (() => void)[] = [];

  client = new RpcClient("ws://test", "token", {
    socketFactory: () => {
      const socket: SocketLike = {
        onopen: null,
        onclose: null,
        onmessage: null,
        send: (data) => this.receive(socket, data),
        close: () => socket.onclose?.(),
      };
      this.sockets.push(socket);
      return socket;
    },
    schedule: (callback, delayMs) => {
      this.retryDelays.push(delayMs);
      this.scheduled.push(callback);
    },
  });

  private receive(socket: SocketLike, data: string): void {
    const { id, method, params } = JSON.parse(data);
    this.sent.push({ method, params });
    if (this.silent) {
      return;
    }
    const failure = this.failures[method];
    if (failure) {
      delete this.failures[method];
      socket.onmessage?.({ data: JSON.stringify({ id, error: failure }) });
      return;
    }
    const result = method in this.results ? this.results[method] : [];
    socket.onmessage?.({ data: JSON.stringify({ id, result }) });
  }

  get socket(): SocketLike {
    return this.sockets[this.sockets.length - 1];
  }

  open(): void {
    this.client.connect();
    this.socket.onopen?.();
  }

  drop(): void {
    this.socket.onclose?.();
  }

  reconnect(): void {
    this.scheduled.shift()?.();
    this.socket.onopen?.();
  }

  notify(method: string, params: unknown): void {
    this.socket.onmessage?.({ data: JSON.stringify({ method, params }) });
  }

  calls(method: string): Record<string, unknown>[] {
    return this.sent.filter((entry) => entry.method === method).map((entry) => entry.params);
  }
}

export const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
