#!/usr/bin/env python3
"""Install the browser dependency in Maestro's existing isolated worker venv."""
from pathlib import Path
import subprocess,shutil
python=Path.home()/'.local/share/maestro/langgraph-venv/bin/python'
if not python.exists():raise SystemExit('Run scripts/install-langgraph.py first')
subprocess.run([str(python),'-m','pip','install','playwright==1.58.0'],check=True)
if not shutil.which('chromium'):
    subprocess.run([str(python),'-m','playwright','install','chromium'],check=True)
print('Browser inspection ready')
