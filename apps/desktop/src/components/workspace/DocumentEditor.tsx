import {useEffect,useMemo,useRef,useState} from "react";
import {EditorContent,useEditor} from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import {Markdown} from "@tiptap/markdown";
import ReactMarkdown from "react-markdown";
import {copy} from "../../copy";
import type {DocumentFormat,Selection} from "../../workspace/types";
import {richSelection,sourceSelection,supportsRichEditing,type DocumentAdapter} from "../../workspace/formats";
import {sanitizePreview} from "../../workspace/sanitize";
import {HtmlPreview} from "./HtmlPreview";
import {CsvEditor} from "./CsvEditor";
type Props={body:string;format:DocumentFormat;onChange:(body:string)=>void;onSelection?:(selection:Selection|null,unavailable:boolean)=>void};
function RichEditor({body,format,onChange,onSelection}:Props){
 const source=useRef(body);const loading=useRef(false);const callbacks=useRef({onChange,onSelection});callbacks.current={onChange,onSelection};
 const content=(value:string)=>format==="html"?sanitizePreview(value):value;
 const editor=useEditor({extensions:[StarterKit.configure({link:false}),Markdown],content:content(body),contentType:format==="html"?"html":"markdown",immediatelyRender:true,editorProps:{attributes:{role:"textbox","aria-label":copy.workspace.text,"aria-multiline":"true"},transformPastedHTML:sanitizePreview,handlePaste:(view,event)=>{const html=event.clipboardData?.getData("text/html");const text=event.clipboardData?.getData("text/plain");if(html&&!supportsRichEditing(html,"html")&&!window.confirm(copy.workspace.convertConfirm))return true;if(!html&&text&&!supportsRichEditing(text,"markdown")){view.dispatch(view.state.tr.insertText(text));return true;}return false;}},onUpdate:({editor,transaction})=>{if(loading.current||!transaction.docChanged)return;source.current=format==="html"?editor.getHTML():editor.getMarkdown();callbacks.current.onChange(source.current);},onSelectionUpdate:({editor})=>{const {from,to}=editor.state.selection;const selected=editor.state.doc.textBetween(from,to,"\n");const selection=richSelection(source.current,selected);callbacks.current.onSelection?.(selection,!!selected&&!selection);}},[format]);
 const adapter=useMemo<DocumentAdapter|null>(()=>editor?{load(value){source.current=value;loading.current=true;editor.commands.setContent(content(value),{emitUpdate:false,contentType:format==="html"?"html":"markdown"});loading.current=false;},serialize:()=>source.current,selection:()=>richSelection(source.current,editor.state.doc.textBetween(editor.state.selection.from,editor.state.selection.to,"\n"))}:null,[editor,format]);
 useEffect(()=>{if(adapter&&body!==adapter.serialize())adapter.load(body);},[adapter,body]);
 return <><div className="workspace-editor-tools">{([['bold',()=>editor?.chain().focus().toggleBold().run()],['italic',()=>editor?.chain().focus().toggleItalic().run()],['bulletList',()=>editor?.chain().focus().toggleBulletList().run()],['undo',()=>editor?.chain().focus().undo().run()],['redo',()=>editor?.chain().focus().redo().run()]] as const).map(([name,run])=><button type="button" key={name} aria-label={copy.workspace[name]} title={copy.workspace[name]} onMouseDown={e=>e.preventDefault()} onClick={run}>{copy.workspace[name]}</button>)}</div><EditorContent className="workspace-rich-editor" editor={editor}/></>;
}
export function DocumentEditor(props:Props){const {body,format,onChange,onSelection}=props;const supported=supportsRichEditing(body,format);const [converted,setConverted]=useState(false);const [mode,setMode]=useState<"edit"|"preview"|"source">(()=>format==="html"||(!supported&&format==="markdown")?"preview":"edit");
 if(format==="csv")return <CsvEditor body={body} onChange={onChange}/>;
 if(format==="text")return <textarea className="workspace-body" aria-label={copy.workspace.text} value={body} onChange={e=>onChange(e.target.value)} onSelect={e=>{const el=e.currentTarget;onSelection?.(sourceSelection(body,el.selectionStart,el.selectionEnd),false);}}/>;
 const rich=supported||(converted&&format==="html");const shown=mode==="edit"&&!rich?"source":mode;
 return <div className="workspace-document"><div className="workspace-format-tabs">{(rich?["edit","preview","source"] as const:["preview","source"] as const).map(value=><button type="button" aria-pressed={shown===value} key={value} onClick={()=>{onSelection?.(null,false);setMode(value);}}>{copy.workspace[value]}</button>)}</div>{!rich?<div className="workspace-format-help"><p>{copy.workspace.unsupported}</p>{format==="html"?<button type="button" onClick={()=>{if(window.confirm(copy.workspace.convertConfirm)){setConverted(true);setMode("edit");}}}>{copy.workspace.convert}</button>:null}</div>:null}
 {body.length>200000&&shown==="preview"?<p className="workspace-format-help">{copy.workspace.previewShortened}</p>:null}{shown==="source"?<textarea className="workspace-body workspace-source" aria-label={copy.workspace.source} value={body} onChange={e=>onChange(e.target.value)} onSelect={e=>{const el=e.currentTarget;onSelection?.(sourceSelection(body,el.selectionStart,el.selectionEnd),false);}}/>:shown==="preview"?(format==="html"?<HtmlPreview html={body}/>:<div className="workspace-markdown-preview"><ReactMarkdown skipHtml components={{img:()=>null,a:({children})=><span>{children}</span>}}>{body.slice(0,200000)}</ReactMarkdown></div>):<RichEditor {...props}/>}
 </div>;
}
