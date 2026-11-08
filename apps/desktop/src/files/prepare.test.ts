import { describe, it, expect } from "vitest";
import { detectFileType, prepareText, validateZipDirectory, selectDocumentContext } from "./prepare";

describe("file preparation",()=>{
  it("uses content signatures and refuses disguised PDFs",()=>{
    expect(detectFileType("a.pdf",new TextEncoder().encode("%PDF-1.7\n"))).toBe("application/pdf");
    expect(()=>detectFileType("a.pdf",new TextEncoder().encode("not pdf"))).toThrow();
    expect(()=>detectFileType("a.png",new Uint8Array([77,90,0,0]))).toThrow();
    expect(detectFileType("data.json",new TextEncoder().encode('{"ok":true}'))).toBe("application/json");
  });
  it("preserves unicode, JSON structure and CSV cells",()=>{
    expect(prepareText("note.md",new TextEncoder().encode("# Ședință\n\nBună"))).toBe("# Ședință\n\nBună");
    expect(prepareText("a.json",new TextEncoder().encode('{"n":2}'))).toContain('"n": 2');
    expect(prepareText("a.csv",new TextEncoder().encode('name,value\n"a,b",=1+1'))).toContain('"a,b"');
    expect(()=>prepareText("a.json",new TextEncoder().encode('{broken'))).toThrow();
  });
  it("refuses malformed ZIPs before library decompression",()=>{
    expect(()=>validateZipDirectory(new Uint8Array([80,75,3,4]))).toThrow();
  });
  it("requires explicit selection for large document context",()=>{
    const body="a".repeat(100_000);
    expect(()=>selectDocumentContext(body,4000)).toThrow(/select/i);
    expect(selectDocumentContext(body,4000,{start:100,end:1000})).toBe("a".repeat(900));
    expect(()=>selectDocumentContext(body,100,{start:0,end:900})).toThrow();
  });
});
