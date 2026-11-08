import { copy } from "../copy";
import type { ConnectionStatus, Notification } from "./types";

export type SocketLike = {
  onopen: (() => void) | null;
  onclose: (() => void) | null;
  onmessage: ((message: { data: string }) => void) | null;
  send(data: string): void;
  close(): void;
};

export type SocketFactory = (url: string, protocols: string[]) => SocketLike;

type Options = {
  socketFactory?: SocketFactory;
  schedule?: (callback: () => void, delayMs: number) => void;
  /** A restarted service has a new port and token, so a reconnect asks where it is now. */
  refreshTarget?: () => Promise<{ url: string; token: string }>;
};

const FIRST_RETRY_MS = 250;
const LAST_RETRY_MS = 10_000;
const PROTOCOL_PREFIX = "vatra.bearer.";

export class RpcError extends Error {
  constructor(
    message: string,
    readonly code: number,
    readonly data?: unknown,
  ) {
    super(message);
  }
}

type Pending = { resolve: (value: never) => void; reject: (error: Error) => void };

export class RpcClient {
  private socket: SocketLike | null = null;
  private nextId = 0;
  private retryMs = FIRST_RETRY_MS;
  private closed = false;
  private pending = new Map<number, Pending>();
  private notificationHandlers = new Set<(notification: Notification) => void>();
  private statusHandlers = new Set<(status: ConnectionStatus) => void>();
  private readonly socketFactory: SocketFactory;
  private readonly schedule: (callback: () => void, delayMs: number) => void;
  private readonly refreshTarget?: () => Promise<{ url: string; token: string }>;

  constructor(
    private url: string,
    private token: string,
    options: Options = {},
  ) {
    this.refreshTarget = options.refreshTarget;
    this.socketFactory = options.socketFactory ?? ((target, protocols) => new WebSocket(target, protocols) as unknown as SocketLike);
    this.schedule = options.schedule ?? ((callback, delayMs) => void setTimeout(callback, delayMs));
  }

  connect(): void {
    this.emitStatus(this.socket === null && this.retryMs === FIRST_RETRY_MS ? "connecting" : "reconnecting");
    const socket = this.socketFactory(this.url, [`${PROTOCOL_PREFIX}${this.token}`]);
    this.socket = socket;
    socket.onopen = () => {
      this.retryMs = FIRST_RETRY_MS;
      this.emitStatus("connected");
    };
    socket.onmessage = (message) => this.handleMessage(message.data);
    socket.onclose = () => this.handleClose(socket);
  }

  close(): void {
    this.closed = true;
    this.socket?.close();
  }

  call<T>(method: string, params: object): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      if (this.socket === null) {
        reject(new RpcError(copy.notConnected, 0));
        return;
      }
      this.nextId += 1;
      this.pending.set(this.nextId, { resolve: resolve as (value: never) => void, reject });
      this.socket.send(JSON.stringify({ jsonrpc: "2.0", id: this.nextId, method, params }));
    });
  }

  on(handler: (notification: Notification) => void): () => void {
    this.notificationHandlers.add(handler);
    return () => this.notificationHandlers.delete(handler);
  }

  onStatus(handler: (status: ConnectionStatus) => void): () => void {
    this.statusHandlers.add(handler);
    return () => this.statusHandlers.delete(handler);
  }

  private emitStatus(status: ConnectionStatus): void {
    this.statusHandlers.forEach((handler) => handler(status));
  }

  private handleMessage(data: string): void {
    let value: { id?: number; method?: string; params?: unknown; result?: unknown; error?: { code: number; message: string; data?:unknown } };
    try {
      value = JSON.parse(data);
    } catch {
      return;
    }
    if (value.method) {
      this.notificationHandlers.forEach((handler) => handler(value as Notification));
      return;
    }
    const pending = value.id === undefined ? undefined : this.pending.get(value.id);
    if (!pending || value.id === undefined) {
      return;
    }
    this.pending.delete(value.id);
    if (value.error) {
      pending.reject(new RpcError(value.error.message, value.error.code, value.error.data));
    } else {
      pending.resolve(value.result as never);
    }
  }

  private handleClose(socket: SocketLike): void {
    if (socket !== this.socket) {
      return;
    }
    this.socket = null;
    const lost = new RpcError(copy.connectionLost, 0);
    this.pending.forEach((pending) => pending.reject(lost));
    this.pending.clear();
    if (this.closed) {
      return;
    }
    this.emitStatus("reconnecting");
    this.retryLater();
  }

  private async reconnect(): Promise<void> {
    if (this.refreshTarget) {
      try {
        const target = await this.refreshTarget();
        this.url = target.url;
        this.token = target.token;
      } catch {
        this.retryLater();
        return;
      }
    }
    this.connect();
  }

  private retryLater(): void {
    const delay = this.retryMs;
    this.retryMs = Math.min(this.retryMs * 2, LAST_RETRY_MS);
    this.schedule(() => void this.reconnect(), delay);
  }
}
