# Manual live test: configured model requests in two disposable repositories.
# Responses are held until three submissions exist to make switching races reproducible.
import asyncio,json,os,subprocess,tempfile,time
from pathlib import Path
from playwright.async_api import async_playwright
repo=Path(__file__).resolve().parents[1];root=Path(tempfile.mkdtemp(prefix='maestro-multi-live-'));port=int(os.environ.get('MAESTRO_TEST_PORT','5174'))
folders=[]
for name,description in [('Atlas-Books','Atlas Books is a library inventory app. Project code: ATLAS-ALPHA. It catalogs books and tracks lending.'),('Boreal-Garden','Boreal Garden is a greenhouse sensor app. Project code: ORCHID-BETA. It measures temperature and watering conditions.')]:
 folder=root/name;folder.mkdir();folders.append(folder);(folder/'README.md').write_text('# '+name+'\n'+description+'\n');(folder/'.gitignore').write_text('.maestro/\n')
 for args in [['init','-b','main'],['add','.'],['-c','user.name=Test','-c','user.email=test@local','commit','-qm','initial']]:subprocess.run(['git',*args],cwd=folder,check=True,stdout=subprocess.DEVNULL)
server=subprocess.Popen(['node','scripts/preview-guild.mjs'],cwd=repo,env={**os.environ,'MAESTRO_PROJECT':str(folders[0]),'MAESTRO_PROJECTS_FILE':str(root/'projects.json'),'MAESTRO_PORT':str(port),'MAESTRO_BINARY':os.environ.get('MAESTRO_BINARY',str(Path.home()/'.local/bin/maestro'))},stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
print('Fixture:',root,flush=True)
async def main():
 report={'root':str(root),'checks':[]};release=asyncio.Event();requests=[]
 async with async_playwright() as p:
  browser=await p.chromium.launch(executable_path=os.environ.get('MAESTRO_CHROMIUM','/usr/bin/chromium'),headless=True);page=await browser.new_page(viewport={'width':1440,'height':900});errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
  async def hold(route):
   if route.request.method!='POST':return await route.continue_()
   requests.append(route.request.url)
   response=await route.fetch(timeout=180000);await release.wait();await route.fulfill(response=response)
  await page.route('**/api/chat/*/messages',hold)
  await page.goto(f'http://127.0.0.1:{port}');await page.get_by_role('button',name='New session in Atlas-Books',exact=True).wait_for(timeout=45000)
  await page.wait_for_function("document.querySelector('[aria-label=\"Send message\"]')?.disabled === false",timeout=45000)
  assert not await page.locator('.live-mission').is_visible()
  first=await page.locator('.session-item.selected').get_attribute('data-session-id')
  collapsed=(await page.locator('.live-composer').bounding_box())['height'];assert collapsed<65,collapsed
  await page.get_by_label('Message to Maestro').fill('Read README.md. In two sentences explain this repository and include its project code.');assert (await page.locator('.live-composer').bounding_box())['height']>100
  await page.get_by_role('button',name='Send message',exact=True).click()
  await page.get_by_role('button',name='New session in Atlas-Books',exact=True).click();await page.wait_for_function(f"document.querySelector('.session-item.selected')?.dataset.sessionId !== '{first}'")
  second=await page.locator('.session-item.selected').get_attribute('data-session-id');await page.get_by_label('Message to Maestro').fill('Read README.md and give a second short overview, including the project code.');await page.get_by_role('button',name='Send message',exact=True).click();await page.get_by_label('Message to Maestro').fill('Draft for the second Atlas session')
  await page.get_by_role('button',name='＋ Open project',exact=True).click();await page.get_by_label('Folder path').fill(str(folders[1]));await page.get_by_role('button',name='Open this folder',exact=True).click();await page.get_by_role('button',name='New session in Boreal-Garden',exact=True).wait_for(timeout=45000);await page.wait_for_function("document.querySelector('.session-heading > span')?.textContent==='Boreal-Garden'")
  third=await page.locator('.session-item.selected').get_attribute('data-session-id');await page.get_by_label('Message to Maestro').fill('Read README.md. In two sentences explain this repository and include its project code.');await page.get_by_role('button',name='Send message',exact=True).click()
  await page.wait_for_timeout(500);assert len(requests)==3,requests;assert await page.locator('.session-dot.state-thinking').count()==3
  report['checks'].append('three independent pending real-model chats across two repositories; switching does not cancel requests')
  await page.screenshot(path='/tmp/maestro-multi-pending.png');release.set()
  await page.wait_for_function("document.querySelector('.live-message.assistant')?.textContent.includes('ORCHID-BETA')",timeout=180000)
  assert 'ATLAS-ALPHA' not in await page.locator('.live-chat-log').inner_text()
  await page.locator(f'[data-session-id="{first}"]').click();await page.get_by_role('navigation').get_by_role('button',name='Conversation',exact=True).click();await page.wait_for_function("document.querySelector('.live-message.assistant')?.textContent.includes('ATLAS-ALPHA')",timeout=180000);assert 'ORCHID-BETA' not in await page.locator('.live-chat-log').inner_text()
  await page.locator(f'[data-session-id="{second}"]').click();assert await page.get_by_label('Message to Maestro').input_value()=='Draft for the second Atlas session';await page.get_by_role('navigation').get_by_role('button',name='Conversation',exact=True).click();await page.wait_for_function("document.querySelector('.live-message.assistant')?.textContent.includes('ATLAS-ALPHA')",timeout=180000)
  report['checks'].append('responses and drafts remain in the originating project and session')
  await page.get_by_label('Message to Maestro').focus();await page.get_by_role('button',name='Collapse composer',exact=True).click();assert (await page.locator('.live-composer').bounding_box())['height']<65
  assert not await page.locator('.live-mission').is_visible();await page.screenshot(path='/tmp/maestro-multi-conversation.png')
  await page.reload();await page.wait_for_function(f"document.querySelector('.session-item.selected')?.dataset.sessionId === '{second}'",timeout=30000);assert await page.get_by_label('Message to Maestro').input_value()=='Draft for the second Atlas session'
  report['checks'].append('selected project/session and draft restored after reload; idle mission bar absent; composer expands and collapses')
  for w,h in [(1440,900),(1366,768),(390,844)]:
   await page.set_viewport_size({'width':w,'height':h});await page.get_by_role('navigation').get_by_role('button',name='Guild',exact=True).click();await page.get_by_role('button',name='Fit guild',exact=True).click();await page.wait_for_timeout(500);assert not await page.evaluate('document.documentElement.scrollWidth>innerWidth');await page.screenshot(path=f'/tmp/maestro-multi-layout-{w}.png')
  assert not errors,errors;report['checks'].append('desktop, laptop and mobile layouts without horizontal overflow or page errors')
  await browser.close()
 Path('/tmp/maestro-multi-live-evidence.json').write_text(json.dumps(report,indent=2));print(json.dumps(report),flush=True)
try:asyncio.run(main())
finally:
 for folder in folders:
  subprocess.run([os.environ.get('MAESTRO_BINARY',str(Path.home()/'.local/bin/maestro')),'--cwd',str(folder),'shutdown'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=15)
 server.terminate();server.wait(timeout=10)
