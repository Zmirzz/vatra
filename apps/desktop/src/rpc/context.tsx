import { createContext, useContext, type ReactNode } from "react";
import type { RpcClient } from "./client";

const ClientContext = createContext<RpcClient | null>(null);

export function ClientProvider({ client, children }: { client: RpcClient; children: ReactNode }) {
  return <ClientContext.Provider value={client}>{children}</ClientContext.Provider>;
}

export function useClient(): RpcClient {
  const client = useContext(ClientContext);
  if (client === null) {
    throw new Error("useClient needs a ClientProvider");
  }
  return client;
}
