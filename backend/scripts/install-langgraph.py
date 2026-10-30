#!/usr/bin/env python3
"""Install the pinned LangGraph worker; --configure enables it in user config."""
import argparse
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tomllib

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--configure',action='store_true')
p.add_argument('--skip-dependencies',action='store_true',help='Reuse an already installed, verified environment')
a=p.parse_args()
repo=pathlib.Path(__file__).resolve().parents[1]
base=pathlib.Path.home()/'.local/share/maestro'
venv=base/'langgraph-venv'
base.mkdir(parents=True,exist_ok=True)
if not (venv/'bin/python').exists(): subprocess.run([sys.executable,'-m','venv',str(venv)],check=True)
if not a.skip_dependencies:
 subprocess.run([str(venv/'bin/python'),'-m','pip','install','-r',str(repo/'workers/langgraph/requirements.lock')],check=True)
subprocess.run([str(venv/'bin/python'),'-I','-c','from langchain_openai import ChatOpenAI; from langgraph.checkpoint.sqlite import SqliteSaver'],check=True)
worker=base/'langgraph-worker.py';tmp=worker.with_suffix('.tmp')
shutil.copy2(repo/'workers/langgraph/worker.py',tmp);os.replace(tmp,worker)
command=[str(venv/'bin/python'),'-I',str(worker)]
if a.configure:
 configdir=pathlib.Path(os.environ.get('MAESTRO_CONFIG_DIR',pathlib.Path(os.environ.get('XDG_CONFIG_HOME',pathlib.Path.home()/'.config'))/'maestro'))
 config=configdir/'maestro.toml';text=config.read_text()
 match=re.search(r'(?ms)^\[runtime\]\s*\n(.*?)(?=^\[|\Z)',text)
 if not match: raise SystemExit('Existing [runtime] section required')
 body=re.sub(r'(?m)^(agentRuntime|langgraphCommand)\s*=.*\n?','',match.group(1))
 body='agentRuntime = "langgraph"\nlanggraphCommand = '+json.dumps(command)+'\n'+body
 text=text[:match.start(1)]+body+text[match.end(1):]
 parsed=tomllib.loads(text)
 assert parsed['runtime']['agentRuntime']=='langgraph'
 tmp=config.with_suffix('.toml.tmp');tmp.write_text(text);tmp.chmod(config.stat().st_mode & 0o777);os.replace(tmp,config)
 print('Enabled LangGraph in',config)
print('Worker command:',json.dumps(command))
