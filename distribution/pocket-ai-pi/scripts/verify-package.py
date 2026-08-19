#!/usr/bin/env python3
from __future__ import annotations
import argparse,hashlib,json,os,stat,sys
from pathlib import Path

def sha256(path:Path)->str:
    h=hashlib.sha256()
    with path.open('rb') as f:
        for b in iter(lambda:f.read(1024*1024),b''): h.update(b)
    return h.hexdigest()

def safe(root:Path, rel:str)->Path:
    p=(root/rel).resolve()
    try:p.relative_to(root.resolve())
    except ValueError: raise ValueError(f"asset escapes package root: {rel}")
    return p

def main()->int:
    ap=argparse.ArgumentParser();ap.add_argument('root',nargs='?',default='.');ap.add_argument('--manifest',default='CONFIG/pocket-ai-pi.manifest.v1.json');a=ap.parse_args()
    root=Path(a.root).resolve(); mp=safe(root,a.manifest)
    if not mp.is_file(): print(f"FAIL: manifest missing: {mp}",file=sys.stderr);return 2
    try:m=json.loads(mp.read_text('utf-8'))
    except Exception as e: print(f"FAIL: manifest parse: {e}",file=sys.stderr);return 2
    if m.get('schema')!='inbharat.pocket_ai_pi.package.v1' or m.get('target')!='linux-aarch64':
        print('FAIL: wrong manifest schema/target',file=sys.stderr);return 2
    seen=set()
    for x in m.get('assets',[]):
        rel=x.get('path','')
        if rel in seen: print(f"FAIL: duplicate asset {rel}",file=sys.stderr);return 2
        seen.add(rel)
        try:p=safe(root,rel)
        except ValueError as e: print(f"FAIL: {e}",file=sys.stderr);return 2
        if not p.is_file(): print(f"FAIL: missing asset {rel}",file=sys.stderr);return 3
        if p.stat().st_size!=x.get('size_bytes'): print(f"FAIL: size mismatch {rel}",file=sys.stderr);return 3
        if sha256(p)!=x.get('sha256'): print(f"FAIL: hash mismatch {rel}",file=sys.stderr);return 3
        if x.get('executable') and not os.access(p,os.X_OK): print(f"FAIL: executable bit missing {rel}",file=sys.stderr);return 3
    print(f"PASS: verified {len(seen)} Pocket AI Pi assets")
    return 0
if __name__=='__main__': raise SystemExit(main())
