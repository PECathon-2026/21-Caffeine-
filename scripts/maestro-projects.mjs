import { readFile, writeFile, mkdir, rename, realpath, stat, readdir } from 'node:fs/promises';
import { resolve, dirname, basename, join, isAbsolute } from 'node:path';
import { homedir } from 'node:os';
import { createHash } from 'node:crypto';
import { spawn } from 'node:child_process';

export async function createProjectRegistry({file, initial, binary}) {
  const projects = new Map();
  const starts = new Map();
  let saving = Promise.resolve();
  try {
    for (const p of JSON.parse(await readFile(file, 'utf8'))) {
      if (typeof p.path === 'string' && typeof p.id === 'string') projects.set(p.id, p);
    }
  } catch (e) { if (e.code !== 'ENOENT') throw new Error('Cannot read the saved project registry'); }
  async function persist() {
    saving = saving.catch(() => {}).then(async () => {
      await mkdir(dirname(file), {recursive: true, mode: 0o700});
      const temp = `${file}.${process.pid}.tmp`;
      await writeFile(temp, JSON.stringify([...projects.values()], null, 2), {mode: 0o600});
      await rename(temp, file);
    });
    await saving;
  }
  async function folder(path) {
    if (typeof path !== 'string' || !path.trim()) throw new Error('Choose a folder');
    const expanded = path === '~' ? homedir() : path.startsWith('~/') ? join(homedir(), path.slice(2)) : path;
    if (!isAbsolute(expanded)) throw new Error('Use an absolute folder path');
    const canonical = await realpath(expanded);
    if (!(await stat(canonical)).isDirectory()) throw new Error('Choose a directory');
    return canonical;
  }
  async function add(path) {
    const canonical = await folder(path);
    const id = createHash('sha256').update(canonical).digest('hex').slice(0,24);
    if (!projects.has(id)) {
      projects.set(id, {id, path: canonical, name: basename(canonical) || canonical});
      await persist();
    }
    return projects.get(id);
  }
  function get(id) {
    const p = projects.get(id);
    if (!p) throw Object.assign(new Error('Unknown project'), {status: 404});
    return p;
  }
  async function descriptor(id) {
    const p = get(id);
    const d = JSON.parse(await readFile(join(p.path, '.maestro/daemon.json'), 'utf8'));
    const u = new URL(d.url);
    if (u.protocol !== 'http:' || !['127.0.0.1','localhost','[::1]'].includes(u.hostname) || u.username || u.password) throw new Error('Daemon must use loopback HTTP');
    return d;
  }
  async function request(id, path) {
    const d = await descriptor(id);
    const r = await fetch(new URL(path,d.url), {headers: d.token ? {Authorization: `Bearer ${d.token}`} : {}, signal: AbortSignal.timeout(2500)});
    if (!r.ok) throw new Error('Daemon unavailable');
    return r.json();
  }
  async function connect(id) {
    get(id);
    if (!starts.has(id)) starts.set(id, (async () => {
      try { await request(id, '/health'); return; } catch {}
      const child = spawn(binary, ['--cwd',get(id).path,'serve'], {detached:true,stdio:'ignore'});
      let failure; child.on('error', e => {failure=e;}); child.unref();
      for(let i=0;i<100;i++) {
        if(failure) throw new Error('Cannot launch Maestro; check MAESTRO_BINARY');
        await new Promise(r=>setTimeout(r,150));
        try { await request(id,'/health'); return; } catch {}
      }
      throw new Error('Daemon startup timed out');
    })().finally(()=>starts.delete(id)));
    await starts.get(id);
  }
  async function list() {
    return Promise.all([...projects.values()].map(async p => {
      try {
        const [state,sessions] = await Promise.all([request(p.id,'/api/state'),request(p.id,'/api/chat/sessions')]);
        return {...p,connected:true,sessions,missions:state.missions,approvals:state.approvals.length};
      } catch { return {...p,connected:false,sessions:[],missions:[],approvals:0}; }
    }));
  }
  async function folders(path=homedir()) {
    const current = await folder(path);
    const entries = await readdir(current,{withFileTypes:true});
    const directories = entries.filter(e=>e.isDirectory() && !e.name.startsWith('.')).map(e=>({name:e.name,path:join(current,e.name)})).sort((a,b)=>a.name.localeCompare(b.name));
    return {path:current,parent:dirname(current),folders:directories.slice(0,500),truncated:directories.length>500};
  }
  async function createFolder(parent,name) {
    if(typeof name!=='string' || !name.trim() || name==='.' || name==='..' || /[\/\\\0]/.test(name)) throw new Error('Enter a folder name without slashes');
    const path=join(await folder(parent),name.trim());
    await mkdir(path,{mode:0o755});
    // Only this newly created folder is initialized. Existing repositories are never changed.
    for(const args of [['init','-b','main',path],['-C',path,'-c','user.name=Maestro','-c','user.email=maestro@localhost','-c','commit.gpgsign=false','commit','--allow-empty','-m','Initialize project']]) {
      await new Promise((ok,fail)=>{const c=spawn('git',args,{stdio:'ignore'});c.on('error',fail);c.on('exit',code=>code===0?ok():fail(new Error('Could not initialize the new Git folder')));});
    }
    return add(path);
  }
  const defaultProject = await add(resolve(initial));
  return {add,get,descriptor,connect,list,folders,createFolder,defaultProject};
}
