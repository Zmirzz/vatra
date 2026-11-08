import {it,expect} from "vitest";
import {sanitizeConversationUi} from "./persistence";
it("restores scoped drafts, attachment ranges and inspector tabs from saved state",()=>{
  const state=sanitizeConversationUi({drafts:{3:"unsent"},sessions:{3:{draft:[{id:7,start:10,end:20}],tabs:[7],active:7,open:true}},edits:{"3/7":{text:"unsaved edit",revision:2}},width:500});
  expect(state.drafts[3]).toBe("unsent");expect(state.sessions[3].draft).toEqual([{id:7,start:10,end:20}]);expect(state.edits["3/7"].revision).toBe(2);
});
it("rejects malformed state instead of sending invalid file references",()=>{
  const state=sanitizeConversationUi({drafts:{bad:42},sessions:{3:{draft:[{id:-1},{id:"bad"}],tabs:[null,"wrong"],active:"x",open:true}},width:999999});
  expect(state.drafts).toEqual({});expect(state.sessions[3].draft).toEqual([]);expect(state.sessions[3].tabs).toEqual([]);expect(state.width).toBeLessThanOrEqual(900);
});
