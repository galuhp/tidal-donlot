# Tidal Downloader

Desktop app untuk mencari & mengunduh track TIDAL dengan metadata lengkap (title, artist, album, cover art embedded) — dibangun dengan **Tauri 2 + Rust + React + TypeScript**.

> ⚠️ **Disclaimer**: Gunakan hanya untuk konten yang berhak kamu unduh sesuai ketentuan TIDAL dan hukum yang berlaku. Endpoint streaming memerlukan akun premium dan persetujuan developer; penggunaan di luar ketentuan layanan TIDAL adalah tanggung jawab pengguna.

## Fitur

- 🔐 **Login pakai kode (OAuth 2.0 Device Authorization Grant)** — kode di `link.tidal.com`, **tanpa daftar aplikasi** & **tanpa `CLIENT_ID`/`CLIENT_SECRET`**
- 📋 Link login **otomatis disalin ke clipboard** (auto-open browser tidak selalu berhasil) + tombol "Salin kode" & "Salin link login"
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

Cara ini memakai kredensial publik klien TIDAL (`DEVICE_CLIENT_ID` di `src-tauri/src/auth.rs`),
sama seperti pendekatan proyek open-source
[Tidal-Media-Downloader-PRO](https://github.com/yaronzz/Tidal-Media-Downloader-PRO).
**Kamu tidak perlu akun developer, tidak perlu `CLIENT_ID`/`CLIENT_SECRET`.**

Catatan: aplikasi tetap **mencoba** membuka browser default; karena itu tidak selalu berhasil di
Windows, statusnya ditampilkan di layar dan link sudah ada di clipboard — jadi selalu ada cara lanjut.

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

## Arsitektur

```
src/                    # Frontend React + TS
  App.tsx               # Login, search, queue download, progress bar
src-tauri/
  src/
    lib.rs              # Tauri commands (login_device, copy_text, search, download_track, ...)
    auth.rs             # Device Authorization Grant (login pakai kode) + refresh + persist
    tidal.rs            # Client API v2 (JSON:API) + resolve stream URL
    downloader.rs       # Download audio + progress event + embed tags
  capabilities/         # Permission Tauri (dialog, event, opener)
```

## Catatan penting

- **API v2** (`openapi.tidal.com/v2`) hanya menyediakan **metadata & cover art**. URL audio diambil via endpoint playback internal yang memerlukan **token user premium** — jika akun tidak memenuhi syarat, request playback akan ditolak dengan pesan error yang jelas.
- Jika TIDAL menyetujui scope terbatas untuk aplikasimu, fitur download mungkin tidak tersedia — fitur search/cover/playlist tetap berfungsi.
- Kredensial disimpan plain-text di `%APPDATA%/com.tidaldownload.app/auth.json`. Untuk produksi, pindahkan ke OS keychain (mis. crate `keyring`).
- Kualitas Hi-Res kadang berbentuk stream DASH terenkripsi dan akan ditolak otomatis dengan pesan; coba kualitas `LOSSLESS`.
- **Login memakai `client_id` publik klien TIDAL** (`DEVICE_CLIENT_ID`). Ini bukan aplikasi yang kamu daftarkan, jadi bisa saja dicabut/dibatasi TIDAL kapan pun. Token tetap milik akunmu sendiri; refresh memakai kredensial yang sama seperti saat login.
- Sudah ada fallback otomatis: kalau server menolak permintaan token yang menyertakan `client_secret`, aplikasi mengulang tanpa `client_secret`.
