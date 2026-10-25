#!/usr/bin/env python3
"""Fresh bounded measurements and walkthrough capture; synthetic audio only."""
from pathlib import Path
import subprocess
import sys
HERE=Path(__file__).resolve().parent
subprocess.run([sys.executable,str(HERE/'measure.py')],check=True)
subprocess.run([sys.executable,str(HERE/'measure.py'),'--modes','alsa','pipewire','--edits','--names','early','buffered-remove','late-remove','seek','--output','edit-traces.json'],check=True)
subprocess.run([sys.executable,str(HERE/'measure.py'),'--modes','alsa','pipewire','--edits','--names','early','accepted-remove','--output','corrected-edits.json'],check=True)
subprocess.run(['node',str(HERE/'replay.cjs')],check=True)
subprocess.run([sys.executable,str(HERE/'summarize.py')],check=True)

subprocess.run([sys.executable,str(HERE/'compact.py')],check=True)
