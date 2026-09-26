"""Compatibility hook for PCC embedded process hosting.

Current Cortex GUI policy is strict: routine embedded work must not allocate a visible
Windows console. Process creation is governed by PCCSurfaceCommon/ProcessHost using
CREATE_NO_WINDOW plus captured pipes. This module intentionally performs no global
monkey-patching; it remains so rolling/manual overwrites with older PYTHONPATH layouts
do not fail import-time startup.
"""
