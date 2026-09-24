# Tidal Downloader

Desktop app untuk mencari & mengunduh track TIDAL dengan metadata lengkap (title, artist, album, cover art embedded) — dibangun dengan **Tauri 2 + Rust + React + TypeScript**.

> ⚠️ **Disclaimer**: Gunakan hanya untuk konten yang berhak kamu unduh sesuai ketentuan TIDAL dan hukum yang berlaku. Endpoint streaming memerlukan akun premium dan persetujuan developer; penggunaan di luar ketentuan layanan TIDAL adalah tanggung jawab pengguna.

## ⬇️ Download (langsung pakai, tanpa build sendiri)

Installer **Windows 10/11 x64** yang sudah jadi ada di folder [`packages/`](packages/):

| File (di `packages/windows-x64/`) | Keterangan |
| --- | --- |
| `Tidal-Downloader-<versi>-x64-setup.exe` | **Disarankan** — installer per-user (tanpa hak admin), WebView2 otomatis |
| `Tidal-Downloader-<versi>-x64.msi` | Installer MSI untuk deployment / GPO |
| `Tidal-Downloader-<versi>-portable.exe` | Portable, dijalankan tanpa install |

Unduh filenya dari GitHub (buka folder → klik file → **Download**), atau langsung:
`https://github.com/galuhp/tidal-donlot/raw/main/packages/windows-x64/<nama-file>`.

Panduan install, catatan SmartScreen, dan hash **SHA-256** ada di
[`packages/README.md`](packages/README.md).

## Fitur

- 🔐 **Login pakai kode (OAuth 2.0 Device Authorization Grant)** — kode di `link.tidal.com`, **tanpa daftar aplikasi** & **tanpa `CLIENT_ID`/`CLIENT_SECRET`**
- 📋 Link login **otomatis disalin ke clipboard** + tombol "Salin kode" & "Salin link login" (link dinormalkan ke `https://` karena balasan TIDAL tidak memuat skema — ini penyebab tombol buka-browser dulu sering gagal)
- ♻️ Token auto-refresh + session tersimpan di disk
- 🔍 **Pencarian track** via TIDAL API v2 (JSON:API)
- ⬇️ **Download queue** dengan progress bar real-time (event Rust → UI)
- 🎚️ **Pilihan kualitas**: Hi-Res Lossless / Lossless (FLAC) / High (AAC 320) / Low (AAC 96)
- 🏷️ **Metadata embedded** (lofty): judul, artis, album, cover art

## Prasyarat

