#Requires -Version 7.0
param(
    [Parameter(Mandatory)][string]$ProductFile,
    [Parameter(Mandatory)][string]$ConfigFile,
    [ValidateSet('validate','prepare','verify','upload','draft','publish')][string]$Action = 'validate',
    [string]$Version, [Nullable[UInt64]]$Sequence,
    [string]$Package, [string]$Output, [string]$Base, [string]$Notes,
    [string]$CaCertificate
)
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
$taskArgs = @('run','--manifest-path',(Join-Path $taskRoot 'updates/Cargo.toml'),'--locked','-p','release-cli','--')
if ($CaCertificate) { $taskArgs += @('--ca-certificate',(Resolve-Path -LiteralPath $CaCertificate).Path) }
$taskArgs += @('project','--product',(Resolve-Path -LiteralPath $ProductFile).Path,'--config',(Resolve-Path -LiteralPath $ConfigFile).Path)
if ($Action -ne 'validate' -and !$Version) { throw 'Version is required for release operations' }
if ($Version) { $taskArgs += @('--version',$Version) }
if ($PSBoundParameters.ContainsKey('Sequence')) { $taskArgs += @('--sequence',$Sequence.ToString()) }
if ($Package) { $taskArgs += @('--package',[IO.Path]::GetFullPath($Package)) }
if ($Output) { $taskArgs += @('--output',[IO.Path]::GetFullPath($Output)) }
if ($Base) { $taskArgs += @('--base',[IO.Path]::GetFullPath($Base)) }
if ($Notes) { $taskArgs += @('--notes',$Notes) }
$taskArgs += $Action
cargo @taskArgs
if ($LASTEXITCODE -ne 0) { throw "Release stage '$Action' failed (exit $LASTEXITCODE)" }
