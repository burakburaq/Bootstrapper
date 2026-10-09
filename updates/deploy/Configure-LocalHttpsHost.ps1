[CmdletBinding()]
param(
    [string]$VMName = 'baykuslig',
    [ValidateSet('172.17.128.1')][string]$HostAddress = '172.17.128.1',
    [ValidateSet(8443)][int]$Port = 8443,
    [string]$ResultPath
)
$ErrorActionPreference = 'Stop'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
if (-not ([Security.Principal.WindowsPrincipal]$identity).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Host ağ hazırlığı yönetici PowerShell gerektirir.'
}
$vm = Get-VM -Name $VMName
if ($vm.State -ne 'Running') { throw "VM çalışmıyor: $VMName" }
$adapters = @(Get-VMNetworkAdapter -VMName $VMName)
$addresses = @(Get-NetIPAddress -AddressFamily IPv4 -IPAddress $HostAddress)
if ($addresses.Count -ne 1 -or $addresses[0].PrefixLength -ne 20) { throw 'Beklenen Hyper-V host adresi/subnet bulunamadı: 172.17.128.1/20.' }
$address = $addresses[0]
$matching = @($adapters | Where-Object { $address.InterfaceAlias -eq "vEthernet ($($_.SwitchName))" })
if ($matching.Count -ne 1) { throw 'VM switch adresi ile Docker HTTPS bind adresi uyuşmuyor; rastgele ağ erişimi açılmadı.' }
# IPNetwork is unavailable in Windows PowerShell 5.1; normalize the network
# explicitly so firewall scope never contains host bits (172.17.128.1/20).
$octets = ([Net.IPAddress]::Parse($HostAddress)).GetAddressBytes()
$remaining = [int]$address.PrefixLength
$networkOctets = foreach ($octet in $octets) {
    $bits = [Math]::Min(8, [Math]::Max(0, $remaining))
    $mask = if ($bits -eq 0) { 0 } else { 256 - [Math]::Pow(2, 8 - $bits) }
    $octet -band [int]$mask
    $remaining -= 8
}
$network = ($networkOctets -join '.') + '/' + $address.PrefixLength
if ($network -ne '172.17.128.0/20') { throw "Beklenmeyen Hyper-V subnet: $network" }
$name = 'BaykusLig-LocalUpdate-HTTPS'
$rule = Get-NetFirewallRule -Name $name -ErrorAction SilentlyContinue
if ($rule) {
    $rule | Set-NetFirewallRule -Enabled True -Direction Inbound -Action Allow -Profile Any -InterfaceAlias $address.InterfaceAlias | Out-Null
    $rule | Get-NetFirewallPortFilter | Set-NetFirewallPortFilter -Protocol TCP -LocalPort $Port -RemotePort Any | Out-Null
    $rule | Get-NetFirewallAddressFilter | Set-NetFirewallAddressFilter -LocalAddress $HostAddress -RemoteAddress $network | Out-Null
} else {
    New-NetFirewallRule -Name $name -DisplayName 'BaykusLig local update HTTPS' -Enabled True -Direction Inbound -Action Allow -Protocol TCP -LocalAddress $HostAddress -LocalPort $Port -RemoteAddress $network -Profile Any -InterfaceAlias $address.InterfaceAlias | Out-Null
}
$configuredRule = Get-NetFirewallRule -Name $name
$portFilter = $configuredRule | Get-NetFirewallPortFilter
$addressFilter = $configuredRule | Get-NetFirewallAddressFilter
$interfaceFilter = $configuredRule | Get-NetFirewallInterfaceFilter
if ($configuredRule.Enabled -ne 'True' -or $configuredRule.Direction -ne 'Inbound' -or $configuredRule.Action -ne 'Allow' -or
    $portFilter.Protocol -ne 'TCP' -or "$($portFilter.LocalPort)" -ne "$Port" -or
    "$($addressFilter.LocalAddress)" -ne $HostAddress -or
    "$($addressFilter.RemoteAddress)" -notin @($network, '172.17.128.0/255.255.240.0') -or
    "$($interfaceFilter.InterfaceAlias)" -ne $address.InterfaceAlias) {
    throw 'Firewall kuralının doğrulanan kapsamı beklenen yerel HTTPS ağıyla uyuşmuyor.'
}
$result = [ordered]@{ vm = $vm.Name; state = "$($vm.State)"; switch = $matching[0].SwitchName; host_address = $HostAddress; subnet = $network; port = $Port; guest_addresses = @($matching[0].IPAddresses); firewall = $name; firewall_verified = $true }
if ($ResultPath) { $result | ConvertTo-Json | Set-Content -LiteralPath $ResultPath -Encoding utf8 }
$result | ConvertTo-Json
