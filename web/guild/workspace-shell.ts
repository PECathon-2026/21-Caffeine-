import {h,adoptCss} from '../ui/dom.ts';
import {DaemonClient,type Project,type Session} from './daemon.ts';
import {createSessionWorkspace} from './live-app.ts';
import css from './workspace-shell.css';

const button=(label:string,action:()=>void,cls='')=>h('button',{class:cls,attrs:{type:'button'},on:{click:action}},label);
type Workspace=ReturnType<typeof createSessionWorkspace>;
export function mountLiveGuild(){
 adoptCss(css);
 const bridge=new DaemonClient();
 const shell=h('div',{class:'workspace-shell'}),rail=h('aside',{class:'project-rail',attrs:{'aria-label':'Projects and sessions'}}),host=h('div',{class:'session-host'});
 const mobile=button('☰',()=>shell.classList.toggle('rail-open'),'rail-toggle');mobile.setAttribute('aria-label','Toggle projects');
 shell.append(rail,host,mobile);document.body.replaceChildren(shell);
 let projects:Project[]=[],current='',error='',loading=false,refreshing=false;
 const controllers=new Map<string,Workspace>(),pending=new Map<string,boolean>(),expanded=new Set<string>();
 const key=(p:Project,s:Session)=>`${p.id}:${s.id}`;
 function updatePending(k:string,busy:boolean){if(pending.get(k)!==busy){pending.set(k,busy);renderRail();}}
 function title(p:Project,s:Session){return localStorage.getItem(`maestro-title:${p.id}:${s.id}`)||s.title||'New conversation';}
 function status(p:Project,s:Session){
  if(pending.get(key(p,s)))return 'thinking';
  if(!p.connected)return 'offline';
  return p.missions.find(m=>m.mission.id===s.activeMissionId)?.mission.status||'idle';
 }
 function select(p:Project,s:Session){
  const k=key(p,s);if(current!==k)controllers.get(current)?.setActive(false);
  current=k;localStorage.setItem('maestro-workspace',JSON.stringify({projectId:p.id,sessionId:s.id}));localStorage.setItem(`maestro-session:${p.path}`,s.id);expanded.add(p.id);
  let controller=controllers.get(k);
  if(!controller){controller=createSessionWorkspace(p,{...s,title:title(p,s)},bridge.token,b=>updatePending(k,b),()=>void newSession(p),other=>select(p,other));controllers.set(k,controller);}
  host.replaceChildren(controller.element);controller.setActive(true);shell.classList.remove('rail-open');renderRail();
 }
 async function refreshProjects(){
  if(refreshing)return;refreshing=true;
  try{const loaded=await bridge.request<Project[]>('/bridge/projects');projects=loaded.map(p=>{const previous=projects.find(old=>old.id===p.id);return !p.connected&&previous?{...p,sessions:previous.sessions,missions:previous.missions}:p;});error='';renderRail();}
  catch(e){error=(e as Error).message;renderRail();}finally{refreshing=false;}
 }
 async function openProject(p:Project){
  try{
   loading=true;renderRail();const client=new DaemonClient(p,bridge.token);await client.request('/bridge/connect',{});await refreshProjects();
   const fresh=projects.find(v=>v.id===p.id)||p;const remembered=localStorage.getItem(`maestro-session:${p.path}`);const s=fresh.sessions.find(v=>v.id===remembered)||fresh.sessions[0];
   if(s)select(fresh,s);else await newSession(fresh,false);
  }catch(e){error=(e as Error).message;}finally{loading=false;renderRail();}
 }
 async function newSession(p:Project,focus=true){
  try{
   const client=new DaemonClient(p,bridge.token);await client.request('/bridge/connect',{});
   const s=await client.request<Session>('/api/chat/sessions',{title:'New conversation'});
   p.sessions.unshift(s);select(p,s);await refreshProjects();
   if(focus)host.querySelector<HTMLTextAreaElement>('textarea')?.focus();
  }catch(e){error=(e as Error).message;renderRail();}
 }
 function renderRail(){
  const previous=rail.querySelector('.project-list')?.scrollTop||0;
  const list=h('div',{class:'project-list'});
  for(const p of projects){
   const active=p.id===current.split(':')[0];
   const heading=button(p.name,()=>{expanded.add(p.id);void openProject(p);},`project-name${active?' active':''}`);heading.title=p.path;
   const add=button('＋',()=>void newSession(p),'project-add');add.setAttribute('aria-label',`New session in ${p.name}`);
   const fold=button(expanded.has(p.id)?'⌄':'›',()=>{expanded.has(p.id)?expanded.delete(p.id):expanded.add(p.id);renderRail();},'project-fold');fold.setAttribute('aria-label',`Toggle ${p.name}`);
   const group=h('section',{class:'project-group'},h('div',{class:'project-heading'},fold,heading,add));
   if(expanded.has(p.id)){
    for(const s of p.sessions){
     const state=status(p,s),k=key(p,s),item=button('',()=>select(p,s),`session-item${current===k?' selected':''}`);
     item.title=`${title(p,s)} · ${state.replaceAll('_',' ')}`;item.setAttribute('aria-current',String(current===k));item.dataset.sessionId=s.id;item.dataset.projectId=p.id;
     item.append(h('span',{class:`session-dot state-${state}`,attrs:{'aria-label':state.replaceAll('_',' ')}}),h('span',{class:'session-label'},title(p,s)));
     if(state==='awaiting_approval'||state==='awaiting_plan_approval')item.append(h('span',{class:'session-alert'},'Review'));
     group.append(item);
    }
    if(!p.sessions.length)group.append(h('p',{class:'project-hint'},p.connected?'No sessions yet':'Open to load sessions'));
   }
   list.append(group);
  }
  rail.replaceChildren(h('div',{class:'rail-brand'},h('strong',null,'MAESTRO'),h('span',null,'THE ENGINEERING GUILD')),button('＋ Open project',()=>void folderDialog(),'open-project'),list,h('div',{class:'rail-footer'},error?h('p',{class:'rail-error',attrs:{role:'status'}},error):h('span',null,loading?'Opening project…':`${projects.length} ${projects.length===1?'project':'projects'} · work continues in the background`)));
  list.scrollTop=previous;
 }
 async function folderDialog(){
  const path=h('input',{attrs:{'aria-label':'Folder path',placeholder:'/home/you/projects/app'}}),rows=h('div',{class:'folder-list'}),message=h('p',{class:'folder-message',attrs:{role:'status'}}),name=h('input',{attrs:{'aria-label':'New folder name',placeholder:'New folder name'}});
  let directory='';let request=0;
  const dialog=h('dialog',{class:'folder-dialog'},h('div',{class:'folder-dialog-title'},h('h2',null,'Open project'),button('×',()=>dialog.close())),h('p',null,'Choose a folder. Each project keeps its own sessions and running agents.'),h('form',{class:'folder-path',on:{submit:(e:Event)=>{e.preventDefault();void browse(path.value);}}},path,button('Go',()=>void browse(path.value))),message,rows,
   h('details',{class:'create-folder'},h('summary',null,'Create a new project folder'),h('div',null,name,button('Create',()=>void create()))),h('div',{class:'folder-actions'},button('Cancel',()=>dialog.close()),button('Open this folder',()=>void open(),'primary')));
  document.body.append(dialog);dialog.addEventListener('close',()=>dialog.remove());dialog.showModal();
  async function browse(value?:string){const seq=++request;try{const data=await bridge.request<{path:string;parent:string;folders:{name:string;path:string}[]}>(`/bridge/folders${value?'?path='+encodeURIComponent(value):''}`);if(seq!==request||!dialog.open)return;directory=data.path;path.value=data.path;message.textContent='';rows.replaceChildren(button('↑ Parent folder',()=>void browse(data.parent)),...data.folders.map(f=>button(`▸ ${f.name}`,()=>void browse(f.path))));}catch(e){if(seq===request)message.textContent=(e as Error).message;}}
  async function open(){try{const p=await bridge.request<Project>('/bridge/projects',{path:path.value||directory});dialog.close();expanded.add(p.id);await refreshProjects();await openProject(projects.find(v=>v.id===p.id)||{...p,sessions:[],missions:[],approvals:0,connected:false});}catch(e){message.textContent=(e as Error).message;}}
  async function create(){try{const p=await bridge.request<Project>('/bridge/folders',{parent:path.value||directory,name:name.value});dialog.close();expanded.add(p.id);await refreshProjects();await openProject(projects.find(v=>v.id===p.id)||{...p,sessions:[],missions:[],approvals:0,connected:false});}catch(e){message.textContent=(e as Error).message;}}
  await browse();path.focus();
 }
 async function boot(){
  try{
   await bridge.initialize();await refreshProjects();projects.forEach(p=>expanded.add(p.id));
   let saved:{projectId?:string;sessionId?:string}={};try{saved=JSON.parse(localStorage.getItem('maestro-workspace')||'{}');}catch{}
   const p=projects.find(p=>p.id===saved.projectId)||projects.find(p=>p.id===bridge.projectId)||projects[0];
   if(p){if(saved.sessionId)localStorage.setItem(`maestro-session:${p.path}`,saved.sessionId);await openProject(p);}
  }catch(e){error=(e as Error).message;renderRail();}
 }
 setInterval(()=>{if(!document.hidden)void refreshProjects();},4000);
 window.addEventListener('beforeunload',()=>controllers.forEach(c=>c.dispose()));
 renderRail();void boot();
}
