from __future__ import annotations
from pathlib import Path
from typing import Mapping
from PCCPortableProjectEnvironment import portable_project_environment
def merge_project_environment(root:Path, env:Mapping[str,str]):
 out,report=portable_project_environment(root,env)
 return out,report
