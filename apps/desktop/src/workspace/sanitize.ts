import DOMPurify from "dompurify";
const TAGS=["div","p","h1","h2","h3","h4","h5","h6","ul","ol","li","strong","b","em","i","s","del","code","pre","br","hr","blockquote","table","thead","tbody","tfoot","tr","th","td"];
/** Parse only in inert template contents; resource elements never enter an active document before sanitization. */
export function sanitizePreview(html:string):string {
 const template=document.createElement("template");template.innerHTML=html;
 const root=template.content.ownerDocument.createElement("div");root.append(template.content);
 DOMPurify.sanitize(root,{IN_PLACE:true,ALLOWED_TAGS:TAGS,ALLOWED_ATTR:["colspan","rowspan"],ALLOW_DATA_ATTR:false,ALLOW_ARIA_ATTR:false,FORBID_TAGS:["svg","math","style","script","iframe","form","input","img","video","audio","object","embed","link","meta","base","template"],FORBID_CONTENTS:["svg","math","style","script","iframe","form","object","template"],SANITIZE_DOM:true});
 for(const node of root.querySelectorAll("[colspan],[rowspan]"))for(const name of ["colspan","rowspan"])if(node.hasAttribute(name)){const n=Number(node.getAttribute(name));if(!Number.isInteger(n)||n<1||n>100)node.removeAttribute(name);}
 return root.innerHTML;
}
const STYLE="html{color-scheme:light;background:#fff;color:#17171a}html[data-theme=dark]{color-scheme:dark;background:#111;color:#eee}body{font:15px/1.7 system-ui,sans-serif;max-width:76ch;margin:24px auto;padding:0 24px;overflow-wrap:anywhere}pre{white-space:pre-wrap}table{border-collapse:collapse;max-width:100%}td,th{border:1px solid #888;padding:6px 10px}blockquote{border-left:2px solid #888;margin-left:0;padding-left:18px}";
export function previewDocument(html:string,nonce:string,dark:boolean){if(!/^[a-zA-Z0-9]+$/.test(nonce))throw Error("Invalid preview nonce");return `<!doctype html><html data-theme="${dark?"dark":"light"}"><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'nonce-${nonce}'; form-action 'none'; base-uri 'none'"><style nonce="${nonce}">${STYLE}</style></head><body>${sanitizePreview(html)}</body></html>`;}
