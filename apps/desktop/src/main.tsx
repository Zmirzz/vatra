import "@fontsource-variable/inter";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { copy } from "./copy";
import { connect } from "./rpc/connection";
import { ClientProvider } from "./rpc/context";
import { applyPreferences, loadPreferences, systemPrefersDark, usePreferences } from "./settings/preferences";
import { attachClient } from "./state/store";
import "./styles.css";
import {attachConversationPersistence} from "./files/persistence";

// Applied before the first paint, so the window doesn't flash the wrong theme.
applyPreferences(loadPreferences(), document.documentElement, systemPrefersDark());
window.matchMedia?.("(prefers-color-scheme: dark)").addEventListener("change", () => {
  applyPreferences(usePreferences.getState().preferences, document.documentElement, systemPrefersDark());
});

const root = createRoot(document.getElementById("root") as HTMLElement);

connect()
  .then((client) => {
    attachClient(client);
    attachConversationPersistence(client);
    client.connect();
    root.render(
      <StrictMode>
        <ClientProvider client={client}>
          <App />
        </ClientProvider>
      </StrictMode>,
    );
  })
  .catch((error: unknown) => {
    root.render(
      <div className="empty" role="alert">
        <h2>{copy.startFailed}</h2>
        <p>{String(error)}</p>
        <button type="button" className="primary" onClick={() => location.reload()}>
          {copy.retry}
        </button>
      </div>,
    );
  });
