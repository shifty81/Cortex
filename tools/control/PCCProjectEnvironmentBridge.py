#!/usr/bin/env python3
"""Compatibility wrapper: inject portable project environment before existing PCC broker."""
from __future__ import annotations
import os
from pathlib import Path
from typing import Mapping
from PCCPortableProjectEnvironment import portable_project_environment
def apply_portable_project_environment(root:Path, env:Mapping[str,str]|None=None):
 return portable_project_environment(root,env)
