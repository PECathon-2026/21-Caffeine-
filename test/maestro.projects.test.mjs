import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,mkdir,writeFile,symlink,readFile} from 'node:fs/promises';
import {join,resolve} from 'node:path';
import {tmpdir} from 'node:os';
import {createServer} from 'node:http';
import {spawn,execFileSync} from 'node:child_process';
import {createProjectRegistry} from '../scripts/maestro-projects.mjs';
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
const close=s=>new Promise(r=>{s.closeAllConnections();s.close(r);});

test('canonical projects persist, and new folders have an initial Git revision',async()=>{
 const root=await mkdtemp(join(tmpdir(),'maestro-project-registry-'));const folder=join(root,'existing');await mkdir(folder);await writeFile(join(folder,'keep.txt'),'original');
 const file=join(root,'registry.json');const registry=await createProjectRegistry({file,initial:folder,binary:'not-used'});
 const alias=join(root,'alias');await symlink(folder,alias);assert.equal((await registry.add(alias)).id,registry.defaultProject.id);
 const next=await registry.createFolder(root,'New project');assert.ok(execFileSync('git',['-C',next.path,'rev-parse','HEAD'],{encoding:'utf8'}).trim());
 await assert.rejects(registry.createFolder(root,'../escape'));await assert.rejects(registry.createFolder(root,'existing'));assert.equal(await readFile(join(folder,'keep.txt'),'utf8'),'original');
 const restarted=await createProjectRegistry({file,initial:folder,binary:'not-used'});assert.equal((await restarted.list()).length,2);assert.equal(restarted.get(next.id).path,next.path);
});

test('bridge scopes concurrent requests, images and SSE to the chosen project',async()=>{
 const root=await mkdtemp(join(tmpdir(),'maestro-project-routing-'));const daemons=[];const folders=[];
 for(const name of ['alpha','beta']){
  const dir=join(root,name);folders.push(dir);await mkdir(join(dir,'.maestro'),{recursive:true});
  const server=createServer(async(req,res)=>{
   if(req.headers.authorization!==`Bearer ${name}-private-token`){res.writeHead(401);res.end();return;}
   if(req.url.startsWith('/api/events')){res.writeHead(200,{'Content-Type':'text/event-stream'});res.end(`data: ${JSON.stringify({project:name})}\n\n`);return;}
   if(req.url.startsWith('/api/images/')){res.writeHead(200,{'Content-Type':'image/jpeg'});res.end(name);return;}
   await new Promise(r=>setTimeout(r,name==='alpha'?75:10));
   const payload=req.url==='/api/state'?{missions:[],approvals:[]}:req.url==='/api/chat/sessions'?[{id:`${name}-session`,title:name}]:{project:name};
   res.setHeader('Content-Type','application/json');res.end(JSON.stringify(payload));
  });const port=await listen(server);daemons.push(server);
  await writeFile(join(dir,'.maestro/daemon.json'),JSON.stringify({url:`http://127.0.0.1:${port}`,token:`${name}-private-token`}));
 }
 const reservation=createServer();const port=await listen(reservation);await close(reservation);
 const child=spawn(process.execPath,['scripts/preview-guild.mjs'],{cwd:resolve('.'),stdio:'pipe',env:{...process.env,MAESTRO_PROJECT:folders[0],MAESTRO_PROJECTS_FILE:join(root,'registry.json'),MAESTRO_PORT:String(port)}});
 const base=`http://127.0.0.1:${port}`;
 try{
  let config;for(let i=0;i<100;i++){try{config=await(await fetch(base+'/bridge/config')).json();break;}catch{await new Promise(r=>setTimeout(r,30));}}
  assert.ok(config);const headers={'Content-Type':'application/json','X-Maestro-UI-Token':config.token};
  assert.equal((await fetch(base+'/bridge/projects',{method:'POST',body:JSON.stringify({path:folders[1]})})).status,403);
  const beta=await(await fetch(base+'/bridge/projects',{method:'POST',headers,body:JSON.stringify({path:folders[1]})})).json();
  const ids=[config.projectId,beta.id];
  const replies=await Promise.all(ids.map(id=>fetch(base+`/p/${id}/api/probe`,{method:'POST',headers,body:'{}'}).then(r=>r.json())));assert.deepEqual(replies,[{project:'alpha'},{project:'beta'}]);
  for(const [i,id] of ids.entries()){
   assert.equal(await(await fetch(base+`/p/${id}/api/images/reference`)).text(),['alpha','beta'][i]);
   assert.match(await(await fetch(base+`/p/${id}/api/events`)).text(),new RegExp(['alpha','beta'][i]));
  }
  const projects=await(await fetch(base+'/bridge/projects')).json();assert.equal(projects.length,2);assert.equal(projects.find(p=>p.id===beta.id).sessions[0].id,'beta-session');assert.ok(!JSON.stringify(projects).includes('private-token'));
  assert.equal((await fetch(base+'/p/000000000000000000000000/api/probe')).status,404);
  assert.equal((await fetch(base+'/bridge/folders',{headers:{Origin:'https://unrelated.example'}})).status,403);
 }finally{child.kill();await new Promise(r=>child.once('exit',r));await Promise.all(daemons.map(close));}
});
