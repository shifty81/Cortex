#!/usr/bin/env python3
from __future__ import annotations
import argparse, os, shutil, subprocess
from pathlib import Path

VERSION='PCC-RUNTIME-STATE-REPAIR-0.2'
PLACEHOLDERS=(
 'logs/arbiter_engine/.gitkeep','logs/host_app/.gitkeep','logs/python_bridge/.gitkeep',
 'logs/self_build/.gitkeep','logs/steam_server_admin/.gitkeep','logs/vs_extension/.gitkeep',
)

def run(root:Path,*args:str)->subprocess.CompletedProcess[bytes]:
    kw={}
    if os.name=='nt': kw['creationflags']=getattr(subprocess,'CREATE_NO_WINDOW',0)
    return subprocess.run(['git','-C',str(root),*args],stdout=subprocess.PIPE,stderr=subprocess.PIPE,check=False,**kw)

def restore_placeholders(root:Path)->list[str]:
    restored=[]
    for rel in PLACEHOLDERS:
        p=root/rel
        if p.exists(): continue
        cp=run(root,'show',f'HEAD:{rel}')
        if cp.returncode!=0: continue
        p.parent.mkdir(parents=True,exist_ok=True); p.write_bytes(cp.stdout); restored.append(rel)
    return restored

def migrate_legacy_events(root:Path)->tuple[int,str]:
    src=root/'logs'/'events'; dst=root/'.cortex'/'observability'/'events'
    if not src.exists(): return 0,str(dst)
    dst.mkdir(parents=True,exist_ok=True); moved=0
    for p in list(src.iterdir()):
        target=dst/p.name
        if target.exists(): target=dst/(p.stem+'-legacy'+p.suffix)
        shutil.move(str(p),str(target)); moved+=1
    try: src.rmdir()
    except OSError: pass
    return moved,str(dst)

def main()->int:
    ap=argparse.ArgumentParser(); ap.add_argument('--root',required=True); ns=ap.parse_args(); root=Path(ns.root).resolve()
    restored=restore_placeholders(root); moved,dst=migrate_legacy_events(root)
    print(f'[PASS] Runtime-state repair {VERSION}: restored {len(restored)} tracked placeholder(s); migrated {moved} legacy event item(s).')
    for rel in restored: print('  RESTORE '+rel)
    if moved: print('  EVENTS -> '+dst)
    dirty=run(root,'status','--short').stdout.decode('utf-8',errors='replace').strip()
    if dirty:
        print('[WARN] Git worktree remains modified after runtime-state normalization:')
        print(dirty)
        return 2
    print('[PASS] Git worktree CLEAN after runtime-state normalization.')
    return 0
if __name__=='__main__': raise SystemExit(main())
