<#
.SYNOPSIS
  Run one cargo command in the driver workspace through a short filesystem root.

.DESCRIPTION
  The driver's encoder deps (pf-encode-win's `pyrowave` and `qsv` features) build PyroWave and
  oneVPL from source with CMake, and CMake probes the toolchain by building a tiny MSBuild
  project. MSBuild writes .tlog paths under the CMake tree and cannot exceed MAX_PATH, so a deep
  checkout fails before anything compiles - the CI runner's root is ~78 characters, which is
  already most of the budget once the target dir is added.

  The target dir cannot move: wdk-build's find_top_level_cargo_manifest() walks UP from OUT_DIR
  for the first ancestor with a Cargo.lock and does not support a relocated CARGO_TARGET_DIR.
  So shorten the ROOT instead - subst a drive letter onto the repo and run cargo through it.

  It must be the repo root, not the driver workspace: the workspace's path deps (pf-encode-win,
  pf-frame) live in crates/ and resolve UPWARD, which a drive mapped at the workspace has no
  parent for. Falls back to running in place when no drive letter is free.
#>
# $args, not a param() block: a [Parameter()] attribute makes this an advanced function, and the
# binder then matches `-p` against its common parameters and fails as ambiguous with
# -ProgressAction / -PipelineVariable. Cargo flags must reach cargo untouched.
$CargoArgs = $args

$ErrorActionPreference = 'Continue'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$drivers = Join-Path $repoRoot 'packaging\windows\drivers'
$rel = $drivers.Substring($repoRoot.Length).TrimStart('\')

# wdk-build needs the workspace's own target dir; a shared CARGO_TARGET_DIR hides the lock.
$prevTarget = $env:CARGO_TARGET_DIR
Remove-Item Env:\CARGO_TARGET_DIR -ErrorAction SilentlyContinue

$used = (Get-PSDrive -PSProvider FileSystem).Name
$letter = 'X', 'Y', 'W', 'V', 'U', 'T' | Where-Object { $used -notcontains $_ } | Select-Object -First 1
$subst = $null
if ($letter) {
    & subst "${letter}:" $repoRoot 2>&1 | Out-Null
    if ($LASTEXITCODE -eq 0 -and (Test-Path "${letter}:\$rel")) { $subst = "${letter}:" }
    elseif ($LASTEXITCODE -eq 0) { & subst "${letter}:" /D 2>&1 | Out-Null }
}
$runDir = if ($subst) { "$subst\$rel" } else { $drivers }
if (-not $subst) { Write-Host '    (no free drive letter - running in place; a deep checkout may hit MAX_PATH)' }

Write-Host "==> cargo $($CargoArgs -join ' ')  [in $runDir]"
Push-Location $runDir
& cargo @CargoArgs
$rc = $LASTEXITCODE
Pop-Location

if ($subst) { & subst $subst /D 2>&1 | Out-Null }
if ($prevTarget) { $env:CARGO_TARGET_DIR = $prevTarget }
exit $rc
