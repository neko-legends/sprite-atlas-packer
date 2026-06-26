$ErrorActionPreference = 'Stop'

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$rootDir = Split-Path -Parent $scriptDir
$targetDir = Join-Path $rootDir 'src-tauri\target-portable'
$releaseDir = Join-Path $targetDir 'release'
$portablePackageDir = Join-Path $rootDir 'release\portable'
$cargoExe = (Get-Command cargo -ErrorAction SilentlyContinue).Source

if (-not $cargoExe) {
  $cargoExe = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
}

if (-not (Test-Path -LiteralPath $cargoExe)) {
  throw 'Cargo was not found. Install Rust or add cargo.exe to PATH.'
}

$resolvedRootDir = [System.IO.Path]::GetFullPath($rootDir)
$resolvedTargetDir = [System.IO.Path]::GetFullPath($targetDir)
$rootPrefix = $resolvedRootDir.TrimEnd('\') + '\'
if (-not $resolvedTargetDir.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
  throw "Refusing to clean portable target outside repository: $resolvedTargetDir"
}

if (Test-Path -LiteralPath $targetDir) {
  Write-Host "Cleaning portable build target: $targetDir"
  Remove-Item -LiteralPath $targetDir -Recurse -Force
}

Push-Location $rootDir
try {
  $previousCargoTargetDir = $env:CARGO_TARGET_DIR
  $previousPath = $env:PATH
  $env:CARGO_TARGET_DIR = $targetDir
  $env:PATH = "$(Split-Path -Parent $cargoExe);$env:PATH"

  try {
    npx tauri build --no-bundle --features desktop --runner $cargoExe
  } finally {
    $env:PATH = $previousPath
    if ($null -eq $previousCargoTargetDir) {
      Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
    } else {
      $env:CARGO_TARGET_DIR = $previousCargoTargetDir
    }
  }

  $sourceExe = Join-Path $releaseDir 'sprite-atlas-packer.exe'
  $staleTargetPortableExe = Join-Path $releaseDir 'sprite-atlas-packer-portable.exe'
  $stalePortableExe = Join-Path $portablePackageDir 'sprite-atlas-packer.exe'
  $portableExe = Join-Path $portablePackageDir 'sprite-atlas-packer-portable.exe'

  if (-not (Test-Path -LiteralPath $sourceExe)) {
    throw "Portable build failed because $sourceExe was not created."
  }

  if (Test-Path -LiteralPath $staleTargetPortableExe) {
    Remove-Item -LiteralPath $staleTargetPortableExe -Force
  }

  New-Item -ItemType Directory -Force -Path $portablePackageDir | Out-Null
  if (Test-Path -LiteralPath $stalePortableExe) {
    Remove-Item -LiteralPath $stalePortableExe -Force
  }
  if (Test-Path -LiteralPath $portableExe) {
    Remove-Item -LiteralPath $portableExe -Force
  }
  Move-Item -LiteralPath $sourceExe -Destination $portableExe -Force
  Set-Content -LiteralPath (Join-Path $portablePackageDir 'README.txt') -Value @(
    'Sprite Atlas Packer Portable',
    '',
    'Run sprite-atlas-packer-portable.exe to launch the portable app.'
  )
  Write-Host "Portable app created: $portableExe"
  Write-Host "Portable package created: $portablePackageDir"
} finally {
  Pop-Location
}
