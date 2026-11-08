declare module "mammoth/mammoth.browser" {
  export function convertToHtml(input:{arrayBuffer:ArrayBufferLike},options?:{externalFileAccess?:boolean}):Promise<{value:string;messages:{message:string}[]}>;
}
