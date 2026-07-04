param(
  [string]$BaseUrl = "https://puppetty-org.github.io/puppetty/gui",
  [string]$InstallDir = (Join-Path $env:LOCALAPPDATA "Programs\puppetty-gui"),
  [string]$Package = "puppetty-gui-windows-x64.zip",
  [switch]$Quiet
)

$ErrorActionPreference = "Stop"

function Write-Step([string]$Message) {
  if (-not $Quiet) {
    Write-Host "puppetty-gui: $Message"
  }
}

$base = $BaseUrl.TrimEnd("/")
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("puppetty-gui-install-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tmp | Out-Null

try {
  $packagePath = Join-Path $tmp "puppetty-gui.zip"
  $shaPath = Join-Path $tmp "puppetty-gui.zip.sha256"
  $extractPath = Join-Path $tmp "payload"

  $packageUrl = "$base/latest/$Package"
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
