import {FileText, ImageOff, Paperclip} from "lucide-react";
import {useEffect, useMemo, useRef, useState} from "react";
import {openableLink, fileSize, prepareMail} from "../../hub/emailHtml";
import {openExternal} from "../../hub/openExternal";
import {useHubCopy} from "../../hub/copy";
import {useClient} from "../../rpc/context";
import type {MailAttachment} from "../../hub/types";

/**
 * A message written in HTML, shown the way its sender designed it: in a frame that can't run anything, with pictures from the
 * web held back until the person asks for them. Links open in the person's browser.
 */
export function MailBody({html}: {html: string}) {
  const t = useHubCopy();
  const rpc = useClient();
  const frame = useRef<HTMLIFrameElement>(null);
  const [loaded, setLoaded] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [height, setHeight] = useState(120);
  const prepared = useMemo(() => prepareMail(html, loaded), [html, loaded]);
  const waiting = prepared.remote.filter((address) => !loaded[address]);

  useEffect(() => { setLoaded({}); }, [html]);

  const showImages = async () => {
    setBusy(true);
    try {
      const result = await rpc.call<{images: Record<string, string>}>("email.images", {urls: prepared.remote.slice(0, 40)});
      setLoaded((old) => ({...old, ...result.images}));
    } catch { /* the pictures simply stay hidden */ } finally { setBusy(false); }
  };

  const attach = () => {
    const doc = frame.current?.contentDocument;
    if (!doc) return;
    const fit = () => setHeight(Math.max(80, doc.documentElement.scrollHeight));
    fit();
    doc.addEventListener("click", (event) => {
      const link = (event.target as Element | null)?.closest?.("a[href]");
      if (!link) return;
      event.preventDefault();
      const target = openableLink(link.getAttribute("href") ?? "");
      if (target) void openExternal(target);
    });
    if (typeof ResizeObserver !== "undefined") new ResizeObserver(fit).observe(doc.body);
    for (const image of Array.from(doc.images)) image.addEventListener("load", fit);
  };

  return (
    <div className="mail-html">
      {waiting.length > 0 ? (
        <div className="mail-images-note" role="status">
          <ImageOff size={15} aria-hidden="true" />
          <span>{t.imagesHidden}</span>
          <button type="button" onClick={() => void showImages()} disabled={busy}>{busy ? t.working : t.showImages}</button>
        </div>
      ) : null}
      <iframe ref={frame} className="mail-frame" title={t.emailTitle} sandbox="allow-same-origin" srcDoc={prepared.doc} style={{height}} onLoad={attach} />
    </div>
  );
}

export function Attachments({files}: {files: MailAttachment[]}) {
  const t = useHubCopy();
  if (files.length === 0) return null;
  const pictures = files.filter((f) => f.preview);
  const others = files.filter((f) => !f.preview);
  return (
    <section className="mail-attachments" aria-label={t.attachments}>
      <h3><Paperclip size={14} aria-hidden="true" />{t.attachments} <span>{files.length}</span></h3>
      {pictures.length ? (
        <div className="mail-pictures">
          {pictures.map((f, i) => <figure key={`${f.name}-${i}`}><img src={f.preview ?? ""} alt={f.name} /><figcaption>{f.name} · {fileSize(f.size)}</figcaption></figure>)}
        </div>
      ) : null}
      {others.length ? (
        <ul className="mail-files">
          {others.map((f, i) => <li key={`${f.name}-${i}`}><FileText size={16} aria-hidden="true" /><span>{f.name}</span><small>{fileSize(f.size)}</small></li>)}
        </ul>
      ) : null}
    </section>
  );
}
