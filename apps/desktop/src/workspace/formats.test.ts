import {expect,it} from "vitest";
import {readFileSync} from "node:fs";
import {resolve} from "node:path";
const fixtures=readFileSync(resolve(process.cwd(),"../../tests/fixtures/workspace-csv.json"),"utf8");
import {parseCsv,serializeCsv,sourceSelection,supportsRichEditing} from "./formats";
for(const example of JSON.parse(fixtures) as {name:string;body:string;rows:string[][]}[])it(`shares CSV contract: ${example.name}`,()=>{expect(parseCsv(example.body)).toEqual(example.rows);expect(parseCsv(serializeCsv(example.rows))).toEqual(example.rows);});
it("rejects malformed CSV and cells beyond the common limits",()=>{for(const body of ['a,b\nx\n','a"b,c','"unclosed',new Array(102).join("a,"),"a".repeat(2097153)])expect(()=>parseCsv(body)).toThrow();});
it("uses exact UTF-16 offsets without splitting an emoji",()=>{expect(sourceSelection("a😀b",1,3)).toEqual({start_utf16:1,end_utf16:3,selected_text:"😀"});expect(sourceSelection("a😀b",1,2)).toBeNull();});
it("keeps unsupported formatting on a source/preview route",()=>{expect(supportsRichEditing("![image](https://outside.invalid/a)","markdown")).toBe(false);expect(supportsRichEditing("<table><tr><td>text</td></tr></table>","html")).toBe(false);expect(supportsRichEditing("# Title\n\nSome **text**.","markdown")).toBe(true);});
