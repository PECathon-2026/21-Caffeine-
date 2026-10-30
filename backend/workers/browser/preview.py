"""Own the entire preview process group; close stdin to stop it."""
import json, os, selectors, signal, subprocess, sys, time
q=json.loads(sys.stdin.readline())
p=subprocess.Popen(q['argv'],cwd=q['cwd'],env=q['env'],stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,start_new_session=True)
print(json.dumps({'pid':p.pid}),flush=True)
def stop(*_):
    try:os.killpg(p.pid,signal.SIGTERM)
    except ProcessLookupError:pass
    time.sleep(.3)
    try:os.killpg(p.pid,signal.SIGKILL)
    except ProcessLookupError:pass
    p.wait();sys.exit(0)
signal.signal(signal.SIGTERM,stop);signal.signal(signal.SIGINT,stop)
s=selectors.DefaultSelector();s.register(sys.stdin,selectors.EVENT_READ);s.register(p.stdout,selectors.EVENT_READ)
try:
    while p.poll() is None:
        for key,_ in s.select(1):
            if key.fileobj is sys.stdin:stop()
            data=os.read(p.stdout.fileno(),4096)
            if data:sys.stdout.buffer.write(data);sys.stdout.buffer.flush()
    print(f'\nPreview exited with status {p.returncode}',flush=True)
finally:stop()
