import { Check, Plus, Search } from "lucide-react";
import { useEffect, useState } from "react";
import { copy } from "../copy";
import { useClient } from "../rpc/context";
import type { CatalogEntry, MarketListing } from "../rpc/types";
import { useAppStore } from "../state/store";

type Text = { name: string; blurb: string };
const textOf = (listing: MarketListing): Text => (copy.market.items as Record<string, Text>)[listing.id] ?? { name: listing.name, blurb: "" };

/** The market of connections: a short list of well-known tools to add with one click, for someone who has none yet. */
export function MarketPage({ onManage }: { onManage?: () => void }) {
  const client = useClient();
  const [listings, setListings] = useState<MarketListing[]>([]);
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [asking, setAsking] = useState<string | null>(null);
  const [token, setToken] = useState("");
  const [note, setNote] = useState<{ id: string; text: string } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = () => client.call<MarketListing[]>("market.list", {}).then(setListings);
  useEffect(() => {
    let live = true;
    client.call<MarketListing[]>("market.list", {}).then((list) => { if (live) setListings(list); }).catch((caught) => { if (live) setError(caught.message); });
    const off = client.on((notice) => {
      if (notice.method === "mcp_login") setNote({ id: notice.params.server, text: notice.params.message });
    });
    return () => { live = false; off(); };
  }, [client]);

  const add = async (listing: MarketListing) => {
    setBusy(listing.id); setError(null); setNote(null);
    try {
      const done = await client.call<{ name: string; login: boolean }>("market.install", { id: listing.id, token: listing.needs === "token" ? token : undefined });
      setAsking(null); setToken("");
      await client.call<CatalogEntry[]>("catalog.list", {}).then((catalog) => useAppStore.setState({ catalog })).catch(() => undefined);
      await refresh();
      if (done.login) {
        setNote({ id: listing.id, text: copy.market.signInLater });
      }
    } catch (caught) { setError(`${copy.market.failed} ${(caught as Error).message}`); }
    finally { setBusy(null); }
  };
  const signIn = async (listing: MarketListing) => {
    setError(null);
    try { await client.call("mcp.login", { server: listing.id }); setNote({ id: listing.id, text: copy.market.signInStarted }); }
    catch (caught) { setError((caught as Error).message); }
  };

  const needle = query.trim().toLowerCase();
  const shown = listings.filter((listing) => { const text = textOf(listing); return !needle || `${text.name} ${text.blurb}`.toLowerCase().includes(needle); });

  return (
    <section className="transfer-page market-page">
      <h3>{copy.market.title}</h3>
      <p className="help">{copy.market.intro}</p>
      <label className="market-search">
        <Search size={14} aria-hidden="true" />
        <input type="search" aria-label={copy.market.search} placeholder={copy.market.search} value={query} onChange={(event) => setQuery(event.target.value)} />
      </label>
      {error ? <p role="alert">{error}</p> : null}
      <ul className="market-grid">
        {shown.map((listing) => {
          const text = textOf(listing);
          const message = note?.id === listing.id ? note.text : null;
          return (
            <li key={listing.id} className="market-card">
              <div className="market-head">
                <b>{text.name}</b>
                {listing.connected ? <span className="market-added"><Check size={13} aria-hidden="true" /> {copy.market.added}</span> : null}
              </div>
              <p className="help">{text.blurb}</p>
              <small className="help">
                {listing.needs === "account" ? copy.market.needsAccount : listing.needs === "token" ? copy.market.needsToken : null}
                {listing.runsProgram ? copy.market.runsProgram : null}
              </small>
              {asking === listing.id ? (
                <form className="market-token" onSubmit={(event) => { event.preventDefault(); void add(listing); }}>
                  <label>{copy.market.token}
                    <input type="password" aria-label={copy.market.token} autoComplete="new-password" autoFocus value={token} onChange={(event) => setToken(event.target.value)} />
                    <small>{copy.market.tokenHelp} {listing.token ? <a href={listing.token.getUrl} target="_blank" rel="noreferrer">{copy.market.getToken}</a> : null}</small>
                  </label>
                  <button type="submit" className="primary" disabled={!token.trim() || busy !== null}>{busy === listing.id ? copy.market.adding : copy.market.add}</button>
                </form>
              ) : listing.connected ? (
                listing.needs === "account" ? <button type="button" onClick={() => void signIn(listing)}>{copy.market.signIn}</button> : null
              ) : (
                <button type="button" className="primary" disabled={busy !== null} onClick={() => (listing.needs === "token" ? (setAsking(listing.id), setToken("")) : void add(listing))}>
                  <Plus size={13} aria-hidden="true" /> {busy === listing.id ? copy.market.adding : copy.market.add}
                </button>
              )}
              {message ? <p role="status" className="help">{message}</p> : null}
            </li>
          );
        })}
      </ul>
      {listings.length > 0 && shown.length === 0 ? <p className="help">{copy.market.empty}</p> : null}
      {onManage ? <button type="button" className="link" onClick={onManage}>{copy.market.manage}</button> : null}
    </section>
  );
}
