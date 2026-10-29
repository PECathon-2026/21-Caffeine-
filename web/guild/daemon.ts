/** Explicit daemon wire types; all statuses, models and evidence come from Rust. */
export type AgentRole='planner'|'designer'|'coder'|'reviewer'|'security';
export interface Mission {id:string;objective:string;status:string;statusReason?:string;createdAt:number;updatedAt:number;summary?:string}
export interface Task {id:string;taskKey:string;role:AgentRole;title:string;description:string;status:string;dependencies:string[];acceptance:string[];statusReason?:string;summary?:string;attempts:number;workspacePath?:string}
export interface Stats {inputTokens:number;outputTokens:number;costMicroUsd:number;[key:string]:number}
export interface Summary {mission:Mission;stats:Stats;taskCounts:{total:number;completed:number;running:number;blocked:number;failed:number}}
export interface Detail {mission:Mission;tasks:Task[];stats:Stats;plan?:unknown}
export interface Approval {id:string;missionId:string;action:string;resource:string;policySnapshot:unknown;patchHash?:string}
export interface Snapshot {missions:Summary[];approvals:Approval[];lastSeq:number;generation:string;workers:{missionId:string;status:string}[]}
export interface RoleModel {role:AgentRole;provider:string;model:string;fallbacks:string[]}
export interface Session {id:string;title:string;activeMissionId?:string;createdAt:number}
export interface ChatMessage {id:string;role:string;content:string;createdAt:number}
export interface EventRecord {seq:number;missionId:string;taskId?:string;kind:string;payload:Record<string,unknown>;createdAt:number}
export interface Version {id:string;label:string;kind:string;createdAt:number}
export interface RestorePreview {target:Version;token:string;unified:string;conflicts:string[];affectedMissions:string[]}
export interface Preview {workspace:string;url:string;running:boolean;logs:string}
export const activeStatuses=new Set(['planning','awaiting_plan_approval','planned','running','awaiting_approval']);
export const guildNames:Record<AgentRole,string>={planner:'Shogun',designer:'Shogun',coder:'Samurai',reviewer:'Sentinel',security:'Shadow Council'};
export const roleStation={planner:'shogun',designer:'shogun',coder:'samurai',reviewer:'sentinel',security:'shadow'} as const;
export function displayText(s:string){return s.replace(/\[\[image:[a-f0-9]{64}\]\]/g,'[image attached]');}
export interface Project {id:string;path:string;name:string;connected:boolean;sessions:Session[];missions:Summary[];approvals:number}
export class DaemonClient {
 token='';project='';projectId='';
 constructor(project?:Pick<Project,'id'|'path'>,token=''){this.project=project?.path||'';this.projectId=project?.id||'';this.token=token;}
 url(path:string){return this.projectId && (path.startsWith('/api/')||path==='/bridge/connect'||path==='/health')?`/p/${this.projectId}${path}`:path;}
 async initialize(){const c=await this.request<{project:string;projectId:string;token:string}>('/bridge/config');this.token=c.token;this.project=c.project;this.projectId=c.projectId;}
 async request<T=unknown>(path:string,body?:unknown):Promise<T>{
  const res=await fetch(this.url(path),{method:body===undefined?'GET':'POST',headers:{'Content-Type':'application/json','X-Maestro-UI-Token':this.token},body:body===undefined?undefined:JSON.stringify(body)});
  const data=await res.json().catch(()=>({error:`HTTP ${res.status}`}));
  if(!res.ok)throw new Error(typeof data.error==='string'?data.error:typeof data.message==='string'?data.message:`Request failed: HTTP ${res.status}`);
  return data as T;
 }
}
