import {expect,it,vi} from "vitest";
import {sanitizePreview,previewDocument} from "./sanitize";
it("removes executable content, resources, links and foreign namespaces before insertion",()=>{
 const parse=vi.spyOn(DOMParser.prototype,"parseFromString");
 const clean=sanitizePreview('<p onclick="alert(1)">Hello &amp; țară</p><img src="https://outside.invalid/image"><iframe src="https://outside.invalid/frame"></iframe><svg><a href="javascript:alert(1)">x</a></svg><math><mtext>x</mtext></math><script>alert(1)</script><form action="/rpc"><input></form><a href="/rpc">label</a><style>@import "https://outside.invalid/css";</style>');
 expect(clean).toContain("Hello &amp; țară");expect(clean).toContain("label");expect(clean).not.toMatch(/script|onclick|src=|href=|iframe|svg|math|style|form|input|outside\.invalid/i);expect(parse).not.toHaveBeenCalled();parse.mockRestore();
});
it("uses a fixed stylesheet and a nonce-bound CSP for a sandboxed preview document",()=>{const html=previewDocument('<p style="background:url(https://outside.invalid)">safe</p>',"abc123",true);expect(html).toContain("default-src 'none'");expect(html).toContain("style-src 'nonce-abc123'");expect(html).toContain('nonce="abc123"');expect(html).not.toContain("outside.invalid");expect(html).toContain('data-theme="dark"');});
