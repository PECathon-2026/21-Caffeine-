"""Fresh local-only browser inspection; no inherited browser profile."""
import base64, json, shutil, sys
from urllib.parse import urlsplit
from playwright.sync_api import sync_playwright, TimeoutError as PlaywrightTimeout

def local(url):
    u=urlsplit(url)
    return u.scheme in ('http','https') and u.hostname in ('127.0.0.1','localhost','::1') and not u.username and not u.password

def dev_socket(url, origin):
    u=urlsplit(url)
    expected='wss' if origin.scheme=='https' else 'ws'
    return u.scheme==expected and u.hostname==origin.hostname and u.port==origin.port and not u.username and not u.password and u.path in ('/_next/hmr','/_next/webpack-hmr')

def hmr_subscription(message):
    if not isinstance(message,str) or len(message)>4096: return False
    try: value=json.loads(message)
    except (ValueError,TypeError): return False
    if not isinstance(value,dict): return False
    path=value.get('path')
    return value.get('type') in ('turbopack-subscribe','turbopack-unsubscribe') and isinstance(path,str) and path.startswith('static/chunks/') and '..' not in path and '\\' not in path and set(value).issubset({'type','path','headers'}) and value.get('headers') is None

def main():
    q=json.loads(sys.stdin.readline());url=q['url']
    if not local(url): raise ValueError('Only localhost preview URLs are supported')
    width=max(320,min(2560,int(q.get('width') or 1280)));height=max(240,min(1600,int(q.get('height') or 800)))
    errors=[];blocked=[];dev_connections=[]
    with sync_playwright() as p:
        browser=p.chromium.launch(executable_path=shutil.which('chromium'),headless=True,chromium_sandbox=True)
        context=browser.new_context(viewport={'width':width,'height':height},service_workers='block',accept_downloads=False)
        origin=urlsplit(url)
        def route(r):
            u=urlsplit(r.request.url)
            if local(r.request.url) and (u.scheme,u.hostname,u.port)==(origin.scheme,origin.hostname,origin.port) and r.request.method in ('GET','HEAD','OPTIONS'):
                r.continue_()
            else:
                if len(blocked)<30: blocked.append({'url':r.request.url[:500],'method':r.request.method})
                r.abort()
        context.route('**/*',route)
        def websocket(ws):
            if dev_socket(ws.url,origin):
                # Next/Turbopack waits for its dev connection before hydration.
                # Forward server updates and narrowly scoped chunk subscriptions;
                # never forward console logs or arbitrary application commands.
                server=ws.connect_to_server()
                ws.on_message(lambda message: server.send(message) if hmr_subscription(message) else None)
                dev_connections.append(ws.url[:500])
            elif len(blocked)<30:
                # Do not close: some dev clients reconnect in a tight loop.
                blocked.append({'url':ws.url[:500],'method':'WEBSOCKET'})
        context.route_web_socket('**/*',websocket)
        page=context.new_page()
        page.set_default_timeout(5000)
        page.on('pageerror',lambda e:errors.append(str(e)[:2000]) if len(errors)<30 else None)
        page.on('console',lambda m:errors.append(m.text[:2000]) if m.type=='error' and len(errors)<30 else None)
        response=page.goto(url,wait_until='domcontentloaded',timeout=30000)
        try: page.wait_for_load_state('load',timeout=15000)
        except PlaywrightTimeout: errors.append('Page load did not settle within 15 seconds; capture may be incomplete')
        page.wait_for_timeout(2000)
        screenshot=page.screenshot(type='jpeg',quality=85,timeout=10000)
        result={'url':page.url,'title':page.title(),'status':response.status if response else None,'text':page.locator('body').inner_text(timeout=3000)[:12000], 'errors':errors,'blockedRequests':blocked,'devConnections':dev_connections,'width':width,'height':height,'screenshot':base64.b64encode(screenshot).decode()}
        browser.close();print(json.dumps(result))
if __name__=='__main__':
    try:main()
    except Exception as e: print(json.dumps({'error':f'{type(e).__name__}: {e}'}));sys.exit(1)
