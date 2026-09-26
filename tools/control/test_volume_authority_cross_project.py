#!/usr/bin/env python3
from pathlib import Path
import tempfile, json, sys
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/"control"))
from PCCVolumeAuthority import resolve_volume_context
with tempfile.TemporaryDirectory() as td:
 r=Path(td); (r/"Cortex/config/cortex").mkdir(parents=True); (r/"projects/Demo").mkdir(parents=True)
 (r/".cortex-volume.json").write_text(json.dumps({"schema":"cortex.volume.v1","volumeId":"test-volume"}))
 auth={"state":".cortex","artifacts":"Artifacts","backups":"Backups","catalogs":"catalogs","cortex":"Cortex","exports":"Exports","git":"Git","intake":"Intake","models":"Models","objects":"objects","projects":"projects","recovery":"Recovery","shared":"shared","source":"Source","tools":"tools","vault":"Vault","archive":".archive"}
 (r/"Cortex/config/cortex/volume_layout.v2.json").write_text(json.dumps({"schema":"cortex.volume_layout.v2","authorities":auth}))
 c=resolve_volume_context(r/"projects/Demo")
 assert c.mount_root==r.resolve()
 assert c.cortex_root==(r/"Cortex").resolve()
 assert c.projects_root==(r/"projects").resolve()
 assert c.artifacts_root==(r/"Artifacts").resolve()
 print("PASS: project-root volume discovery resolves Cortex-owned layout")
