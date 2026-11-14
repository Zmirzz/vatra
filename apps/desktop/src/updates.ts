import { invoke, isTauri } from "@tauri-apps/api/core";
import { create } from "zustand";

type Status = { configured: boolean; available: string | null; error: string | null };
type UpdateState = {
  status: Status | null;
  startupChecked: boolean;
  checking: boolean;
  installing: boolean;
  installed: boolean;
  installError: string | null;
  dismissedVersion: string | null;
};

export const initialUpdateState = (): UpdateState => ({
  status: null, startupChecked: false, checking: false, installing: false,
  installed: false, installError: null, dismissedVersion: null,
});

// Settings and the startup prompt share requests and installation state.
export const useUpdates = create<UpdateState>(initialUpdateState);

export async function checkForUpdates(startup = false): Promise<void> {
  if (!isTauri()) return;
  const state = useUpdates.getState();
  if (startup && state.startupChecked) return;
  if (startup) useUpdates.setState({ startupChecked: true });
  if (state.checking || state.installing || state.installed) return;
  useUpdates.setState({ checking: true, installError: null });
  try { useUpdates.setState({ status: await invoke<Status>("update_status") }); }
  catch (error) { useUpdates.setState({ status: { configured: true, available: null, error: String(error) } }); }
  finally { useUpdates.setState({ checking: false }); }
}

export async function installUpdate(): Promise<void> {
  const state = useUpdates.getState();
  if (!isTauri() || !state.status?.available || state.installing || state.installed) return;
  useUpdates.setState({ installing: true, installError: null });
  try {
    // The native command checks again and verifies the downloaded artifact's signature.
    await invoke("install_update");
    useUpdates.setState({ installed: true });
  } catch (error) { useUpdates.setState({ installError: String(error) }); }
  finally { useUpdates.setState({ installing: false }); }
}

export function dismissUpdate(): void {
  if (!useUpdates.getState().installing) {
    useUpdates.setState({ dismissedVersion: useUpdates.getState().status?.available ?? null });
  }
}
