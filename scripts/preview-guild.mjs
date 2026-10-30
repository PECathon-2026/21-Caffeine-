// Same-origin local UI bridge. Provider keys and daemon tokens stay server-side.
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve, sep, extname } from 'node:path';
import { randomBytes } from 'node:crypto';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { homedir } from 'node:os';
import { createProjectRegistry } from './maestro-projects.mjs';
const root=resolve('dist/web');
const project=resolve(process.env.MAESTRO_PROJECT||process.cwd());
const port=Number(process.env.MAESTRO_PORT||5173);
const secret=randomBytes(32).toString('hex');
const bundledBinary=fileURLToPath(new URL('../backend/target/release/maestro',import.meta.url));
const binary=process.env.MAESTRO_BINARY||(existsSync(bundledBinary)?bundledBinary:'maestro');
const types={'.html':'text/html','.js':'text/javascript','.svg':'image/svg+xml','.woff2':'font/woff2','.css':'text/css','.png':'image/png','.jpg':'image/jpeg'};
const registry=await createProjectRegistry({file:process.env.MAESTRO_PROJECTS_FILE||resolve(homedir(),'.local/share/maestro/web-projects.json'),initial:project,binary});
async function body(req){const chunks=[];let size=0;for await(const chunk of req){size+=chunk.length;if(size>17*1024*1024)throw Object.assign(new Error('Upload exceeds 17 MiB'),{status:413});chunks.push(chunk);}return Buffer.concat(chunks);}

function json(res,status,value){res.writeHead(status,{'Content-Type':'application/json','Cache-Control':'no-store'});res.end(JSON.stringify(value));}
createServer(async(req,res)=>{
 try{
  const allowedHosts=new Set([`127.0.0.1:${port}`,`localhost:${port}`]);
  if(!allowedHosts.has(req.headers.host)||req.headers['sec-fetch-site']==='cross-site')return json(res,403,{error:'Open this UI from its localhost address'});
  const incoming=new URL(req.url,`http://${req.headers.host}`);
  if(req.headers.origin&&new URL(req.headers.origin).host!==req.headers.host)return json(res,403,{error:'Cross-origin request rejected'});
  let path=decodeURIComponent(incoming.pathname);
  if(!['GET','HEAD'].includes(req.method)&&req.headers['x-maestro-ui-token']!==secret)return json(res,403,{error:'Missing UI request token; reload the page'});
  if(path==='/bridge/config'&&req.method==='GET')return json(res,200,{project,projectId:registry.defaultProject.id,token:secret});
  if(path==='/bridge/projects'&&req.method==='GET')return json(res,200,await registry.list());
  if(path==='/bridge/projects'&&req.method==='POST')return json(res,200,await registry.add(JSON.parse(await body(req)).path));
  if(path==='/bridge/folders'&&req.method==='GET')return json(res,200,await registry.folders(incoming.searchParams.get('path')||undefined));
  if(path==='/bridge/folders'&&req.method==='POST'){const q=JSON.parse(await body(req));return json(res,200,await registry.createFolder(q.parent,q.name));}
  const scoped=path.match(/^\/p\/([a-f0-9]{24})(\/.*)$/);
  const projectId=scoped?scoped[1]:registry.defaultProject.id;
  if(scoped){registry.get(projectId);path=scoped[2];}
  if(path==='/bridge/connect'&&req.method==='POST'){
   await registry.connect(projectId);return json(res,200,{connected:true});
  }
  if(path==='/health'||path.startsWith('/api/')){
   let d;try{d=await registry.descriptor(projectId);}catch{return json(res,503,{error:'Daemon is not connected. Use Connect daemon.'});}
   const controller=new AbortController();res.on('close',()=>controller.abort());
   const headers={'Content-Type':req.headers['content-type']||'application/json'};
   if(d.token)headers.Authorization=`Bearer ${d.token}`;
   if(req.headers['last-event-id'])headers['Last-Event-ID']=req.headers['last-event-id'];
   const options={method:req.method,headers,signal:controller.signal};
   if(!['GET','HEAD'].includes(req.method)){
    options.body=await body(req);
   }
   let response;try{response=await fetch(new URL(path+incoming.search,d.url),options);}catch{if(!res.destroyed)return json(res,503,{error:'Daemon connection lost. Reconnect to resume.'});return;}
   res.writeHead(response.status,{'Content-Type':response.headers.get('content-type')||'application/json','Cache-Control':'no-store','X-Content-Type-Options':'nosniff','X-Accel-Buffering':'no'});
   if(response.body)await pipeline(Readable.fromWeb(response.body),res);else res.end();return;
  }
  if(req.method!=='GET'&&req.method!=='HEAD')return json(res,405,{error:'Method not allowed'});
  const file=resolve(root,'.'+(path==='/'?'/index.html':path));
  if(!file.startsWith(root+sep))return json(res,403,{error:'Invalid asset path'});
  const assetBody=await readFile(file);res.writeHead(200,{'Content-Type':types[extname(file)]||'application/octet-stream','Cache-Control':'no-store','X-Content-Type-Options':'nosniff'});res.end(req.method==='HEAD'?undefined:assetBody);
 }catch(error){if(!res.headersSent)json(res,error.status||(error?.code==='ENOENT'?404:400),{error:error?.code==='ENOENT'?'Folder or file not found':error.message||'Request failed'});else res.destroy();}
}).listen(port,'127.0.0.1',()=>console.log(`Maestro guild: http://127.0.0.1:${port}\nProject: ${project}`));
