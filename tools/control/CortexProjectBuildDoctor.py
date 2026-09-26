#!/usr/bin/env python3
from __future__ import annotations
import argparse,json,os,re,shutil,subprocess
from pathlib import Path
from PCCPortableProjectEnvironment import portable_project_environment
from PCCProjectDiscovery import discover_project_contract_data
from CortexProjectInventory import inventory
VERSION="PCC-PROJECT-BUILD-DOCTOR-0.7"
def probe(exe,args,env,cwd):
 p=shutil.which(exe,path=env.get("PATH",""))
 if not p:return {"status":"MISSING","path":None,"version":None}
 try:
  cp=subprocess.run([p,*args],cwd=str(cwd),env=env,capture_output=True,text=True,timeout=10,creationflags=getattr(subprocess,"CREATE_NO_WINDOW",0))
  t=(cp.stdout or cp.stderr).strip().splitlines()
  return {"status":"READY" if cp.returncode==0 else "ERROR","path":p,"version":t[0][:200] if t else ""}
 except Exception as e:return {"status":"ERROR","path":p,"version":str(e)}
def reqs(root,data):
 req=set();e=[]
 def add(x,w):req.add(x);e.append({"tool":x,"reason":w})
 for c in data.get("commands") or []:
  if not isinstance(c,dict):continue
  b=(str(c.get("program") or "")+" "+" ".join(map(str,c.get("args") or []))).casefold()
  if "cargo" in b:add("cargo","project control command");add("rustc","Rust compiler")
  if "cmake" in b:add("cmake","project control command")
  if re.search(r"\bnpm(?:\.cmd)?\b",b):add("npm","project control command");add("node","npm runtime")
  if "python" in b:add("python","project control command")
  if "gradle" in b or "gradlew" in b:add("gradle-wrapper","project control command")
 for n,xs in [("Cargo.toml",("cargo","rustc")),("CMakeLists.txt",("cmake",)),("package.json",("node","npm")),("pyproject.toml",("python",))]:
  if (root/n).is_file():
   for x in xs:add(x,n)
 if (root/"gradlew.bat").is_file() or (root/"gradlew").is_file():add("gradle-wrapper","Gradle wrapper")
 return sorted(req),e
def main():
 a=argparse.ArgumentParser();a.add_argument("--root",required=True);n=a.parse_args();registered=Path(n.root).resolve()
 inv=inventory(registered);effective=registered;redirect=None
 if inv["rootLooksEmpty"] and inv["recommendedRoot"] not in (None,"."):
  effective=(registered/inv["recommendedRoot"]).resolve();redirect=inv["recommendedRoot"]
 env,rep=portable_project_environment(registered);data=discover_project_contract_data(effective);rq,ev=reqs(effective,data);tools={}
 specs={"cargo":["--version"],"rustc":["--version"],"cmake":["--version"],"node":["--version"],"npm":["--version"],"python":["--version"]}
 for x in rq:
  if x=="gradle-wrapper":
   g=effective/("gradlew.bat" if os.name=="nt" else "gradlew");tools[x]={"status":"READY" if g.is_file() else "MISSING","path":str(g) if g.is_file() else None}
  else:tools[x]=probe(x,specs[x],env,effective)
 blockers=[k for k,v in tools.items() if v["status"]!="READY"]
 if not rq:blockers=["NO_BUILD_REQUIREMENTS_DETECTED"];status="INCOMPLETE";ready=False
 elif blockers:status="BLOCKED";ready=False
 else:status="READY";ready=True
 d=data.get("_pccDiscovery") or {}
 rep.update({"doctorVersion":VERSION,"registeredRoot":str(registered),"effectiveDiagnosticRoot":str(effective),
 "nestedRootDetected":redirect,"inventory":inv,"contract":{"source":d.get("source"),"authority":d.get("authority"),"provider":d.get("provider"),
 "commandKeys":[c.get("key") for c in data.get("commands",[]) if isinstance(c,dict)]},"requirements":rq,"requirementEvidence":ev,
 "tools":tools,"status":status,"blockers":blockers,"ready":ready})
 print(json.dumps(rep,indent=2));return 0 if ready else 2
if __name__=="__main__":raise SystemExit(main())
