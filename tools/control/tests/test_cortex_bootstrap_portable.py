from __future__ import annotations

import json
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
BOOTSTRAP = ROOT / "scripts" / "Bootstrap-CortexEnvironment.ps1"
POLICY = ROOT / "config" / "cortex" / "portable_drive_root_vault.v1.json"
BUILD_TOOLS = ROOT / "HYDRATE_CORTEX_BUILD_TOOLS.cmd"


class PortableBootstrapTests(unittest.TestCase):
    def test_portable_source_policy_uses_repository_drive_root(self) -> None:
        data = json.loads(POLICY.read_text(encoding="utf-8"))
        self.assertTrue(data["enabled"])
        self.assertEqual(data["mode"], "repository_drive_root")
        text = BOOTSTRAP.read_text(encoding="utf-8")
        self.assertIn("Test-PortableDriveRootPolicy", text)
        self.assertIn("return [System.IO.Path]::GetFullPath($repoDrive)", text)

    def test_rustup_verification_retries_and_evicts_bad_cache(self) -> None:
        text = BOOTSTRAP.read_text(encoding="utf-8")
        self.assertIn("Ensure-VerifiedSha256Download", text)
        self.assertIn("$rustupUrl + '.sha256'", text)
        self.assertIn("-Attempts 3", text)
        self.assertIn("deleting stale file and downloading again", text)
        self.assertIn("Remove-Item -LiteralPath $Destination -Force", text)

    def test_optional_rust_failure_does_not_become_python_launch_failure(self) -> None:
        text = BOOTSTRAP.read_text(encoding="utf-8")
        self.assertIn("preserving Python/Git environment so PCC can still launch", text)
        self.assertIn("if (-not $PythonOk)", text)
        self.assertIn("exit 11", text)
        self.assertIn("PCC launch is available, but one or more build/certification dependencies remain unavailable", text)

    def test_windows_store_python_alias_is_not_treated_as_real_python(self) -> None:
        text = BOOTSTRAP.read_text(encoding="utf-8")
        self.assertIn("Microsoft\\\\WindowsApps", text)
        self.assertIn("*> $null", text)

    def test_rust_sha_parser_decodes_binary_content(self) -> None:
        text = BOOTSTRAP.read_text(encoding="utf-8")
        self.assertIn("$response.Content -is [byte[]]", text)
        self.assertIn("[System.Text.Encoding]::ASCII.GetString([byte[]]$response.Content)", text)
        self.assertIn("response preview", text)

    def test_portable_python_migrates_full_tk_runtime_when_installer_noops(self) -> None:
        text = BOOTSTRAP.read_text(encoding="utf-8")
        self.assertIn("Get-PythonMigrationCandidates", text)
        self.assertIn("Copy-CortexPythonRuntime", text)
        self.assertIn("Include_tcltk=1", text)
        self.assertIn("Test-CortexPythonRuntime -Exe $targetExe -RequireTk", text)
        self.assertIn("old AppData Vault", text)

    def test_portable_python_does_not_use_tk_less_embeddable_runtime(self) -> None:
        text = BOOTSTRAP.read_text(encoding="utf-8")
        self.assertIn("PCC GUI requires Tkinter", text)
        self.assertNotIn("python-3.14.7-embed-amd64.zip", text)

    def test_msvc_build_tools_activation_is_persisted_into_launcher_environment(self) -> None:
        text = BOOTSTRAP.read_text(encoding="utf-8")
        self.assertIn("Test-MsvcDeveloperEnvironment", text)
        self.assertIn("VsDevCmd.bat", text)
        self.assertIn("-arch=amd64", text)
        self.assertIn("where link.exe", text)
        self.assertIn("where cl.exe", text)
        self.assertIn("CORTEX_VSDEVCMD", text)
        self.assertIn("call \"{0}\" -no_logo -arch=amd64 -host_arch=amd64", text)

    def test_build_tools_hydration_uses_vctools_workload(self) -> None:
        text = BOOTSTRAP.read_text(encoding="utf-8")
        self.assertIn("Microsoft.VisualStudio.Workload.VCTools", text)
        self.assertIn("--includeRecommended", text)
        self.assertIn("https://aka.ms/vs/17/release/vs_BuildTools.exe", text)

    def test_build_tools_launcher_requires_link_and_cl_before_success(self) -> None:
        text = BUILD_TOOLS.read_text(encoding="utf-8")
        self.assertIn("where link.exe", text)
        self.assertIn("where cl.exe", text)
        self.assertIn("MSVC amd64 developer environment active", text)

    def test_msvc_is_machine_scoped_not_installed_to_removable_vault(self) -> None:
        text = BOOTSTRAP.read_text(encoding="utf-8")
        self.assertIn("machine-scoped Visual Studio 2022 C++ Build Tools", text)
        self.assertIn("Test-IsAdministrator", text)
        self.assertIn("-Verb RunAs", text)
        self.assertNotIn("--installPath',$installPath", text)
        self.assertIn("scope = 'machine'", text)

    def test_msvc_installer_records_exit_code_and_setup_logs(self) -> None:
        text = BOOTSTRAP.read_text(encoding="utf-8")
        self.assertIn("Get-VsInstallerExitMeaning", text)
        self.assertIn("Copy-VisualStudioSetupEvidence", text)
        self.assertIn("Visual Studio Build Tools installer exit code", text)
        self.assertIn("rebootRequired = $MsvcRebootRequired", text)
        self.assertIn("Microsoft.VisualStudio.Component.VC.Tools.x86.x64", text)


    def test_shared_sccache_is_hydrated_as_optional_cross_project_accelerator(self) -> None:
        text = BOOTSTRAP.read_text(encoding="utf-8")
        self.assertIn("install sccache --locked --root", text)
        self.assertIn("CORTEX_DEPENDENCY_REUSE_POLICY=share-compatible-isolate-conflicts", text)
        self.assertIn("RUSTC_WRAPPER", text)
        self.assertIn("optional_cross_project_compiled_reuse", text)
        self.assertIn("different versions/features naturally produce different keys", text)



if __name__ == "__main__":
    unittest.main()
