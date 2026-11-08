import { invoke, isTauri } from "@tauri-apps/api/core";
import { RpcClient } from "./client";

type ConnectionInfo = { port: number; token: string };

const rpcUrl = (port: number) => `ws://127.0.0.1:${port}/rpc`;

/** The desktop shell knows the port and token, and restarts the service if it died. */
async function shellTarget() {
  const { port, token } = await invoke<ConnectionInfo>("connection_info");
  return { url: rpcUrl(port), token };
}

export async function connect(): Promise<RpcClient> {
  if (isTauri()) {
    const { url, token } = await shellTarget();
    return new RpcClient(url, token, { refreshTarget: shellTarget });
  }
  // In a browser (`npm run dev`) they come from the url.
  const parameters = new URLSearchParams(location.hash.slice(1));
  return new RpcClient(rpcUrl(Number(parameters.get("port"))), parameters.get("token") ?? "");
}
