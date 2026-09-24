#Requires -Version 5.1
<#
.SYNOPSIS
    Bangun rilis Tidal Downloader lalu siapkan folder `packages/` yang siap diunduh orang lain.

.DESCRIPTION
    Script ini:
      1. Menjalankan `npm run tauri build` (installer NSIS + MSI + exe portable).
      2. Menyalin artefak ke `packages/windows-x64/` dengan nama file yang rapi & URL-friendly.
      3. Menulis `packages/SHA256SUMS.txt` dan `packages/README.md` (selalu sinkron dengan versi build).

.PARAMETER SkipBuild
    Lewati proses build, langsung pakai artefak terakhir di `src-tauri/target/release`.

.EXAMPLE
    npm run package:win
.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts/build-package.ps1 -SkipBuild
#>
[CmdletBinding()]
param(
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

$root     = Split-Path -Parent $PSScriptRoot
$tauriDir = Join-Path $root 'src-tauri'
$release  = Join-Path $tauriDir 'target\release'
$bundle   = Join-Path $release 'bundle'
$pkgDir   = Join-Path $root 'packages'
$outDir   = Join-Path $pkgDir 'windows-x64'

function Write-Utf8NoBom {
    param([string]$Path, [string]$Text)
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $Text, $utf8)
}

function Copy-Artifact {
    param([string]$Source, [string]$Name, [string]$Kind)
    if (-not (Test-Path -LiteralPath $Source)) {
        Write-Warning "Tidak ditemukan: $Source"
        return $null
    }
    $target = Join-Path $outDir $Name
    Copy-Item -LiteralPath $Source -Destination $target -Force
    return [pscustomobject]@{ Kind = $Kind; File = $Name; Path = $target }
}

# --- Baca identitas aplikasi dari tauri.conf.json (satu sumber kebenaran) -------
$conf        = Get-Content (Join-Path $tauriDir 'tauri.conf.json') -Raw | ConvertFrom-Json
$productName = $conf.productName
$version     = $conf.version
$slug        = ($productName -replace '[^A-Za-z0-9]+', '-').Trim('-')   # "Tidal-Downloader"

Write-Host "==> $productName v$version  ->  packages\windows-x64"

# --- 1. Build ------------------------------------------------------------------
if ($SkipBuild) {
    Write-Host '==> -SkipBuild: memakai artefak yang sudah ada di src-tauri\target\release'
} else {
    Write-Host '==> npm run tauri build (NSIS + MSI, butuh beberapa menit)'
    Push-Location $root
    try {
        & npm run tauri build
        if ($LASTEXITCODE -ne 0) { throw "tauri build gagal (exit code $LASTEXITCODE)" }
    } finally {
        Pop-Location
    }
}

if (-not (Test-Path -LiteralPath $release)) {
    throw "Folder build tidak ada: $release. Jalankan script tanpa -SkipBuild."
}

# --- 2. Bersihkan & salin artefak ---------------------------------------------
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
Get-ChildItem -Path $outDir -File | Remove-Item -Force

$artifacts = @()

# Portable: satu file exe, tanpa installer (tetap butuh WebView2 Runtime)
$artifacts += Copy-Artifact (Join-Path $release 'tauri-app.exe') `
                            "$slug-$version-portable.exe" `
                            'Portable (.exe, tanpa installer)'

# Installer NSIS
$nsis = Get-ChildItem -Path (Join-Path $bundle 'nsis') -Filter '*-setup.exe' -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($nsis) {
    $artifacts += Copy-Artifact $nsis.FullName "$slug-$version-x64-setup.exe" 'Installer NSIS (.exe)'
} else {
    Write-Warning "Installer NSIS tidak ditemukan di $bundle\nsis"
}

# Installer MSI (WiX)
$msi = Get-ChildItem -Path (Join-Path $bundle 'msi') -Filter '*.msi' -ErrorAction SilentlyContinue |
       Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($msi) {
    $artifacts += Copy-Artifact $msi.FullName "$slug-$version-x64.msi" 'Installer MSI (.msi)'
} else {
    Write-Warning "Installer MSI tidak ditemukan di $bundle\msi"
}

$artifacts = @($artifacts | Where-Object { $_ -ne $null })
if ($artifacts.Count -eq 0) {
    throw 'Tidak ada artefak yang bisa dipaketkan. Jalankan script ini tanpa -SkipBuild.'
}

# --- 3. Hash + ukuran ---------------------------------------------------------
$rows = foreach ($a in $artifacts) {
    $item = Get-Item -LiteralPath $a.Path
    [pscustomobject]@{
        File   = $a.File
        Kind   = $a.Kind
        SizeMB = [math]::Round($item.Length / 1MB, 2)
        SHA256 = (Get-FileHash -LiteralPath $a.Path -Algorithm SHA256).Hash.ToLower()
    }
}

