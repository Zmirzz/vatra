import { isTauri } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { memo, type MouseEvent } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {SyntaxCode} from "../files/CodeView";

const SAFE_LINK = /^(https?:|mailto:)/i;

/** A webview doesn't open new windows on its own, so links go to the user's browser. */
function openExternally(event: MouseEvent<HTMLAnchorElement>, href: string): void {
  if (isTauri()) {
    event.preventDefault();
    openUrl(href).catch(() => undefined);
  }
}

/** Model text is untrusted: raw HTML stays text, and images never reach the network on their own. */
export const Markdown = memo(function Markdown({ text }: { text: string }) {
  return (
    <div className="markdown">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        skipHtml={false}
        components={{
          code:({className,children})=>{
            const language=/language-([\w+-]+)/.exec(className??"")?.[1];
            const source=String(children??"");
            return <SyntaxCode text={source} language={language} className={className}/>;
          },
          a: ({ href, children }) =>
            href && SAFE_LINK.test(href) ? (
              <a href={href} target="_blank" rel="noreferrer noopener" onClick={(event) => openExternally(event, href)}>
                {children}
              </a>
            ) : (
              <a>{children}</a>
            ),
          img: ({ src, alt }) =>
            typeof src === "string" && SAFE_LINK.test(src) ? (
              <a href={src} target="_blank" rel="noreferrer noopener" onClick={(event) => openExternally(event, src)}>
                {alt || src}
              </a>
            ) : (
              <span>{alt}</span>
            ),
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
});
