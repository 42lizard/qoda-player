"""THROWAWAY UI study. Run: python3 prototypes/library-interactions/serve.py"""
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

root = Path(__file__).resolve().parent
print('Qoda prototype: http://localhost:8765/?variant=A', flush=True)
ThreadingHTTPServer(('127.0.0.1', 8765), partial(SimpleHTTPRequestHandler, directory=str(root))).serve_forever()
