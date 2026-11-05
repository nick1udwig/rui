#!/usr/bin/env python3
"""Serve the browser/phone replay runner and a selected local evidence directory."""
import argparse
from http.server import SimpleHTTPRequestHandler,ThreadingHTTPServer
from pathlib import Path
import urllib.parse
ROOT=Path(__file__).resolve().parents[2]
class Handler(SimpleHTTPRequestHandler):
    def translate_path(self,path):
        path=urllib.parse.unquote(urllib.parse.urlsplit(path).path)
        base=self.server.data if path.startswith('/data/') else ROOT
        relative=path[6:] if path.startswith('/data/') else path.lstrip('/')
        if base==self.server.data and (len(Path(relative).parts)!=2 or Path(relative).name!='publications.packets'):
            return str(base/'not-found')
        if not relative:relative='demos/structured/phone.html'
        target=(base/relative).resolve()
        if not target.is_relative_to(base):return str(base/'not-found')
        if base==ROOT and not (relative.startswith('client-browser/dist/') or relative.startswith('client-browser/wasm/') or relative in ('demos/structured/phone.html','demos/structured/state.mjs')):return str(base/'not-found')
        return str(target)
    def list_directory(self,path):self.send_error(404);return None

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--data',type=Path,required=True);p.add_argument('--host',default='127.0.0.1');p.add_argument('--port',type=int,default=8099);args=p.parse_args()
    server=ThreadingHTTPServer((args.host,args.port),Handler);server.data=args.data.resolve()
    print(f'Open http://{args.host}:{server.server_port}/',flush=True);server.serve_forever()
if __name__=='__main__':main()
