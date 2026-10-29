import { h, adoptCss } from '../ui/dom.ts';
import { renderMarkdown } from '../ui/control/chat/markdown.ts';
import baseCss from './styles.css';
import sceneCss from './cockpit.css';
import css from './live.css';
import { createGuildScene } from './scene.ts';
import { characterPixels } from './characters.ts';
import type { Phase, Role } from './mission.ts';
import { DaemonClient,guildNames,roleStation,displayText,activeStatuses,type Snapshot,type Detail,type RoleModel,type Session,type ChatMessage,type EventRecord,type Version,type RestorePreview,type Preview,type Project } from './daemon.ts';

type View='guild'|'conversation'|'tasks'|'evidence'|'history'|'sessions'|'settings';
const labels:Record<View,string>={guild:'Guild',conversation:'Conversation',tasks:'Tasks',evidence:'Evidence & approvals',history:'History',sessions:'Sessions',settings:'Connection'};
const workingRoles=['planner','coder','reviewer','security'] as const;
const money=(n:number)=>`$${((n||0)/1_000_000).toFixed(3)}`;
const clock=(n:number)=>new Date(n).toLocaleTimeString([],{hour:'2-digit',minute:'2-digit'});
const btn=(label:string,fn:()=>void,primary=false)=>h('button',{class:primary?'live-btn primary':'live-btn',attrs:{type:'button'},on:{click:fn}},label);
const badge=(value:string)=>h('span',{class:`live-badge status-${value.replaceAll('_','-')}`},value.replaceAll('_',' '));
const markdown=(value:string)=>{const div=h('div',{class:'live-markdown'});renderMarkdown(document,div,displayText(value),{onCopy:(text,button)=>{void navigator.clipboard.writeText(text).then(()=>{button.textContent='Copied';}).catch(()=>{button.textContent='Copy unavailable';});}});return div;};
const pre=(value:string)=>h('pre',{class:'live-pre'},displayText(value));
const card=(title:string,...children:(HTMLElement|string|undefined|false)[])=>h('section',{class:'live-card'},h('h2',null,title),...children);
const imageViews=(text:string,api:DaemonClient)=>Array.from(new Set(Array.from(text.matchAll(/\[\[image:([a-f0-9]{64})\]\]/g),m=>m[1]!))).slice(-4).map(id=>h('a',{attrs:{href:api.url(`/api/images/${id}`),target:'_blank',rel:'noopener noreferrer'}},h('img',{class:'live-image',attrs:{src:api.url(`/api/images/${id}`),alt:'Attached reference image',loading:'lazy'}})));
const details=(title:string,body:string)=>h('details',{class:'live-details'},h('summary',null,title),pre(body));
const empty=(text:string)=>h('p',{class:'live-empty'},text);
function portrait(role:Role){const span=h('span',{class:'live-portrait'});span.innerHTML=`<svg viewBox="-6 -5 34 32" aria-hidden="true">${characterPixels(role)}</svg>`;return span;}