$rows | Format-Table -AutoSize | Out-String | Write-Host

# --- 4. packages/SHA256SUMS.txt -----------------------------------------------
$sumsText = ($rows | ForEach-Object { '{0}  {1}' -f $_.SHA256, $_.File }) -join "`r`n"
Write-Utf8NoBom -Path (Join-Path $pkgDir 'SHA256SUMS.txt') -Text ($sumsText + "`r`n")

# --- 5. packages/README.md ----------------------------------------------------
$builtAt = (Get-Date).ToString('yyyy-MM-dd HH:mm')
$table   = ($rows | ForEach-Object { '| `{0}` | {1} | {2} MB |' -f $_.File, $_.Kind, $_.SizeMB }) -join "`r`n"

$setup = ($rows | Where-Object { $_.File -like '*-setup.exe' } | Select-Object -First 1)
if (-not $setup) { $setup = $rows[0] }

$portable = ($rows | Where-Object { $_.File -like '*-portable.exe' } | Select-Object -First 1)
$portableName = if ($portable) { $portable.File } else { $setup.File }

$template = @'
# Paket rilis {{PRODUCT}}

Folder ini berisi installer **siap pakai** hasil build dari source code di repo ini.
Kamu **tidak perlu** memasang Node.js / Rust kalau hanya ingin memakai aplikasinya.

- **Versi:** `{{VERSION}}`
- **Platform:** Windows 10/11 **64-bit** (`windows-x64`)
- **Dibangun:** {{BUILT_AT}}

| File | Jenis | Ukuran |
| ---- | ----- | ------ |
{{TABLE}}

## Cara install (disarankan)

1. Unduh **`{{SETUP}}`** - klik file di tabel, lalu tombol **Download**.
2. Klik dua kali file tersebut dan ikuti wizard-nya. **Tidak butuh hak admin**, karena
   installer dipasang per-user (`installMode: currentUser`).
3. Jalankan **{{PRODUCT}}** dari Start Menu (shortcut desktop juga ikut dibuat).
4. Klik **"Login pakai kode"** -> buka `link.tidal.com` -> masukkan user code yang tampil
   (link login otomatis disalin ke clipboard). Detailnya di bagian **Setup** pada README utama.

### Kalau SmartScreen muncul

Build ini **belum ditandatangani (unsigned)**, jadi Windows bisa menampilkan
*"Windows protected your PC"*. Klik **More info -> Run anyway**. Ini normal untuk aplikasi
open-source tanpa code-signing certificate.

### Butuh WebView2 Runtime

Installer memakai **WebView2 bootstrapper**, jadi runtime diunduh & dipasang otomatis saat
instalasi kalau belum ada. Untuk versi **portable**, WebView2 harus sudah terpasang di
komputer (Windows 11 dan Windows 10 ter-update biasanya sudah). Kalau belum, ambil
[WebView2 Evergreen Runtime](https://developer.microsoft.com/microsoft-edge/webview2/).

## Versi portable (tanpa installer)

`{{PORTABLE}}` bisa langsung dijalankan dari folder mana pun, termasuk flashdisk. Tidak ada
file yang ditulis ke Program Files, tetapi sesi login tetap disimpan di
`%APPDATA%\com.tidaldownload.app\auth.json`.

## Verifikasi unduhan (opsional)

Cocokkan hash file dengan isi `SHA256SUMS.txt`:

```powershell
Get-FileHash .\{{SETUP}} -Algorithm SHA256
```

## Membangun ulang paket ini

Dari root repo:

```bash
npm install
npm run package:win
```

`scripts/build-package.ps1` menjalankan `npm run tauri build`, menyalin artefak ke folder
ini, lalu memperbarui `SHA256SUMS.txt` dan README ini.

Mau memaketkan hasil build terakhir tanpa build ulang:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/build-package.ps1 -SkipBuild
```

---

**PERINGATAN:** Gunakan hanya untuk konten yang berhak kamu unduh sesuai ketentuan TIDAL dan
hukum yang berlaku.
'@

$readme = $template
$readme = $readme.Replace('{{PRODUCT}}', $productName)
$readme = $readme.Replace('{{VERSION}}', $version)
$readme = $readme.Replace('{{BUILT_AT}}', $builtAt)
$readme = $readme.Replace('{{TABLE}}', $table)
$readme = $readme.Replace('{{SETUP}}', $setup.File)
$readme = $readme.Replace('{{PORTABLE}}', $portableName)

Write-Utf8NoBom -Path (Join-Path $pkgDir 'README.md') -Text $readme

Write-Host "==> Selesai. Folder siap dibagikan: $pkgDir"
Get-ChildItem -Path $outDir -File |
    Select-Object Name, @{ n = 'SizeMB'; e = { [math]::Round($_.Length / 1MB, 2) } } |
    Format-Table -AutoSize
