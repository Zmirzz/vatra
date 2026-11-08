import {useMemo} from "react";
import {copy} from "../../copy";
import {usePreferences,systemPrefersDark} from "../../settings/preferences";
import {previewDocument} from "../../workspace/sanitize";
export function HtmlPreview({html}:{html:string}){const mode=usePreferences(s=>s.preferences.mode);const nonce=useMemo(()=>Array.from(crypto.getRandomValues(new Uint8Array(16)),n=>n.toString(16).padStart(2,"0")).join(""),[]);const dark=mode==="dark"||(mode==="system"&&systemPrefersDark());const src=useMemo(()=>previewDocument(html.slice(0,200000),nonce,dark),[html,nonce,dark]);return <iframe className="workspace-preview" title={copy.workspace.preview} sandbox="" referrerPolicy="no-referrer" srcDoc={src}/>;}
