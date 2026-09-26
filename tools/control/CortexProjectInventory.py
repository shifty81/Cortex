#!/usr/bin/env python3
from __future__ import annotations
import argparse,json,os
from pathlib import Path
SKIP={".git","target","node_modules",".venv","venv","build","Build","dist",".cortex",".project_control","artifacts","logs"}
NAMES={"project.control.json","PROJECT_CONTROL_CENTER.cmd","PCC.cmd","ProjectControlCenter.cmd","ProjectControlCenter.py",
"Cargo.toml","CMakeLists.txt","package.json","pyproject.toml","gradlew","gradlew.bat","settings.gradle","settings.gradle.kts"}
def inventory(root:Path,max_depth=6):
 found=[]; roots={}
 for base,dirs,files in os.walk(root):
  b=Path(base); rel=b.relative_to(root); depth=len(rel.parts)
  dirs[:]=[d for d in dirs if d not in SKIP and depth<max_depth]
  if depth>max_depth:continue
  for n in files:
   if n in NAMES or ("control" in n.casefold() and Path(n).suffix.casefold() in {".cmd",".bat",".ps1",".py",".json"}):
    p=b/n; rr=p.relative_to(root).as_posix();found.append(rr)
    if n in {"Cargo.toml","CMakeLists.txt","package.json","pyproject.toml","gradlew","gradlew.bat","project.control.json","PROJECT_CONTROL_CENTER.cmd","PCC.cmd"}:
     roots.setdefault(rel.as_posix() if rel.parts else ".",[]).append(n)
 candidates=[]
 for rel,markers in roots.items():
  score=sum(3 if x in {"project.control.json","PROJECT_CONTROL_CENTER.cmd","PCC.cmd"} else 1 for x in markers)
  candidates.append({"relativeRoot":rel,"score":score,"markers":sorted(markers)})
 candidates.sort(key=lambda x:(-x["score"],len(Path(x["relativeRoot"]).parts),x["relativeRoot"]))
 return {"schema":"pcc.project_inventory.v1","root":str(root),"found":sorted(found),"candidateRoots":candidates,
         "rootLooksEmpty":not bool(roots.get(".")),"recommendedRoot":candidates[0]["relativeRoot"] if candidates else None}
def main():
 a=argparse.ArgumentParser();a.add_argument("--root",required=True);a.add_argument("--max-depth",type=int,default=6);n=a.parse_args()
 r=Path(n.root).resolve();print(json.dumps(inventory(r,n.max_depth),indent=2));return 0
if __name__=="__main__":raise SystemExit(main())
