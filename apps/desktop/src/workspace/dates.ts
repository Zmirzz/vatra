import type {WorkspaceItem} from "./types";
type Parts={year:number;month:number;day:number;hour:number;minute:number;second:number};
function utc(parts:Parts):number {const date=new Date(0);date.setUTCFullYear(parts.year,parts.month-1,parts.day);date.setUTCHours(parts.hour,parts.minute,parts.second,0);return date.getTime();}
function parseWall(value:string):Parts|null {const match=/^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/.exec(value);if(!match)return null;const [year,month,day,hour,minute]=match.slice(1).map(Number);if(year<1||year>9999||hour>23||minute>59)return null;const parts={year,month,day,hour,minute,second:0};const date=new Date(utc(parts));return date.getUTCFullYear()===year&&date.getUTCMonth()===month-1&&date.getUTCDate()===day?parts:null;}
export function validDateOnly(value:string):boolean {return /^\d{4}-\d{2}-\d{2}$/.test(value)&&parseWall(value+"T00:00")!==null;}
function formatter(zone:string){return new Intl.DateTimeFormat("en-CA",{timeZone:zone,year:"numeric",month:"2-digit",day:"2-digit",hour:"2-digit",minute:"2-digit",second:"2-digit",hourCycle:"h23"});}
function at(format:Intl.DateTimeFormat,instant:number):Parts {const fields=Object.fromEntries(format.formatToParts(new Date(instant)).filter(p=>p.type!=="literal").map(p=>[p.type,Number(p.value)]));return {year:fields.year,month:fields.month,day:fields.day,hour:fields.hour,minute:fields.minute,second:fields.second};}
const pad=(n:number,width=2)=>String(n).padStart(width,"0");
function wall(parts:Parts){return `${pad(parts.year,4)}-${pad(parts.month)}-${pad(parts.day)}T${pad(parts.hour)}:${pad(parts.minute)}`;}
export function formatWallTime(instant:number,zone:string):string {if(!Number.isSafeInteger(instant)||instant<0||instant>253402300799)throw Error("Invalid instant");return wall(at(formatter(zone),instant*1000));}
export function localTimeCandidates(value:string,zone:string):{instant:number;offsetMinutes:number}[]{
 const parts=parseWall(value);if(!parts)return [];const format=formatter(zone);const naive=utc(parts);const offsets=new Set<number>();
 // Collect both sides of nearby transitions instead of letting Date silently choose an occurrence.
 for(let hours=-48;hours<=48;hours+=6){const sample=naive+hours*3600000;offsets.add(utc(at(format,sample))-sample);}
 const candidates=[...offsets].map(offset=>({instant:(naive-offset)/1000,offsetMinutes:offset/60000})).filter(candidate=>Number.isSafeInteger(candidate.instant)&&candidate.instant>=0&&candidate.instant<=253402300799&&formatWallTime(candidate.instant,zone)===value);
 return candidates.sort((a,b)=>a.instant-b.instant);
}
export function offsetLabel(minutes:number):string {const sign=minutes<0?"-":"+";const absolute=Math.abs(minutes);return `UTC${sign}${pad(Math.floor(absolute/60))}:${pad(absolute%60)}`;}
export function localTimezone():string {return Intl.DateTimeFormat().resolvedOptions().timeZone;}
export function todayInZone(instant:number,zone:string):string {return formatWallTime(instant,zone).slice(0,10);}
export type TaskView="today"|"upcoming"|"completed";
export function taskBucket(fields:Record<string,unknown>,today:string):TaskView {if(fields.status==="completed")return "completed";return typeof fields.due_date==="string"&&fields.due_date<=today?"today":"upcoming";}
export function isOverdue(item:WorkspaceItem,today:string){return item.fields.status!=="completed"&&typeof item.fields.due_date==="string"&&item.fields.due_date<today;}
