#!/usr/bin/env python3
from __future__ import annotations
import argparse
from pathlib import Path
from PCCVolumeAuthority import resolve_volume_context
def main():
 p=argparse.ArgumentParser(); p.add_argument("--root",required=True); p.add_argument("--create",action="store_true"); a=p.parse_args()
 c=resolve_volume_context(Path(a.root),create=a.create)
 print(f"VolumeId : {c.volume_id}"); print(f"Mount    : {c.mount_root}")
 for k,v in c.__dict__.items():
  if k not in ("volume_id","mount_root"): print(f"{k:14}: {v} {'[OK]' if v.exists() else '[MISSING]'}")
 return 0
if __name__=="__main__": raise SystemExit(main())
