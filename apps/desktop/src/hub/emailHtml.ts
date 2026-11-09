import createDOMPurify from "dompurify";

/** What the reader needs to show a message written in HTML: a finished page, and the pictures it would fetch from the web. */
export type PreparedMail = {doc: string; remote: string[]};

const FORBIDDEN_TAGS = [
  "script", "iframe", "frame", "frameset", "object", "embed", "applet", "form", "input", "button", "textarea", "select", "option", "link", "meta", "base",
  "template", "svg", "math", "audio", "video", "source", "track", "dialog", "noscript", "title",
];
const FORBIDDEN_ATTRS = ["srcset", "ping", "action", "formaction", "autofocus", "contenteditable"];

// The page is shown in a frame that cannot run scripts, load anything from the web, or send anything anywhere.
const FRAME_POLICY = "default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:";
const FRAME_STYLE = "html,body{margin:0;padding:0;background:#fff;color:#1f2328}body{padding:16px 18px;font:14px/1.55 -apple-system,'Segoe UI',Inter,system-ui,sans-serif;overflow-wrap:anywhere}img{max-width:100%;height:auto}table{max-width:100%}a{color:#4b3fd6}pre{white-space:pre-wrap}blockquote{margin:8px 0;padding-left:12px;border-left:3px solid #d0d7de;color:#57606a}";

const purify = createDOMPurify(window);

function remoteAddress(value: string): string | null {
  const v = value.trim();
  if (/^https?:\/\//i.test(v)) return v;
  if (v.startsWith("//")) return `https:${v}`;
  return null;
}

/** Strips address-bearing `url(...)` from a style so it can't fetch anything. */
function plainStyle(style: string): string {
  return style.replace(/url\([^)]*\)/gi, "none").replace(/expression\([^)]*\)/gi, "").replace(/@import[^;]*;?/gi, "");
}

/**
 * Cleans a message's HTML and wraps it in a page for the frame. Pictures from the web are left out unless `loaded` has them
 * (a map from address to a `data:` address), so opening a message never tells the sender anything.
 */
export function prepareMail(html: string, loaded: Record<string, string> = {}): PreparedMail {
  const remote = new Set<string>();
  purify.removeAllHooks();
  purify.addHook("afterSanitizeAttributes", (node) => {
    if (!(node instanceof Element)) return;
    if (node.tagName === "IMG") {
      const src = node.getAttribute("src") ?? "";
      const address = remoteAddress(src);
      if (address) {
        remote.add(address);
        if (loaded[address]) node.setAttribute("src", loaded[address]);
        else { node.removeAttribute("src"); node.setAttribute("data-hidden", "1"); }
      } else if (!/^data:image\/(png|jpe?g|gif|webp|bmp|avif);/i.test(src)) {
        node.removeAttribute("src");
      }
    }
    if (node.hasAttribute("style")) node.setAttribute("style", plainStyle(node.getAttribute("style") ?? ""));
    for (const attribute of ["background", "poster", "src"]) {
      if (node.tagName !== "IMG" && node.hasAttribute(attribute)) node.removeAttribute(attribute);
    }
    if (node.tagName === "STYLE") node.textContent = plainStyle(node.textContent ?? "");
    if (node.tagName === "A") {
      node.setAttribute("rel", "noopener noreferrer");
      node.removeAttribute("target");
    }
  });
  const body = purify.sanitize(html, {FORCE_BODY: true, FORBID_TAGS: FORBIDDEN_TAGS, FORBID_ATTR: FORBIDDEN_ATTRS, ALLOW_DATA_ATTR: false, ALLOWED_URI_REGEXP: /^(?:https?|mailto|tel|data:image\/(?:png|jpe?g|gif|webp|bmp|avif);|#)/i});
  const doc = `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${FRAME_POLICY}"><style>${FRAME_STYLE}</style></head><body>${body}</body></html>`;
  return {doc, remote: [...remote]};
}

/** Where a link in a message leads, or null when it isn't one the app opens (only web, mail and phone links are). */
export function openableLink(href: string): string | null {
  return /^(https?:|mailto:|tel:)/i.test(href.trim()) ? href.trim() : null;
}

export function senderName(from: string): string {
  const name = from.includes("<") ? from.slice(0, from.lastIndexOf("<")).trim().replace(/^"+|"+$/g, "").trim() : "";
  if (name) return name;
  const address = from.replace(/[<>"]/g, "").trim();
  return address.split("@")[0] || address || "?";
}

export function senderAddress(from: string): string {
  const match = from.match(/<([^>]+)>\s*$/);
  return (match ? match[1] : from).trim();
}

export function initial(from: string): string {
  const letter = senderName(from).match(/[\p{L}\p{N}]/u)?.[0];
  return letter ? letter.toUpperCase() : "?";
}

/** Today shows the time, this year the day and month, older mail the year as well. A date that can't be read is shown as written. */
export function shortDate(raw: string, language: string, now = new Date()): string {
  const at = new Date(raw);
  if (Number.isNaN(at.getTime())) return raw;
  const locale = language === "ro" ? "ro-RO" : "en-GB";
  if (at.toDateString() === now.toDateString()) return at.toLocaleTimeString(locale, {hour: "2-digit", minute: "2-digit"});
  if (at.getFullYear() === now.getFullYear()) return at.toLocaleDateString(locale, {day: "numeric", month: "short"});
  return at.toLocaleDateString(locale, {day: "numeric", month: "short", year: "numeric"});
}

export function fileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
