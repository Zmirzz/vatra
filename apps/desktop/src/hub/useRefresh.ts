import {useEffect, useRef} from "react";
import type {RpcClient} from "../rpc/client";

/** Calls `reload` when the service says one of these things changed (and once the connection returns). */
export function useRefresh(rpc: RpcClient, methods: string[], reload: () => void) {
  const latest = useRef(reload);
  latest.current = reload;
  const key = methods.join("|");
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const off = rpc.on((n) => {
      if (methods.includes(n.method)) {
        clearTimeout(timer);
        timer = setTimeout(() => latest.current(), 120);
      }
    });
    const status = rpc.onStatus((s) => { if (s === "connected") latest.current(); });
    return () => { clearTimeout(timer); off(); status(); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rpc, key]);
}
