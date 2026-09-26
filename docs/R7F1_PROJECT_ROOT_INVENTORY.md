# R7F.1 Project root inventory
Adds bounded local-tree inventory. It detects nested PCC/build authorities without silently changing the registered project root. If a nested candidate exists, Build Doctor diagnoses it and reports `nestedRootDetected`; registration repair remains an explicit later operation.
