import {memo,useEffect,useState} from "react";
import DOMPurify from "dompurify";
export const SyntaxCode=memo(function SyntaxCode({text,language,className}:{text:string;language?:string;className?:string}){
  const [highlighted,setHighlighted]=useState<{text:string;html:string}|null>(null);
  useEffect(()=>{
    if(!language||text.length>100_000)return;let active=true;
    const timer=setTimeout(()=>{void import("highlight.js/lib/common").then(({default:hljs})=>{
      if(active&&hljs.getLanguage(language)){const html=DOMPurify.sanitize(hljs.highlight(text,{language,ignoreIllegals:true}).value,{ALLOWED_TAGS:["span"],ALLOWED_ATTR:["class"]});setHighlighted({text,html});}
    });},40);
    return()=>{active=false;clearTimeout(timer);};
  },[text,language]);
  return highlighted?.text===text?<code className={className} dangerouslySetInnerHTML={{__html:highlighted.html}}/>:<code className={className}>{text}</code>;
});
export function fileLanguage(name:string){return ({rs:"rust",ts:"typescript",tsx:"typescript",js:"javascript",jsx:"javascript",py:"python",css:"css",json:"json",html:"xml",xml:"xml",toml:"ini",yaml:"yaml",yml:"yaml",sh:"bash",ps1:"powershell",sql:"sql"} as Record<string,string>)[name.split(".").pop()?.toLowerCase()??""];}
