param(
  [string]$Repo = "puppetty-org/puppetty",
  [string]$InstallDir = (Join-Path $env:LOCALAPPDATA "Programs\puppetty-gui"),
  [string]$Package = "puppetty-gui-windows-x64.zip",
  # Prereleases are never installed unless requested: -Channel beta, or
  # $env:PUPPETTY_CHANNEL = "beta" for the `iwr | iex` one-liner (which
  # cannot pass parameters). -Tag / $env:PUPPETTY_TAG pins an exact
  # release (e.g. gui-v0.2.0-beta.1) and skips channel resolution.
  [string]$Channel = $(if ($env:PUPPETTY_CHANNEL) { $env:PUPPETTY_CHANNEL } else { "latest" }),
  [string]$Tag = $env:PUPPETTY_TAG,
  [switch]$Quiet
)

$ErrorActionPreference = "Stop"

if ($Channel -notin @("latest", "beta")) {
  throw "unknown channel `"$Channel`" (use latest or beta)"
}

function Write-Step([string]$Message) {
  if (-not $Quiet) {
    Write-Host "puppetty-gui: $Message"
  }
}

function Test-WebView2 {
  $keys = @(
    "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
    "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
    "HKCU:\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
  )
  foreach ($key in $keys) {
    $pv = (Get-ItemProperty -Path $key -Name pv -ErrorAction SilentlyContinue).pv
    if ($pv -and $pv -ne "0.0.0.0") {
      return $true
    }
  }
  return $false
}

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("puppetty-gui-install-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tmp | Out-Null

try {
  $installRoot = [System.IO.Path]::GetFullPath($InstallDir).TrimEnd("\")
  $running = Get-Process -Name "puppetty-gui" -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and [System.IO.Path]::GetDirectoryName($_.Path).TrimEnd("\") -ieq $installRoot }
  if ($running) {
    throw "puppetty-gui is running; close it and re-run the installer"
  }

  $packagePath = Join-Path $tmp "puppetty-gui.zip"
  $shaPath = Join-Path $tmp "puppetty-gui.zip.sha256"
  $extractPath = Join-Path $tmp "payload"

  # Resolve the release via the GitHub API: newest published gui-v* release
  # on the requested channel that actually carries this platform's package
  # (skips historical releases with other asset formats). Drafts are never
  # visible to the unauthenticated API.
  if (-not $Tag) {
    Write-Step "resolving the newest $Channel release"
    $releases = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases?per_page=30" -UseBasicParsing
    $wantPrerelease = $Channel -eq "beta"
    $release = $releases |
      Where-Object {
        $_.tag_name -like "gui-v*" -and
        $_.prerelease -eq $wantPrerelease -and
        $_.assets.name -contains $Package
      } |
      Select-Object -First 1
    if (-not $release) {
      throw "no $Channel release with $Package found in $Repo"
    }
    $Tag = $release.tag_name
  }

  $packageUrl = "https://github.com/$Repo/releases/download/$Tag/$Package"
  $shaUrl = "$packageUrl.sha256"

  Write-Step "downloading $packageUrl"
  Invoke-WebRequest -Uri $packageUrl -OutFile $packagePath -UseBasicParsing
  Invoke-WebRequest -Uri $shaUrl -OutFile $shaPath -UseBasicParsing

  $actual = (Get-FileHash $packagePath -Algorithm SHA256).Hash.ToLowerInvariant()
  $expected = ((Get-Content $shaPath -Raw) -split "\s+")[0].ToLowerInvariant()
  if ($actual -ne $expected) {
    throw "checksum mismatch for downloaded package"
  }
  Unblock-File -LiteralPath $packagePath -ErrorAction SilentlyContinue

  Write-Step "installing to $InstallDir"
  Expand-Archive -Path $packagePath -DestinationPath $extractPath -Force

  $guiExe = Join-Path $extractPath "puppetty-gui.exe"
  $engineExe = Join-Path $extractPath "puppetty-engine.exe"
  if (-not (Test-Path -LiteralPath $guiExe)) {
    throw "package is missing puppetty-gui.exe"
  }
  if (-not (Test-Path -LiteralPath $engineExe)) {
    throw "package is missing puppetty-engine.exe"
  }

  if (-not (Test-WebView2)) {
    Write-Step "installing the Microsoft Edge WebView2 runtime"
    $bootstrapper = Join-Path $tmp "MicrosoftEdgeWebview2Setup.exe"
    Invoke-WebRequest -Uri "https://go.microsoft.com/fwlink/p/?LinkId=2124703" -OutFile $bootstrapper -UseBasicParsing
    Start-Process -FilePath $bootstrapper -ArgumentList "/silent", "/install" -Wait
    if (-not (Test-WebView2)) {
      throw "the WebView2 runtime is required; install it from https://developer.microsoft.com/microsoft-edge/webview2/ and re-run the installer"
    }
  }

  if (Test-Path -LiteralPath $InstallDir) {
    $existing = Get-ChildItem -LiteralPath $InstallDir -Force
    if ($existing -and -not (Test-Path -LiteralPath (Join-Path $InstallDir "puppetty-gui.exe"))) {
      throw "$InstallDir is not empty and does not look like a previous puppetty-gui install"
    }
    $existing | Remove-Item -Recurse -Force
  }
  New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
  Copy-Item -Path (Join-Path $extractPath "*") -Destination $InstallDir -Recurse -Force
  Get-ChildItem -LiteralPath $InstallDir -Recurse -File | Unblock-File -ErrorAction SilentlyContinue

  $installedGui = Join-Path $InstallDir "puppetty-gui.exe"
  $startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
  $shortcutPath = Join-Path $startMenu "puppetty-gui.lnk"
  $shell = New-Object -ComObject WScript.Shell
  $shortcut = $shell.CreateShortcut($shortcutPath)
  $shortcut.TargetPath = $installedGui
  $shortcut.WorkingDirectory = $InstallDir
  $shortcut.IconLocation = $installedGui
  $shortcut.Save()

  $uninstallPath = Join-Path $InstallDir "uninstall.ps1"
  @'
$ErrorActionPreference = "Stop"
$installDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$shortcutPath = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\puppetty-gui.lnk"
$uninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\puppetty-gui"

Remove-Item -LiteralPath $shortcutPath -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $uninstallKey -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $installDir -Recurse -Force -ErrorAction SilentlyContinue
'@ | Set-Content -Path $uninstallPath -Encoding utf8

  $uninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\puppetty-gui"
  New-Item -Path $uninstallKey -Force | Out-Null
  New-ItemProperty -Path $uninstallKey -Name "DisplayName" -Value "puppetty-gui" -PropertyType String -Force | Out-Null
  New-ItemProperty -Path $uninstallKey -Name "Publisher" -Value "puppetty" -PropertyType String -Force | Out-Null
  New-ItemProperty -Path $uninstallKey -Name "DisplayIcon" -Value $installedGui -PropertyType String -Force | Out-Null
  New-ItemProperty -Path $uninstallKey -Name "InstallLocation" -Value $InstallDir -PropertyType String -Force | Out-Null
  $uninstallCommand = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$uninstallPath`""
  New-ItemProperty -Path $uninstallKey -Name "UninstallString" -Value $uninstallCommand -PropertyType String -Force | Out-Null
  New-ItemProperty -Path $uninstallKey -Name "NoModify" -Value 1 -PropertyType DWord -Force | Out-Null
  New-ItemProperty -Path $uninstallKey -Name "NoRepair" -Value 1 -PropertyType DWord -Force | Out-Null

  Write-Step "installed"
} finally {
  Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
