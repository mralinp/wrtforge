# Download the latest wrtforge release for Windows, verify its checksum, and run it. Nothing is installed.
#   irm https://raw.githubusercontent.com/mralinp/wrtforge/main/get.ps1 | iex
#   $env:WRTFORGE_ARGS = '192.168.1.1 --dry-run'; irm https://raw.githubusercontent.com/mralinp/wrtforge/main/get.ps1 | iex
$WrtArgs = if ($env:WRTFORGE_ARGS) { @($env:WRTFORGE_ARGS -split '\s+') } else { @() }
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64' -and $env:PROCESSOR_ARCHITEW6432 -ne 'AMD64') {
    throw "wrtforge: unsupported CPU $env:PROCESSOR_ARCHITECTURE (x64 only)"
}
$t = 'x86_64-pc-windows-msvc'
$url = "https://github.com/mralinp/wrtforge/releases/latest/download/wrtforge-$t.zip"
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("wrtforge-" + [guid]::NewGuid())
New-Item -ItemType Directory $tmp | Out-Null
try {
    Invoke-WebRequest $url -OutFile "$tmp\w.zip"
    $want = ((Invoke-RestMethod "$url.sha256") -split '\s+')[0].ToLower()
    $got = (Get-FileHash "$tmp\w.zip" -Algorithm SHA256).Hash.ToLower()
    if ($want -ne $got) { throw 'wrtforge: checksum mismatch, refusing to run' }
    Expand-Archive "$tmp\w.zip" -DestinationPath $tmp
    & "$tmp\wrtforge.exe" @WrtArgs
} finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
