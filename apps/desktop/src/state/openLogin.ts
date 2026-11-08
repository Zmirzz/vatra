import { isTauri } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";

const WEB_ADDRESS = /^https?:\/\//i;

/** Opens the address of an MCP login in the user's browser. Other schemes are never opened. */
export function openLoginAddress(url: string): void {
  if (!WEB_ADDRESS.test(url)) {
    return;
  }
  if (isTauri()) {
    openUrl(url).catch(() => undefined);
  } else {
    window.open(url, "_blank", "noopener,noreferrer");
  }
}
