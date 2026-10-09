[CmdletBinding()]
param(
    [ValidateSet('172.17.128.1')][string]$HostAddress = '172.17.128.1',
    [ValidateSet(8443)][int]$Port = 8443,
    [string]$RootCertificate = (Join-Path $PSScriptRoot 'root.crt')
)
$ErrorActionPreference = 'Stop'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
if (-not ([Security.Principal.WindowsPrincipal]$identity).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'VM sertifika/hosts hazırlığı yönetici PowerShell gerektirir.'
}
$hostname = 'updates.baykuslig.test'
$parsed = [Net.IPAddress]::Parse($HostAddress)
if ($parsed.AddressFamily -ne [Net.Sockets.AddressFamily]::InterNetwork -or [Net.IPAddress]::IsLoopback($parsed)) { throw 'Hyper-V host IPv4 adresi gerekli.' }
$certificate = [Security.Cryptography.X509Certificates.X509Certificate2]::new((Resolve-Path -LiteralPath $RootCertificate).Path)
if ($certificate.HasPrivateKey) { throw 'Yalnızca genel root.crt sertifikası kullanılmalı.' }
$constraints = @($certificate.Extensions | Where-Object { $_.Oid.Value -eq '2.5.29.19' })
if ($constraints.Count -ne 1 -or -not $constraints[0].CertificateAuthority -or $certificate.Subject -ne $certificate.Issuer) { throw 'Geçerli bir kök CA sertifikası gerekli.' }
if ((Get-Date) -lt $certificate.NotBefore -or (Get-Date) -gt $certificate.NotAfter) { throw 'Kök sertifikanın geçerlilik süresi dışında.' }
$hosts = Join-Path $env:WINDIR 'System32/drivers/etc/hosts'
$lines = @(Get-Content -LiteralPath $hosts)
$managed = '# BaykusLig local update HTTPS'
$other = @($lines | Where-Object { $_ -notlike "*$managed*" -and $_ -notmatch '^\s*#' -and ($_ -split '#')[0] -match "(^|\s)$([regex]::Escape($hostname))(\s|$)" })
if ($other.Count) { throw 'Hosts dosyasında bu ad için başka eşleme var; önce mevcut eşlemeyi düzeltin.' }
$store = [Security.Cryptography.X509Certificates.X509Store]::new('Root', 'LocalMachine')
$store.Open('ReadWrite')
try { if (-not $store.Certificates.Find('FindByThumbprint', $certificate.Thumbprint, $false).Count) { $store.Add($certificate) } }
finally { $store.Close() }
$lines = @($lines | Where-Object { $_ -notlike "*$managed*" })
$lines += "$HostAddress $hostname $managed"
[IO.File]::WriteAllLines($hosts, $lines, [Text.Encoding]::ASCII)
Clear-DnsClientCache
$origin = "https://${hostname}:$Port"
# Windows PowerShell 5.1 can otherwise use an obsolete TLS default.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
$live = Invoke-RestMethod "$origin/health/live" -TimeoutSec 20
$ready = Invoke-RestMethod "$origin/health/ready" -TimeoutSec 20
[ordered]@{ origin = $origin; certificate_thumbprint = $certificate.Thumbprint; server = $live; readiness = $ready } | ConvertTo-Json -Depth 5
