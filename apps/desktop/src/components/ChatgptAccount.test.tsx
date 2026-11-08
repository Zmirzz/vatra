import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { ClientProvider } from "../rpc/context";
import type { RpcClient } from "../rpc/client";
import { ChatgptAccount } from "./ChatgptAccount";

test("connection is explicit and tokens never enter account UI", async () => {
  const call=vi.fn(async (method:string)=>method==="chatgpt.status" ? {connected:false,sharing:false,account:null}:{});
  const client={call,on:()=>()=>{}} as unknown as RpcClient;
  render(<ClientProvider client={client}><ChatgptAccount /></ClientProvider>);
  const connect=await screen.findByRole("button",{name:"Continue with ChatGPT"});
  expect(call).not.toHaveBeenCalledWith("chatgpt.login",{});
  fireEvent.click(connect);
  await waitFor(()=>expect(call).toHaveBeenCalledWith("chatgpt.login",{}));
  expect(screen.getByRole("button",{name:"Cancel"})).toBeTruthy();
});

test("returning to the page during a sign-in still offers to cancel it", async () => {
  const call=vi.fn(async ()=>({connected:false,sharing:false,account:null,connecting:true}));
  const client={call,on:()=>()=>{}} as unknown as RpcClient;
  render(<ClientProvider client={client}><ChatgptAccount /></ClientProvider>);
  expect(await screen.findByRole("button",{name:"Cancel"})).toBeTruthy();
  expect(screen.queryByRole("button",{name:"Continue with ChatGPT"})).toBeNull();
});
