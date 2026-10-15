param()
$ErrorActionPreference = 'Stop'
$desktopDirectory = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$version = '0.46.0'
$expectedHash = '21d829653ac1f178b124ed384e2a57df19b99ae404935cba9a1a5719c199b0bf'
$buildDirectory = Join-Path $desktopDirectory 'build-file-server'
$destination = Join-Path $desktopDirectory 'src-tauri\file-server'
New-Item -ItemType Directory -Path $buildDirectory,$destination -Force | Out-Null
$archive = Join-Path $buildDirectory "dufs-v$version-x86_64-pc-windows-msvc.zip"
if (-not (Test-Path -LiteralPath $archive)) {
    Invoke-WebRequest "https://github.com/sigoden/dufs/releases/download/v$version/dufs-v$version-x86_64-pc-windows-msvc.zip" -OutFile $archive
}
if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expectedHash) { throw 'Dufs archive SHA-256 verification failed' }
Expand-Archive -LiteralPath $archive -DestinationPath $buildDirectory -Force
$executable = Join-Path $buildDirectory 'dufs.exe'
if (-not (Test-Path -LiteralPath $executable)) { throw 'Dufs executable is missing from the official archive' }
Copy-Item -LiteralPath $executable -Destination (Join-Path $destination 'dufs.exe') -Force
Copy-Item -LiteralPath (Join-Path $PSScriptRoot '..\third-party\dufs-LICENSE-MIT.txt') -Destination (Join-Path $destination 'LICENSE-MIT.txt') -Force
Write-Host "Dufs $version prepared; official archive SHA-256 verified." -ForegroundColor Green