export function createSessionWorkspace(project:Project,session:Session,token:string,onChange:(busy:boolean)=>void,onNewSession:()=>void,onOpenSession:(session:Session)=>void){
 adoptCss(baseCss);adoptCss(sceneCss);adoptCss(css);
 const api=new DaemonClient(project,token);
 let isActive=true;
  let connected=false,error='',busy=false,view:View='guild',snapshot:Snapshot|undefined,detail:Detail|undefined,models:RoleModel[]=[],sessions:Session[]=[],messages:ChatMessage[]=[],events:EventRecord[]=[],versions:Version[]=[],previews:Preview[]=[],sessionId=session.id,selected=session.activeMissionId||'',diff='';
 let stream:EventSource|undefined,refreshTimer:ReturnType<typeof setTimeout>|undefined,refreshing=false,refreshAgain=false,day=localStorage.getItem('maestro-theme')==='light',motion=true,attachments:{imageId:string;name:string}[]=[],uploading=false;
 const root=h('div',{class:'live-app'}),header=h('header',{class:'live-header'}),nav=h('nav',{class:'live-nav',attrs:{'aria-label':'Workspace navigation'}}),banner=h('div',{class:'live-mission'}),content=h('main',{class:'live-content',attrs:{id:'main-content'}}),notice=h('div',{class:'live-notice',attrs:{role:'status'}});
 const draftKey=`maestro-draft:${project.id}:${session.id}`;
 const textarea=h('textarea',{attrs:{placeholder:'Ask a question, or describe what you want the guild to build…','aria-label':'Message to Maestro',rows:1}});
 const mode=h('select',{attrs:{'aria-label':'Message mode'}},h('option',{attrs:{value:'chat'}},'Chat / auto'),h('option',{attrs:{value:'mission'}},'New mission'),h('option',{attrs:{value:'steer'}},'Steer mission'));
 const attachList=h('div',{class:'live-attachments'}),send=btn('Send',()=>void sendMessage(),true),attach=btn('Attach image',()=>fileInput.click()),fileInput=h('input',{attrs:{type:'file',accept:'image/png,image/jpeg,image/webp',multiple:true,hidden:true}});
 const collapse=btn('⌄',()=>{composer.classList.remove('expanded');textarea.blur();});collapse.setAttribute('aria-label','Collapse composer');
 const composer=h('form',{class:'live-composer',on:{submit:(e:Event)=>{e.preventDefault();void sendMessage();}}},h('div',{class:'composer-input'},textarea,send),attachList,h('div',{class:'live-compose-actions'},attach,mode,h('span',{class:'live-compose-hint'},'Enter ↵ · Shift+Enter for a new line'),collapse),fileInput);
 textarea.value=localStorage.getItem(draftKey)||'';
 textarea.addEventListener('input',()=>localStorage.setItem(draftKey,textarea.value));
 textarea.addEventListener('focus',()=>composer.classList.add('expanded'));
 textarea.addEventListener('keydown',e=>{if(e.key==='Escape'){composer.classList.remove('expanded');textarea.blur();}});
 send.setAttribute('aria-label','Send message');attach.textContent='＋';attach.setAttribute('aria-label','Attach image');
 textarea.addEventListener('keydown',e=>{if(e.key==='Enter'&&!e.shiftKey&&!e.isComposing){e.preventDefault();void sendMessage();}});
 const scene=createGuildScene(()=>navigate('tasks'),()=>navigate('evidence'));
 const world=h('section',{class:'world-panel live-world'},h('div',{class:'world-map'},scene.element),scene.navigation.controls);
 const media=matchMedia('(prefers-reduced-motion: reduce)');media.addEventListener('change',()=>render());
 root.append(header,nav,banner,notice,content,composer);
 function notify(message:string){error=message;notice.textContent=message;notice.hidden=!message;}
 function navigate(next:View){view=next;render();if(next==='history'||next==='settings'||next==='evidence')void loadExtras();}
 function chooseMission(id:string){if(selected!==id){selected=id;events=[];detail=undefined;openStream();}void refresh();}
 async function run(action:()=>Promise<unknown>){if(busy)return;busy=true;updateComposer();try{await action();error='';await refresh();await loadExtras();}catch(e){notify((e as Error).message);}finally{busy=false;render();}}
 function updateComposer(){send.disabled=busy||uploading||!connected;send.textContent=busy?'…':'↑';onChange(busy);attach.disabled=busy||uploading||!connected;mode.disabled=busy;attachList.replaceChildren(...attachments.map((a,i)=>btn(`${a.name} ×`,()=>{attachments.splice(i,1);updateComposer();})));}
 async function sendMessage(){
  const text=textarea.value.trim();if(!text||busy||uploading||!connected)return;
  const imageIds=attachments.map(a=>a.imageId);busy=true;error='';if(!localStorage.getItem(`maestro-title:${project.id}:${session.id}`)&&(session.title==='New conversation'||session.title==='New chat'))localStorage.setItem(`maestro-title:${project.id}:${session.id}`,text.slice(0,80));render();
  try{
   if(mode.value==='mission'){
    const objective=text+imageIds.map(id=>`\n[[image:${id}]]`).join('');
    const response=await api.request<{missionId:string}>('/api/missions',{objective,sessionId});chooseMission(response.missionId);
   }else if(mode.value==='steer'){
    if(!selected)throw new Error('Select a mission before steering');
    await api.request(`/api/missions/${selected}/steer`,{text:text+imageIds.map(id=>`\n[[image:${id}]]`).join('')});
   }else{
    const response=await api.request<{missionId?:string;messages:ChatMessage[]}>(`/api/chat/${sessionId}/messages`,{text,images:imageIds});
    if(response.missionId)chooseMission(response.missionId);
   }
   if(textarea.value.trim()===text){textarea.value='';localStorage.removeItem(draftKey);}attachments=[];view='conversation';await loadMessages();await refresh();
  }catch(e){notify((e as Error).message);}finally{busy=false;render();}
 }
 async function uploadFiles(files:File[]){if(files.length+attachments.length>4){notify('Attach at most four images.');return;}
  uploading=true;updateComposer();
  try{for(const file of files){if(file.size>12*1024*1024)throw new Error(`${file.name} exceeds 12 MiB`);const dataUrl=await new Promise<string>((resolve,reject)=>{const reader=new FileReader();reader.onload=()=>resolve(String(reader.result));reader.onerror=()=>reject(new Error('Image could not be read'));reader.readAsDataURL(file);});attachments.push(await api.request<{imageId:string;name:string}>('/api/images',{name:file.name,dataUrl}));}}
  catch(e){notify((e as Error).message);}finally{uploading=false;updateComposer();}
 }
 fileInput.addEventListener('change',()=>{const files=Array.from(fileInput.files||[]);fileInput.value='';void uploadFiles(files);});
 textarea.addEventListener('paste',e=>{const files=Array.from(e.clipboardData?.files||[]).filter(f=>f.type.startsWith('image/'));if(files.length){e.preventDefault();void uploadFiles(files);}});
 root.addEventListener('dragover',e=>{if(e.dataTransfer?.types.includes('Files'))e.preventDefault();});
 root.addEventListener('drop',e=>{const files=Array.from(e.dataTransfer?.files||[]).filter(f=>f.type.startsWith('image/'));if(files.length){e.preventDefault();void uploadFiles(files);}});
 async function loadMessages(){
  if(sessionId)messages=await api.request<ChatMessage[]>(`/api/chat/${sessionId}/messages`);
  const titleKey=`maestro-title:${project.id}:${session.id}`;
  if(!localStorage.getItem(titleKey)&&['New conversation','New chat'].includes(session.title)){
   const first=messages.find(m=>m.role==='user');if(first)localStorage.setItem(titleKey,displayText(first.content).slice(0,80));
  }
 }
 async function refresh(){
  if(refreshing){refreshAgain=true;return;}refreshing=true;
  try{
   snapshot=await api.request<Snapshot>('/api/state');connected=true;
   const requested=selected;if(requested){const loaded=await api.request<Detail>(`/api/missions/${requested}`);if(requested===selected)detail=loaded;}
   sessions=await api.request<Session[]>('/api/chat/sessions');
   const currentSession=sessions.find(s=>s.id===sessionId);const currentMission=currentSession?.activeMissionId||'';
   if(currentSession&&currentMission!==selected){selected=currentMission;detail=undefined;events=[];openStream();refreshAgain=true;}
   await loadMessages();render();
  }catch(e){connected=false;notify((e as Error).message);render();}
  finally{refreshing=false;if(refreshAgain){refreshAgain=false;void refresh();}}
 }
 async function loadExtras(){
  try{
   if(view==='history')versions=(await api.request<{versions:Version[]}>('/api/history')).versions;
   if(view==='settings')previews=await api.request<Preview[]>('/api/previews');
   if(view==='evidence'&&selected){const id=selected;const value=await api.request<{unified:string}>(`/api/missions/${id}/diff`);if(selected===id)diff=value.unified;}
   render();
  }catch(e){notify((e as Error).message);}
 }
 function openStream(){
  stream?.close();if(!selected)return;
  const missionId=selected;stream=new EventSource(api.url(`/api/events?missionId=${encodeURIComponent(missionId)}&after=0`));
  stream.onmessage=e=>{
   if(selected!==missionId)return;
   const event=JSON.parse(e.data) as EventRecord;if(!events.some(v=>v.seq===event.seq))events=[...events,event].slice(-500);
   if(!refreshTimer)refreshTimer=setTimeout(()=>{refreshTimer=undefined;void refresh();},350);
  };
  stream.onerror=()=>{notice.textContent='Reconnecting to the event stream…';};
  stream.onopen=()=>{if(!error)notice.textContent='';};
 }
 function dialog(title:string,body:HTMLElement,actions:HTMLElement[]=[]){
  const d=h('dialog',{class:'live-dialog'},h('div',{class:'live-dialog-head'},h('h2',null,title),btn('Close',()=>d.close())),body,h('div',{class:'live-dialog-actions'},...actions));
  if(isActive)document.body.append(d);d.addEventListener('close',()=>d.remove());if(isActive)d.showModal();return d;
 }
 function textDialog(title:string,label:string,action:(text:string)=>Promise<unknown>){
  const input=h('input',{attrs:{'aria-label':label,placeholder:label,required:true}});let d:HTMLDialogElement;
  d=dialog(title,h('label',null,label,input),[btn('Save',()=>{if(input.value.trim())void run(async()=>{await action(input.value.trim());d.close();});},true)]);input.focus();
 }
 async function previewRestore(id?:string){
  try{
   const path=id?`/api/history/${id}/restore`:'/api/history/undo';const p=await api.request<RestorePreview>(path);
   let d:HTMLDialogElement;const apply=btn('Apply this restore',()=>void run(async()=>{await api.request(path,{token:p.token});d.close();}),true);apply.disabled=p.conflicts.length>0;
   d=dialog(`Restore: ${p.target.label}`,h('div',null,h('p',null,`This will retire ${p.affectedMissions.length} previous mission(s). Manual edits are preserved where they merge cleanly. A recovery version is saved first.`),p.conflicts.length>0&&h('p',{class:'live-error'},p.conflicts.join('\n')),pre(p.unified||'No file differences.')),[apply]);
  }catch(e){notify((e as Error).message);}
 }
 async function approvalDialog(id:string){
  const a=snapshot?.approvals.find(a=>a.id===id);if(!a)return;
  try{
   const delta=await api.request<{unified:string}>(`/api/missions/${a.missionId}/diff`);let d:HTMLDialogElement;
   d=dialog(`Review ${a.action}`,h('div',null,h('p',null,`Resource: ${a.resource}`),pre(delta.unified||JSON.stringify(a.policySnapshot,null,2))),[
    btn('Reject',()=>void run(async()=>{await api.request(`/api/approvals/${id}`,{approve:false});d.close();})),
    btn('Approve',()=>void run(async()=>{await api.request(`/api/approvals/${id}`,{approve:true});d.close();}),true)]);
  }catch(e){notify((e as Error).message);}
 }
 function exportEvidence(){const url=URL.createObjectURL(new Blob([JSON.stringify({project:api.project,mission:detail,events},null,2)],{type:'application/json'}));const a=h('a',{attrs:{href:url,download:'maestro-live-evidence.json'}});a.click();setTimeout(()=>URL.revokeObjectURL(url),1000);}
 function team(){return card('The engineering guild',...workingRoles.map(role=>{
  const running=detail?.tasks.filter(t=>t.role===role&&t.status==='running')||[];const assigned=models.find(m=>m.role===role);const status=role==='planner'&&(detail?.mission.status==='planning'||busy)?'running':running.length?'running':detail?.tasks.filter(t=>t.role===role).at(-1)?.status||'idle';
  return h('article',{class:'live-role'},portrait(roleStation[role]),h('div',null,h('strong',null,guildNames[role]),h('span',{class:'live-model'},assigned?.model||'Loading model…'),badge(status),...running.map(t=>h('p',null,t.title))));
 }),h('p',{class:'live-muted'},'Shogun plans · Samurai builds · Sentinel tests · Shadow reviews'));}
 function guildPage(){return h('div',{class:'live-guild-grid'},world,team());}
 function conversation(){
  const log=h('div',{class:'live-chat-log'});
  if(!messages.length)log.append(empty('Start a conversation. Questions go to the architect; implementation requests launch a mission.'));
  for(const m of messages)log.append(h('article',{class:`live-message ${m.role}`},h('header',null,h('strong',null,m.role),h('time',null,clock(m.createdAt))),(m.role==='assistant'||m.role==='agent')?markdown(m.content):pre(m.content),...imageViews(m.content,api)));
  const recent=events.filter(e=>/tool\.(call|result)|task\.status|mission\.status/.test(e.kind));
  if(recent.length)log.append(card('Agent activity',...recent.slice(-80).map(e=>details(`${clock(e.createdAt)} · ${guildNames[detail?.tasks.find(t=>t.id===e.taskId)?.role||'planner']} · ${e.kind}`,JSON.stringify(e.payload,null,2)))));
  if(busy)log.append(h('p',{class:'live-working',attrs:{role:'status'}},'Maestro is working on your message…'));
  return log;
 }
 function tasksPage(){
  const tasks=detail?.tasks||[];return h('div',{class:'live-page'},h('div',{class:'live-page-heading'},h('h1',null,'Plan & tasks'),btn('Inspect plan',()=>dialog('Mission plan',pre(JSON.stringify(detail?.plan||{},null,2))))),
   !tasks.length&&empty('The planner has not created tasks yet.'),
   ...tasks.map(t=>card(t.title,h('div',{class:'live-row'},badge(t.status),badge(guildNames[t.role]),h('span',null,`${t.attempts} attempt(s)`)),h('p',null,t.description),t.statusReason&&h('p',{class:'live-error'},t.statusReason),t.summary&&details('Agent result',t.summary),h('p',{class:'live-muted'},`Depends on: ${t.dependencies.join(', ')||'No dependencies'}`),h('ul',null,...t.acceptance.map(a=>h('li',null,a))),['blocked','failed'].includes(t.status)&&btn('Retry task',()=>void run(()=>api.request(`/api/missions/${selected}/retry`,{taskId:t.id}))))),
   detail?.mission.status==='awaiting_plan_approval'&&h('div',{class:'live-row'},btn('Approve plan',()=>void run(()=>api.request(`/api/missions/${selected}/plan/approve`,{approve:true})),true),btn('Reject plan',()=>void run(()=>api.request(`/api/missions/${selected}/plan/approve`,{approve:false})))));
 }
 function evidencePage(){const approvals=snapshot?.approvals.filter(a=>a.missionId===selected)||[];
  return h('div',{class:'live-page'},h('div',{class:'live-page-heading'},h('h1',null,'Evidence & approvals'),btn('Export evidence',exportEvidence)),
   card('Human approval',...approvals.map(a=>h('div',{class:'live-row'},h('span',null,`${a.action}: ${a.resource}`),btn('Inspect & decide',()=>void approvalDialog(a.id),true))),!approvals.length&&empty('No pending approvals for this mission.')),
   card('Changes',pre(diff||'No diff available for this mission.')),
   card('Execution record',...events.slice(-200).map(e=>details(`${e.seq} · ${e.kind}`,JSON.stringify(e.payload,null,2)))));
 }
 function historyPage(){return h('div',{class:'live-page'},h('div',{class:'live-page-heading'},h('h1',null,'Code history'),btn('Save backup',()=>textDialog('Save a version','Version name',label=>api.request('/api/history',{label}))),btn('Undo latest request',()=>void previewRestore())),
  !versions.length&&empty('Code versions appear after a mission or a saved backup.'),...versions.slice().sort((a,b)=>b.createdAt-a.createdAt).map(v=>card(v.label,h('div',{class:'live-row'},badge(v.kind),h('time',null,new Date(v.createdAt).toLocaleString()),btn('View diff',()=>void run(async()=>{const d=await api.request<{unified:string}>(`/api/history/${v.id}/diff`);dialog(v.label,pre(d.unified||'No changes'));})),btn('Restore…',()=>void previewRestore(v.id))))));}
 function sessionsPage(){return h('div',{class:'live-page'},h('h1',null,'Conversations'),btn('New conversation',newSession),...sessions.map(s=>btn(s.title||'Untitled conversation',()=>onOpenSession(s))));}

 function newSession(){onNewSession();}

 function settingsPage(){return h('div',{class:'live-page'},h('h1',null,'Connection & local previews'),card('Connected project',h('p',null,api.project),h('div',{class:'live-row'},badge(connected?'connected':'offline'),btn('Connect daemon',()=>void connect())),h('p',{class:'live-muted'},'Other projects and sessions continue running when you switch.')),
  card('Local previews',...previews.map(p=>h('div',{class:'live-preview'},h('a',{attrs:{href:p.url,target:'_blank',rel:'noopener noreferrer'}},p.url),badge(p.running?'running':'stopped'),details('Recent logs',p.logs))),!previews.length&&empty('Ask Maestro to start the app locally to create a preview.'),previews.length>0&&btn('Stop managed previews',()=>void run(()=>api.request('/api/previews/stop',{})))));}
 function render(){
  if(isActive)document.documentElement.dataset.theme=day?'light':'dark';root.dataset.uiMotion=motion&&!media.matches?'on':'off';
  const appearance=h('details',{class:'session-options'},h('summary',null,'Appearance'),h('div',null,btn(day?'Night shift':'Daylight',()=>{day=!day;localStorage.setItem('maestro-theme',day?'light':'dark');render();}),btn(motion?'Pause scenery':'Resume scenery',()=>{motion=!motion;render();})));
  header.replaceChildren(h('div',{class:'session-heading'},h('span',null,project.name),h('strong',null,localStorage.getItem(`maestro-title:${project.id}:${session.id}`)||sessions.find(s=>s.id===sessionId)?.title||session.title||'New conversation')),badge(connected?'connected':'offline'),appearance);

  nav.replaceChildren(...(Object.keys(labels) as View[]).filter(id=>id!=='sessions').map(id=>{const b=btn(labels[id],()=>navigate(id));b.setAttribute('aria-current',String(view===id));return b;}));

  const stats=detail?.stats;
  banner.replaceChildren(h('div',{class:'live-mission-title'},h('strong',null,detail?displayText(detail.mission.objective):'What shall we build?'),h('div',{class:'live-row'},badge(detail?.mission.status||'idle'),stats&&h('span',null,`${((stats.inputTokens+stats.outputTokens)||0).toLocaleString()} tokens · ${money(stats.costMicroUsd)}`))),
   h('div',{class:'live-row'},!!selected&&['running','planning'].includes(detail?.mission.status||'')&&btn('Pause mission',()=>void run(()=>api.request(`/api/missions/${selected}/pause`,{}))),!!selected&&activeStatuses.has(detail?.mission.status||'')&&btn('Cancel',()=>void run(()=>api.request(`/api/missions/${selected}/cancel`,{}))),!!selected&&['paused','blocked','failed','cancelled'].includes(detail?.mission.status||'')&&btn('Resume',()=>void run(()=>api.request(`/api/missions/${selected}/resume`,{})))));
  banner.hidden=!detail;
  notice.textContent=error;notice.hidden=!error;
  const expanded=new Set(Array.from(content.querySelectorAll('details[open]')).map(d=>d.querySelector('summary')?.textContent));
  const focused=document.activeElement instanceof HTMLButtonElement ? document.activeElement.textContent : null;
  const previousScroll=content.scrollTop;const atEnd=content.scrollHeight-content.clientHeight-content.scrollTop<50;
  const active=detail?.tasks.filter(t=>t.status==='running').map(t=>roleStation[t.role])||[];if(detail?.mission.status==='planning')active.push('shogun');
  const phase:Phase=detail?.mission.status==='completed'?'complete':detail?.mission.status==='awaiting_approval'?'approval':active.includes('samurai')?'building':active.includes('shadow')?'reviewing':active.includes('sentinel')?'verifying':active.length?'planning':'complete';
  scene.update({dark:!day,paused:!motion||!isActive,reduced:media.matches,phase,held:detail?.mission.status==='blocked'&&active.length>0,activeRoles:[...new Set(active)]});
  content.replaceChildren(view==='guild'?guildPage():view==='conversation'?conversation():view==='tasks'?tasksPage():view==='evidence'?evidencePage():view==='history'?historyPage():view==='sessions'?sessionsPage():settingsPage());
  content.querySelectorAll('details').forEach(d=>{if(expanded.has(d.querySelector('summary')?.textContent))d.open=true;});
  content.scrollTop=view==='conversation'&&atEnd?content.scrollHeight:previousScroll;
  if(focused && document.activeElement===document.body){const match=Array.from(root.querySelectorAll('button')).find(b=>b.textContent===focused);match?.focus({preventScroll:true});}
  updateComposer();
 }
 async function connect(){
  try{await api.request('/bridge/connect',{});error='';await refresh();const result=await api.request<{roles:RoleModel[]}>('/api/models');models=result.roles;render();}catch(e){notify((e as Error).message);render();}
 }
 async function boot(){
  try{await connect();openStream();await loadMessages();render();}catch(e){notify((e as Error).message);render();}
 }
 const polling=setInterval(()=>{if(isActive&&!document.hidden)void refresh();},5000);render();void boot();
 return {element:root,setActive(active:boolean){isActive=active;if(active){day=localStorage.getItem('maestro-theme')==='light';void refresh();openStream();}else{stream?.close();composer.classList.remove('expanded');document.querySelectorAll('.live-dialog').forEach(d=>d.remove());}render();},dispose(){stream?.close();clearInterval(polling);scene.dispose();}};
}
