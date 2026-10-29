from pathlib import Path
import subprocess,tempfile,os,json,time,shutil,urllib.request
from playwright.sync_api import sync_playwright
# Manual live integration check: uses configured providers and a disposable Git repository.
# Run with the Maestro browser worker Python after npm run build.
import struct,zlib
def make_reference(path):
 width,height=240,120
 pixels=[]
 for y in range(height):
  row=bytearray([0])
  for x in range(width):
   color=(35,95,220) if 20<=x<95 and 25<=y<95 else ((240,130,30) if (x-175)**2+(y-60)**2<35**2 else (255,255,255))
   row.extend(color)
  pixels.append(bytes(row))
 def chunk(kind,data):return struct.pack('>I',len(data))+kind+data+struct.pack('>I',zlib.crc32(kind+data)&0xffffffff)
 path.write_bytes(b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',width,height,8,2,0,0,0))+chunk(b'IDAT',zlib.compress(b''.join(pixels)))+chunk(b'IEND',b''))

repo=Path(__file__).resolve().parents[1];root=Path(tempfile.mkdtemp(prefix='maestro-web-live-'));port=int(os.environ.get('MAESTRO_TEST_PORT','5174'))
binary=os.environ.get('MAESTRO_BINARY',str(Path.home()/'.local/bin/maestro'))
for args in [('init','-b','main'),('config','user.name','Web Test'),('config','user.email','web@local')]:subprocess.run(['git',*args],cwd=root,check=True,stdout=subprocess.DEVNULL)
(root/'.gitignore').write_text('.maestro/\nweb-projects.json\n');(root/'README.md').write_text('# Live presentation test\nThis is a minimal repository for a reviewed text change. No shell checks are required.\n');make_reference(root/'reference.png')
subprocess.run(['git','add','.'],cwd=root,check=True);subprocess.run(['git','commit','-qm','Initial'],cwd=root,check=True)
server=subprocess.Popen(['node','scripts/preview-guild.mjs'],cwd=repo,env={**os.environ,'MAESTRO_PROJECT':str(root),'MAESTRO_PROJECTS_FILE':str(root/'web-projects.json'),'MAESTRO_PORT':str(port),'MAESTRO_BINARY':binary},stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
print('Root:',root,flush=True);time.sleep(1)
report={'root':str(root),'checks':[]}
try:
 with sync_playwright() as p:
  browser=p.chromium.launch(executable_path=os.environ.get('MAESTRO_CHROMIUM','/usr/bin/chromium'),headless=True,chromium_sandbox=True);page=browser.new_page(viewport={'width':1366,'height':768});errors=[];page.on('pageerror',lambda e:errors.append(str(e)));page.goto(f'http://127.0.0.1:{port}')
  page.get_by_text('gpt-6-astra',exact=True).first.wait_for(timeout=45000)
  assert page.get_by_text('connected',exact=True).count()
  token=page.request.get(f'http://127.0.0.1:{port}/bridge/config').json()['token']
  def api(path,body=None):
   url=f'http://127.0.0.1:{port}'+path
   response=page.request.get(url) if body is None else page.request.post(url,data=json.dumps(body),headers={'Content-Type':'application/json','X-Maestro-UI-Token':token})
   assert response.ok,(response.status,response.text()[:300]);return response.json()
  assert page.request.post(f'http://127.0.0.1:{port}/api/history',data='{"label":"forbidden"}',headers={'Content-Type':'application/json'}).status==403
  report['checks'].append('real daemon auto-start and mutation token enforcement')
  def nav(label):page.get_by_role('navigation').get_by_role('button',name=label,exact=True).click()
  nav('History');page.get_by_role('button',name='Save backup',exact=True).click();page.get_by_role('dialog').get_by_role('textbox').fill('Before presentation');page.get_by_role('dialog').get_by_role('button',name='Save',exact=True).click();page.get_by_role('heading',name='Before presentation',exact=True).wait_for(timeout=15000)
  report['checks'].append('named backup saved through website')
  objective='Create exactly three tasks: Samurai (coder), Sentinel (reviewer), then Shadow Council (security). Samurai: create greeting.txt containing exactly Hello from the live guild followed by a newline. Only greeting.txt may change. Sentinel: read greeting.txt and independently verify it matches that requirement. Shadow Council: independently review the patch for correctness and security; depend on both coder and reviewer. No shell checks or preview needed. Complete the reviewed change and require the final human approval.'
  page.get_by_label('Message to Maestro').fill(objective);page.get_by_role('button',name='Send message',exact=True).click()
  deadline=time.monotonic()+300;last=None;mid=None
  while time.monotonic()<deadline:
   state=api('/api/state');matches=[m for m in state['missions'] if m['mission']['objective']==objective]
   if matches:
    mission=matches[0]['mission'];mid=mission['id'];status=mission['status']
    if status!=last:print('Mission:',status,flush=True);last=status
    if status in ('awaiting_approval','blocked','failed','completed'):break
   page.wait_for_timeout(1500)
  assert last=='awaiting_approval',(last,api('/api/missions/'+mid) if mid else state)
  nav('Tasks');page.screenshot(path='/tmp/maestro-web-live-tasks.png');nav('Evidence & approvals');page.get_by_role('button',name='Inspect & decide',exact=True).click();page.get_by_role('dialog').wait_for();assert 'Hello from the live guild' in page.get_by_role('dialog').inner_text();page.get_by_role('dialog').get_by_role('button',name='Approve',exact=True).click()
  for _ in range(30):
   page.wait_for_timeout(500)
   if (root/'greeting.txt').exists():break
  assert (root/'greeting.txt').read_text()=='Hello from the live guild\n'
  report['checks'].append('live Foundry plan, Samurai, Sentinel, Shadow Council, SSE and reviewed approval integrated real file');report['missionId']=mid
  nav('History');page.get_by_role('button',name='Undo latest request',exact=True).click();page.get_by_role('dialog').wait_for();assert 'greeting.txt' in page.get_by_role('dialog').inner_text();page.get_by_role('dialog').get_by_role('button',name='Apply this restore',exact=True).click()
  for _ in range(30):
   page.wait_for_timeout(500)
   if not (root/'greeting.txt').exists():break
  assert not (root/'greeting.txt').exists();report['checks'].append('previewed undo restored real code')
  page.get_by_role('button',name=f'New session in {root.name}',exact=True).click();page.wait_for_timeout(1000)
  page.locator('input[type=file]').set_input_files(root/'reference.png');page.get_by_role('button',name='reference.png ×',exact=True).wait_for(timeout=10000)
  page.get_by_label('Message to Maestro').fill('What are the shapes and colors in this image, and their left/right positions?');page.get_by_role('button',name='Send message',exact=True).click()
  page.wait_for_function("document.querySelectorAll('.live-message.assistant').length > 0",timeout=180000)
  response=page.locator('.live-message.assistant').last.inner_text().lower();assert 'blue' in response and 'orange' in response,response
  page.wait_for_function("Array.from(document.querySelectorAll('.live-image')).some(i => i.complete && i.naturalWidth > 0)",timeout=15000);report['checks'].append('image upload, persisted image retrieval and live visual chat');page.screenshot(path='/tmp/maestro-web-live-chat.png')
  # A draft must survive normal refreshes and navigation.
  page.get_by_label('Message to Maestro').fill('Draft stays while events refresh');page.wait_for_timeout(11000);assert page.get_by_label('Message to Maestro').input_value()=='Draft stays while events refresh'
  for width,height in [(1366,768),(390,844)]:
   page.set_viewport_size({'width':width,'height':height});nav('Guild');page.get_by_role('button',name='Fit guild',exact=True).click();page.wait_for_timeout(400);assert not page.evaluate('document.documentElement.scrollWidth > innerWidth')
   page.screenshot(path=f'/tmp/maestro-web-live-{width}.png')
  report['checks'].append('draft persistence, laptop and mobile layouts without horizontal overflow');assert not errors,errors
  browser.close()
 print(json.dumps(report),flush=True);Path('/tmp/maestro-web-live-evidence.json').write_text(json.dumps(report,indent=2))
finally:
 try:subprocess.run([binary,'--cwd',str(root),'shutdown'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=10)
 except:pass
 server.terminate();server.wait(timeout=10)
