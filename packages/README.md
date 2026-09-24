# Paket rilis Tidal Downloader

Folder ini berisi installer **siap pakai** hasil build dari source code di repo ini.
Kamu **tidak perlu** memasang Node.js / Rust kalau hanya ingin memakai aplikasinya.

- **Versi:** `0.1.0`
- **Platform:** Windows 10/11 **64-bit** (`windows-x64`)
- **Dibangun:** 2026-09-24 12.25

| File | Jenis | Ukuran |
| ---- | ----- | ------ |
| `Tidal-Downloader-0.1.0-portable.exe` | Portable (.exe, tanpa installer) | 8,19 MB |
| `Tidal-Downloader-0.1.0-x64-setup.exe` | Installer NSIS (.exe) | 3,4 MB |
| `Tidal-Downloader-0.1.0-x64.msi` | Installer MSI (.msi) | 4,67 MB |

## Cara install (disarankan)

1. Unduh **`Tidal-Downloader-0.1.0-x64-setup.exe`** - klik file di tabel, lalu tombol **Download**.
2. Klik dua kali file tersebut dan ikuti wizard-nya. **Tidak butuh hak admin**, karena
   installer dipasang per-user (`installMode: currentUser`).
3. Jalankan **Tidal Downloader** dari Start Menu (shortcut desktop juga ikut dibuat).
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

`Tidal-Downloader-0.1.0-portable.exe` bisa langsung dijalankan dari folder mana pun, termasuk flashdisk. Tidak ada
file yang ditulis ke Program Files, tetapi sesi login tetap disimpan di
`%APPDATA%\com.tidaldownload.app\auth.json`.

## Verifikasi unduhan (opsional)

Cocokkan hash file dengan isi `SHA256SUMS.txt`:

```powershell
Get-FileHash .\Tidal-Downloader-0.1.0-x64-setup.exe -Algorithm SHA256
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