- Node.js 18+ dan npm
- [Rust](https://rustup.rs) (stable, target MSVC di Windows)
- [Prasyarat Tauri 2 untuk Windows](https://tauri.app/start/prerequisites/) (MSVC Build Tools + WebView2)

## Setup

### Login (tanpa registrasi)

Tidak ada konfigurasi yang diperlukan. Jalankan aplikasi lalu klik **"Login pakai kode"**:

1. Aplikasi meminta *device code* dari TIDAL dan menampilkan **user code** (mis. `ABCD-1234`)
2. **Link login otomatis disalin ke clipboard** — tempel (Ctrl+V) di browser, atau pakai tombol **"Salin link login"**
3. Setujui permintaan login di `link.tidal.com`; aplikasi mem-poll token tiap beberapa detik
4. Begitu disetujui, kamu langsung masuk

Cara ini memakai kredensial publik **klien TV TIDAL** (`DEVICE_CLIENT_ID` di `src-tauri/src/auth.rs`),
sama seperti modul [OrpheusDL-TIDAL](https://github.com/bascurtiz/orpheusdl-tidal)
(`TidalTvSession`, nilai default `tv_atmos_token`/`tv_atmos_secret`).
Inilah klien yang diizinkan TIDAL untuk mengambil URL stream, sehingga unduhan bisa berjalan tanpa
memakai client id web/desktop publik seperti tidal-dl (yaronzz).
**Kamu tidak perlu akun developer, tidak perlu `CLIENT_ID`/`CLIENT_SECRET`.**

Catatan: link dari TIDAL tidak memuat skema (`link.tidal.com/ABCD`); aplikasi menambah `https://`
(`absolute_url` di `auth.rs`) sehingga **buka browser otomatis** berfungsi dan link di clipboard bisa
langsung ditempel (Ctrl+V). Kalau browser tetap tidak terbuka, statusnya ditampilkan di layar dan link
sudah ada di clipboard — jadi selalu ada cara lanjut.

### Menjalankan

```bash
npm install
npm run tauri dev
```

### Build produksi

```bash
npm run tauri build
```

Hasil installer ada di `src-tauri/target/release/bundle/`.

### Siapkan folder `packages/` (siap dibagikan)

```bash
npm run package:win
```

Perintah ini menjalankan `npm run tauri build`, lalu menyalin hasilnya ke
`packages/windows-x64/` dengan nama file yang rapi, serta memperbarui
`packages/SHA256SUMS.txt` dan `packages/README.md` secara otomatis
(sumber: [`scripts/build-package.ps1`](scripts/build-package.ps1)).
Kirim folder `packages/` itu ke pengguna lain agar mereka bisa langsung mengunduh EXE-nya
tanpa perlu memasang Node.js/Rust.

## Arsitektur

```
src/                    # Frontend React + TS
  App.tsx               # Login, search, queue download, progress bar
src-tauri/
  src/
    lib.rs              # Tauri commands (login_device, copy_text, search, download_track, ...)
    auth.rs             # Device Authorization Grant (login pakai kode) + refresh + persist
    tidal.rs            # Client API v2 (JSON:API) + paginasi + resolve stream URL
    downloader.rs       # Download audio + progress event + embed tags
  capabilities/         # Permission Tauri (dialog, event, opener)
```

## Catatan penting

- **API v2** (`openapi.tidal.com/v2`) dipakai untuk **metadata, pencarian & cover art**. URL audio diambil dari **API v1** (`api.tidal.com/v1`) dengan klien TV — bukan dari v2.
- **Endpoint playback sudah diaktifkan** (Sept 2026): aplikasi memakai **klien TV TIDAL** (`DEVICE_CLIENT_ID` = klien AndroidTV publik yang dipakai modul [OrpheusDL-TIDAL](https://github.com/bascurtiz/orpheusdl-tidal)) dan mengambil URL audio dari API v1 `tracks/{id}/playbackinfopostpaywall/**v4**` dengan header `X-Tidal-Token: <client_id>` + `User-Agent: TIDAL_ANDROID/1039 okhttp/3.14.9`. Yang menentukan izin stream adalah **client id** ini, bukan scope: scope device login tetap `r_usr w_usr` (menambah `playback` ditolak `invalid_scope`).
- Kalau stream tetap ditolak (`4005` / `PREREQUISITE_MISSING`): pastikan akun berlangganan HiFi/Plus, lalu **login ulang** (sesi lama dari klien device non-TV otomatis dimigrasi: `client_id` dikosongkan supaya refresh berikutnya memakai klien TV, lihat `migrate_session_client` di `auth.rs`), dan cek region track.
- Fitur **search / album / playlist / cover / badge jenis** tetap berfungsi penuh tanpa scope playback.
- Kredensial disimpan plain-text di `%APPDATA%/com.tidaldownload.app/auth.json`. Untuk produksi, pindahkan ke OS keychain (mis. crate `keyring`).
- Kualitas Hi-Res kadang berbentuk stream DASH terenkripsi dan akan ditolak otomatis dengan pesan; coba kualitas `LOSSLESS`.
- **Login memakai `client_id` publik klien TV TIDAL** (`DEVICE_CLIENT_ID` = `4N3n6Q1x95LL5K7p`, nilai default `tv_atmos_token` di modul OrpheusDL-TIDAL). Ini bukan aplikasi yang kamu daftarkan, jadi bisa saja dicabut/dibatasi TIDAL kapan pun. Token tetap milik akunmu sendiri; refresh memakai kredensial yang sama seperti saat login.
- Sudah ada fallback otomatis: kalau server menolak permintaan token yang menyertakan `client_secret`, aplikasi mengulang tanpa `client_secret`.
- **Endpoint v2 yang terbukti jalan** (diverifikasi langsung ke `openapi.tidal.com`): pencarian butuh dua langkah (`/searchResults?filter%5Bquery%5D=…` menghasilkan id, lalu `/searchResults/{id}?include=…`), isi album & playlist diambil dari `relationships/items` (bukan `relationships/tracks` — endpoint itu membalas error), dan halaman berikutnya diikuti dari `links.next`. Pola `items` ini sama dengan proyek tidal-dl (yaronzz) yang memakai `albums/{id}/items` dan `playlists/{id}/items` di API v1.
- Scope device login adalah `r_usr w_usr` (persis seperti `TidalTvSession.auth()` di OrpheusDL); menambahkan `playback` ditolak (`invalid_scope`) — jadi scope **bukan** penentu izin stream, melainkan client id pada header `X-Tidal-Token`.
- Hasil pencarian menampilkan **badge jenis: Lagu / Album / Artis**. Dokumen `searchResults` memang punya relationship `albums`, `artists`, `tracks`, `videos` (selaras dengan spec resmi TIDAL — klien `tidalv2` memakai `include=albums|artists|tracks`). Hasil Album/Artis punya tombol **Buka** untuk menampilkan lagu-lagunya; hanya **Lagu** yang bisa langsung diunduh.
