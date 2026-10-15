param()
$ErrorActionPreference = 'Stop'
$repository = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$desktop = Join-Path $repository 'desktop'
$python = Join-Path $repository '.venv\Scripts\python.exe'
Push-Location $repository
try {
    if (-not (Test-Path -LiteralPath $python)) {
        & uv sync --locked --python 3.14
        if ($LASTEXITCODE -ne 0) { throw 'Failed to install the official backend dependencies' }
    }
    # UV adds the build tool in an isolated overlay; the official lockfile stays intact.
    & uv run --locked --python $python --with 'pyinstaller>=6.16,<7' python -m PyInstaller `
        --noconfirm --clean --onedir --console --name windows-mcp-backend `
        --distpath (Join-Path $desktop 'build-backend') `
        --workpath (Join-Path $desktop 'build-pyinstaller') `
        --specpath (Join-Path $desktop 'build-pyinstaller') `
        --collect-all windows_mcp --collect-all fastmcp --collect-data mcp `
        --collect-submodules mcp.server --collect-submodules mcp.types `
        --hidden-import win32timezone --copy-metadata windows-mcp `
        (Join-Path $PSScriptRoot 'backend-entry.py')
    if ($LASTEXITCODE -ne 0) { throw 'Backend packaging failed' }
    $source = Join-Path $desktop 'build-backend\windows-mcp-backend'
    $destination = Join-Path $desktop 'src-tauri\backend'
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $source 'windows-mcp-backend.exe') -Destination $destination -Force
    Copy-Item -LiteralPath (Join-Path $source '_internal') -Destination $destination -Recurse -Force
    Write-Host 'Official Windows-MCP backend prepared.' -ForegroundColor Green
} finally { Pop-Location }
