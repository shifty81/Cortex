#!/usr/bin/env python3
from __future__ import annotations
import argparse,json
from pathlib import Path
def main()->int:
    ap=argparse.ArgumentParser(); ap.add_argument("--root",required=True); ns=ap.parse_args()
    root=Path(ns.root).resolve()
    p=json.loads((root/"config/cortex/repository_layout.v1.json").read_text(encoding="utf-8"))
    allowed_files=set(p["root_files"])|set(p.get("keep_review_root_files",[]))
    allowed_dirs=set(p["root_directories"])
    bad=[]
    for x in root.iterdir():
        if x.name==".git": continue
        if x.is_file() and x.name not in allowed_files: bad.append(x.name)
        elif x.is_dir() and x.name not in allowed_dirs: bad.append(x.name+"/")
    runtime=[x for x in p["runtime_state"] if (root/x).exists()]
    if bad or runtime:
        print("[FAIL] Repository layout not converged.")
        for x in bad: print("  ROOT "+x)
        for x in runtime: print("  RUNTIME-IN-REPO "+x)
        return 2
    print("[PASS] Repository root conforms to cortex.repository_layout.v1")
    return 0
if __name__=="__main__": raise SystemExit(main())
