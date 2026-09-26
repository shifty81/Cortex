#!/usr/bin/env python3
from __future__ import annotations
import argparse, fnmatch, hashlib, json, os, shutil
from datetime import datetime, timezone
from pathlib import Path

VERSION="CORTEX-REPO-CONVERGENCE-R7A-0.1"

def copy_tree_verified(src:Path,dst:Path)->int:
    count=0
    for p in src.rglob("*"):
        if not p.is_file(): continue
        rel=p.relative_to(src); q=dst/rel; q.parent.mkdir(parents=True,exist_ok=True)
        if not q.exists() or hashlib.sha256(q.read_bytes()).digest()!=hashlib.sha256(p.read_bytes()).digest():
            shutil.copy2(p,q)
        if hashlib.sha256(q.read_bytes()).digest()!=hashlib.sha256(p.read_bytes()).digest():
            raise RuntimeError(f"verification failed: {p} -> {q}")
        count+=1
    return count

def main()->int:
    ap=argparse.ArgumentParser()
    ap.add_argument("--root",required=True)
    ap.add_argument("--apply",action="store_true")
    ns=ap.parse_args()
    root=Path(ns.root).resolve()
    policy=json.loads((root/"config/cortex/repository_layout.v1.json").read_text(encoding="utf-8"))
    volume=root.parent
    state=volume/".cortex"; vault=volume/"Vault"
    stamp=datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    archive=vault/"history"/"Cortex"/"repository-convergence"/stamp
    actions=[]

    for src_rel,dst_rel in policy["runtime_state"].items():
        src=root/src_rel
        if src.exists():
            actions.append(("runtime",src,volume/dst_rel))
    mig=root/"migration"
    if mig.exists(): actions.append(("archive",mig,archive/"migration"))

    root_globs=policy["archive_root_globs"]
    for p in root.iterdir():
        if p.is_file() and any(fnmatch.fnmatch(p.name,g) for g in root_globs):
            actions.append(("archive",p,archive/"root-history"/p.name))

    print(f"{VERSION} {'APPLY' if ns.apply else 'PLAN'}")
    for kind,src,dst in actions: print(f"{kind.upper():7} {src} -> {dst}")
    if not ns.apply:
        print(f"[PASS] Plan ready: {len(actions)} action(s); no files changed.")
        return 0

    receipt={"schema":"cortex.repository_convergence.receipt.v1","version":VERSION,
             "createdUtc":datetime.now(timezone.utc).isoformat(),"actions":[]}
    for kind,src,dst in actions:
        if src.is_dir():
            n=copy_tree_verified(src,dst)
            receipt["actions"].append({"kind":kind,"source":str(src),"destination":str(dst),"files":n})
            shutil.rmtree(src)
        else:
            dst.parent.mkdir(parents=True,exist_ok=True)
            shutil.copy2(src,dst)
            if hashlib.sha256(src.read_bytes()).digest()!=hashlib.sha256(dst.read_bytes()).digest():
                raise RuntimeError(f"verification failed: {src}")
            receipt["actions"].append({"kind":kind,"source":str(src),"destination":str(dst),"files":1})
            src.unlink()

    receipt_dir=state/"receipts"/"repository-convergence"; receipt_dir.mkdir(parents=True,exist_ok=True)
    rp=receipt_dir/f"r7a-{stamp}.json"; rp.write_text(json.dumps(receipt,indent=2)+"\n",encoding="utf-8")
    print(f"[PASS] Applied {len(actions)} verified action(s). Receipt: {rp}")
    return 0
if __name__=="__main__": raise SystemExit(main())
