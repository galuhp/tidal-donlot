import { useCallback, useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { confirm, open } from "@tauri-apps/plugin-dialog";
import {
  AlertCircle,
  CheckCircle2,
  Disc3,
  Download,
  FolderOpen,
  ListMusic,
  Loader2,
  LogOut,
  Moon,
  Search,
  Sun,
} from "lucide-react";
import { applyTheme, readStoredTheme, systemTheme, type Theme } from "./theme";
import "./App.css";

type Track = {
  id: string;
  title: string;
  artists: string;
  album: string;
  album_id: string | null;
  cover_url: string | null;
  duration: number | null;
  /** "song" | "album" | "artist" — dari backend, dipakai untuk menandai hasil. */
  kind: string;
};

type Progress = {
  track_id: string;
  title: string;
  status: "started" | "progress" | "done" | "error";
  downloaded: number;
  total: number;
  message: string | null;
  path: string | null;
};

type DeviceEvent = {
  stage: string;
  user_code?: string | null;
  verification_uri?: string | null;
  verificationUriComplete?: string | null;
  copied?: boolean | null;
  browserOpened?: boolean | null;
  message?: string | null;
};

type DeviceState = {
  stage: "code" | "done" | "error" | null;
  userCode: string | null;
  verificationUri: string | null;
  verificationUriComplete: string | null;
  copied: boolean;
  browserOpened: boolean;
  message: string | null;
};

/** Label jenis konten untuk badge di pojok atas-kiri tiap card hasil. */
const KIND_LABEL: Record<string, string> = {
  song: "Lagu",
  album: "Album",
  artist: "Artis",
};

/** Hanya "song" yang bisa langsung diunduh; album/artis harus dibuka dulu. */
const isSong = (t: Track) => t.kind === "song";

const QUALITIES = [
  { value: "HI_RES_LOSSLESS", label: "Hi-Res Lossless (24-bit)" },
  { value: "LOSSLESS", label: "Lossless (FLAC)" },
  { value: "HIGH", label: "High (AAC 320)" },
  { value: "LOW", label: "Low (AAC 96)" },
];

function fmtDuration(s?: number | null) {
  if (!s) return "";
  const m = Math.floor(s / 60);
  return `${m}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
}

function fmtBytes(n: number) {
  if (n > 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024).toFixed(0)} KB`;
}

/** Banyak placeholder skeleton yang ditampilkan saat fetch sedang berjalan. */
const SKELETON_KEYS = [0, 1, 2, 3, 4, 5];

/**
 * Tombol toggle tema. Dua icon (Sun & Moon) ditumpuk di posisi yang sama lalu
 * di-cross-fade lewat CSS sesuai `[data-theme]` yang aktif.
 */
function ThemeToggle({
  theme,
  onToggle,
}: {
  theme: Theme;
  onToggle: () => void;
}) {
  const target = theme === "dark" ? "tema terang" : "tema gelap";
  return (
    <button
      type="button"
      className="icon-btn theme-toggle"
      onClick={onToggle}
      title={`Ganti ke ${target}`}
      aria-label={`Ganti ke ${target}`}
    >
      <span className="icon-stack" aria-hidden="true">
        <Sun className="icon icon-sun" size={18} />
        <Moon className="icon icon-moon" size={18} />
      </span>
    </button>
  );
}

export default function App() {
  const [loggedIn, setLoggedIn] = useState(false);
  const [loginBusy, setLoginBusy] = useState(false);
  const [logoutBusy, setLogoutBusy] = useState(false);
  const [query, setQuery] = useState("");
  const [tracks, setTracks] = useState<Track[]>([]);
  const [searching, setSearching] = useState(false);
  const [quality, setQuality] = useState("LOSSLESS");
  const [outDir, setOutDir] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState<Record<string, Progress>>({});
  const [queue, setQueue] = useState<string[]>([]);
  const [device, setDevice] = useState<DeviceState>({
    stage: null,
    userCode: null,
    verificationUri: null,
    verificationUriComplete: null,
    copied: false,
    browserOpened: false,
    message: null,
  });
  const busyRef = useRef(false);
  const [theme, setTheme] = useState<Theme>(
    () => readStoredTheme() ?? systemTheme(),
  );

  /** Sinkronkan tema aktif ke atribut `data-theme` di <html>. */
  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  useEffect(() => {
    invoke("auth_status")
      .then((s: any) => setLoggedIn(!!s?.loggedIn))
      .catch(() => setLoggedIn(false));
    const unProgress = listen<Progress>("download-progress", (e) => {
      const p = e.payload;
      setProgress((prev) => ({ ...prev, [p.track_id]: p }));
      if (p.status === "done" || p.status === "error") {
        setQueue((q) => q.filter((id) => id !== p.track_id));
      }
    });
    const unDevice = listen<DeviceEvent>("device-login", (e) => {
      const d = e.payload;
      setDevice({
        stage: (d.stage as DeviceState["stage"]) ?? null,
        userCode: d.user_code ?? null,
        verificationUri: d.verification_uri ?? null,
        verificationUriComplete: d.verificationUriComplete ?? null,
        copied: !!d.copied,
        browserOpened: !!d.browserOpened,
        message: d.message ?? null,
      });
      if (d.stage === "error" && d.message) setError(d.message);
    });
    return () => {
      unProgress.then((f) => f());
      unDevice.then((f) => f());
    };
  }, []);

  const runQueue = useCallback(async () => {
    if (busyRef.current) return;
    busyRef.current = true;
    try {
      for (;;) {
        const snapshot = await new Promise<string[]>((resolve) =>
          setQueue((q) => {
            resolve(q);
            return q;
          }),
        );
        const nextId = snapshot[0];
        if (!nextId) break;
        const track = tracks.find((t) => t.id === nextId);
        if (!track || !outDir) break;
        try {
          await invoke("download_track", { track, quality, dir: outDir });
        } catch (e) {
          setError(String(e));
          setQueue((q) => q.filter((id) => id !== nextId));
          break;
        }
      }
    } finally {
      busyRef.current = false;
    }
  }, [tracks, outDir, quality]);

  /** Login pakai kode (Device Authorization Grant) — tanpa daftar aplikasi. */
  async function loginDevice() {
    setLoginBusy(true);
    setError(null);
    setDevice({
      stage: null,
      userCode: null,
      verificationUri: null,
      verificationUriComplete: null,
      copied: false,
      browserOpened: false,
      message: null,
    });
    try {
      await invoke("login_device");
      setLoggedIn(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoginBusy(false);
    }
  }

  /** Logout: hapus sesi tersimpan supaya user bisa connect ulang / ganti akun. */
  async function logout() {
    const ok = await confirm(
      "Logout dari akun TIDAL ini? Kamu perlu login ulang untuk mengunduh.",
      { title: "Logout", kind: "warning" },
    );
    if (!ok) return;
    setLogoutBusy(true);
    setError(null);
    try {
      await invoke("logout");
      setLoggedIn(false);
      setDevice({
        stage: null,
        userCode: null,
        verificationUri: null,
        verificationUriComplete: null,
        copied: false,
        browserOpened: false,
        message: null,
      });
      setTracks([]);
      setProgress({});
      setQueue([]);
    } catch (e) {
      setError(String(e));
    } finally {
      setLogoutBusy(false);
    }
  }

  /** Toggle Light/Dark. Pilihan user disimpan supaya bertahan setelah restart. */
  function toggleTheme() {
    const next: Theme = theme === "dark" ? "light" : "dark";
    setTheme(next);
    applyTheme(next, true);
  }

  async function copyToClipboard(text: string | null) {
    if (!text) return;
    try {
      await invoke("copy_text", { text });
    } catch (e) {
      setError(String(e));
    }
  }

  async function doSearch(e?: FormEvent) {
    e?.preventDefault();
    if (!query.trim()) return;
    setSearching(true);
    setError(null);
    try {
      const res: Track[] = await invoke("search", { query });
      setTracks(res);
      if (res.length === 0)
        setError("Tidak ada hasil untuk pencarian ini.");
    } catch (err) {
      setError(String(err));
    } finally {
      setSearching(false);
    }
  }

  async function pickDir() {
    const dir = await open({ directory: true, multiple: false });
    if (typeof dir === "string") setOutDir(dir);
  }

  function enqueue(t: Track) {
    if (!outDir) {
      setError("Pilih folder tujuan dulu.");
      return;
    }
    setQueue((q) => (q.includes(t.id) ? q : [...q, t.id]));
    setTimeout(runQueue, 0);
  }

  function enqueueAll() {
    if (!outDir) {
      setError("Pilih folder tujuan dulu.");
      return;
    }
    setQueue((q) => {
      // hanya lagu yang bisa diunduh; album/artis harus dibuka dulu
      const ids = tracks
        .filter(isSong)
        .map((t) => t.id)
        .filter((id) => !q.includes(id));
      return [...q, ...ids];
    });
    setTimeout(runQueue, 0);
  }

  /** Buka album/artis dari hasil pencarian → tampilkan lagu-lagunya. */
  async function openTarget(t: Track) {
    const url =
      t.kind === "album"
        ? `https://tidal.com/album/${t.id}`
        : `https://tidal.com/artist/${t.id}`;
    setQuery(url);
    setSearching(true);
    setError(null);
    try {
      const res: Track[] = await invoke("search", { query: url });
      setTracks(res);
      if (res.length === 0) setError("Tidak ada lagu untuk ditampilkan.");
    } catch (err) {
      setError(String(err));
    } finally {
      setSearching(false);
    }
  }

  if (!loggedIn) {
    const waiting = device.stage === "code";
    return (
      <main className="login-screen">
        <div className="login-theme-toggle">
          <ThemeToggle theme={theme} onToggle={toggleTheme} />
        </div>
        <div className="login-card">
          <div className="logo-mark">T</div>
          <h1>Tidal Downloader</h1>

          {!waiting && (
            <>
              <p>
                Login pakai kode — <strong>tanpa perlu daftar aplikasi</strong>.
                <br />
                Link login otomatis disalin ke clipboard; tinggal tempel di browser.
              </p>
              <button
                className="btn primary"
                disabled={loginBusy}
                onClick={loginDevice}
              >
                {loginBusy ? "Menunggu persetujuan…" : "Login pakai kode"}
              </button>
            </>
          )}

          {waiting && (
            <div className="device-panel">
              <p>Masukkan kode ini di situs TIDAL:</p>
              <div className="user-code">{device.userCode}</div>
              <div className="device-actions">
                <button
                  className="btn"
                  onClick={() => copyToClipboard(device.userCode)}
                >
                  Salin kode
                </button>
                <button
                  className="btn primary"
                  onClick={() =>
                    copyToClipboard(
                      device.verificationUriComplete ?? device.verificationUri,
                    )
                  }
                >
                  Salin link login
                </button>
              </div>
              <p className={device.copied ? "ok" : "muted"}>
                {device.copied
                  ? "✓ Link login sudah ada di clipboard — tempel (Ctrl+V) di browser."
                  : "Salin link di atas, lalu buka di browser kamu."}
              </p>
              <p className="muted">
                {device.browserOpened
                  ? "Browser juga sudah dibuka otomatis di link.tidal.com."
                  : "Buka link.tidal.com di browser, lalu setujui permintaan login."}
              </p>
              <p className="muted">
                Menunggu persetujuan… jangan tutup aplikasi ini.
              </p>
            </div>
          )}

          {error && <p className="error">{error}</p>}
        </div>
      </main>
    );
  }

  const songs = tracks.filter(isSong).length;
  const albums = tracks.filter((t) => t.kind === "album").length;
  const artists = tracks.filter((t) => t.kind === "artist").length;
  const summary =
    albums + artists > 0
      ? `${tracks.length} hasil · ${songs} lagu, ${albums} album, ${artists} artis`
      : `${tracks.length} hasil`;

  return (
    <main className="container">
      <header className="toolbar">
        <div className="brand">
          <div className="logo-mark small">T</div>
          <h1>Tidal Downloader</h1>
        </div>
        <div className="spacer" />
        <select
          className="toolbar-select"
          value={quality}
          onChange={(e) => setQuality(e.target.value)}
          title="Kualitas audio saat diunduh"
          aria-label="Kualitas audio"
        >
          {QUALITIES.map((q) => (
            <option key={q.value} value={q.value}>
              {q.label}
            </option>
          ))}
        </select>
        <button
          className="btn"
          onClick={pickDir}
          title={outDir ?? "Pilih folder tujuan"}
        >
          <FolderOpen size={16} aria-hidden="true" />
          <span className="btn-label">
            {outDir ? outDir.split(/[\\/]/).pop() : "Pilih Folder"}
          </span>
        </button>
        <ThemeToggle theme={theme} onToggle={toggleTheme} />
        <button className="btn" onClick={logout} disabled={logoutBusy}>
          <LogOut size={16} aria-hidden="true" />
          <span className="btn-label">{logoutBusy ? "Keluar…" : "Logout"}</span>
        </button>
      </header>

      <form className="searchbar" onSubmit={doSearch}>
        <div className="search-pill">
          <Search className="search-icon" size={18} aria-hidden="true" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Cari lagu, artis, album… atau tempel link TIDAL"
            aria-label="Cari lagu, artis, album, atau tempel link TIDAL"
          />
          <button className="search-submit" type="submit" disabled={searching}>
            {searching ? "Mencari…" : "Cari"}
          </button>
        </div>
      </form>

      {error && <p className="error">{error}</p>}

      {searching ? (
        <section className="grid" aria-busy="true" aria-label="Memuat hasil">
          {SKELETON_KEYS.map((i) => (
            <div className="card skeleton" key={i}>
              <div className="sk sk-cover" />
              <div className="sk sk-line" />
              <div className="sk sk-line short" />
            </div>
          ))}
        </section>
      ) : tracks.length > 0 ? (
        <section className="results">
          <div className="results-head">
            <span>{summary}</span>
            <button className="btn small" onClick={enqueueAll}>
              <Download size={15} aria-hidden="true" />
              <span className="btn-label">Download semua</span>
            </button>
          </div>
          <div className="grid">
            {tracks.map((t) => {
              const p = progress[t.id];
              const inQueue = queue.includes(t.id);
              const done = p?.status === "done";
              const pct =
                p && p.total > 0 ? Math.round((p.downloaded / p.total) * 100) : 0;
              return (
                <article
                  key={t.id}
                  className={`card ${t.kind}${done ? " done" : ""}`}
                >
                  <span className={`badge ${t.kind}`}>
                    {KIND_LABEL[t.kind] ?? "Lagu"}
                  </span>
                  <div className="cover-wrap">
                    {t.cover_url ? (
                      <img
                        className="cover"
                        src={t.cover_url}
                        alt={`Cover ${t.title}`}
                        loading="lazy"
                      />
                    ) : (
                      <div className="cover cover-fallback">
                        <Disc3 size={30} aria-hidden="true" />
                      </div>
                    )}
                    {isSong(t) && (
                      <button
                        type="button"
                        className={done ? "card-download done" : "card-download"}
                        disabled={inQueue || done}
                        onClick={() => enqueue(t)}
                        aria-label={
                          done
                            ? "Sudah diunduh"
                            : inQueue
                              ? "Sedang diunduh"
                              : "Unduh lagu"
                        }
                        title={
                          done
                            ? "Sudah diunduh"
                            : inQueue
                              ? "Sedang diunduh…"
                              : "Unduh lagu ini"
                        }
                      >
                        {done ? (
                          <CheckCircle2 size={16} aria-hidden="true" />
                        ) : inQueue ? (
                          <Loader2 className="spin" size={16} aria-hidden="true" />
                        ) : (
                          <Download size={16} aria-hidden="true" />
                        )}
                      </button>
                    )}
                  </div>
                  <div className="card-body">
                    <span className="card-title" title={t.title}>
                      {t.title}
                    </span>
                    <span className="card-sub" title={t.artists}>
                      {t.artists}
                      {t.album ? ` — ${t.album}` : ""}
                      {t.duration ? ` · ${fmtDuration(t.duration)}` : ""}
                    </span>
                    {p && p.status === "progress" && p.total > 0 && (
                      <>
                        <div className="bar">
                          <div style={{ width: `${pct}%` }} />
                        </div>
                        <span className="card-note">
                          {fmtBytes(p.downloaded)} / {fmtBytes(p.total)}
                        </span>
                      </>
                    )}
                    {done && (
                      <span className="card-note ok" title={p?.path ?? undefined}>
                        <CheckCircle2 size={12} aria-hidden="true" />
                        <span className="path">{p?.path}</span>
                      </span>
                    )}
                    {p?.status === "error" && (
                      <span className="card-note error">
                        <AlertCircle size={12} aria-hidden="true" />
                        {p.message}
                      </span>
                    )}
                    {!isSong(t) && (
                      <button
                        className="btn small open-btn"
                        onClick={() => openTarget(t)}
                        title={
                          t.kind === "album"
                            ? "Lihat lagu di album ini"
                            : "Lihat lagu artis ini"
                        }
                      >
                        <ListMusic size={14} aria-hidden="true" />
                        <span className="btn-label">Buka</span>
                      </button>
                    )}
                  </div>
                </article>
              );
            })}
          </div>
        </section>
      ) : (
        <div className="empty-state">
          <Disc3
            className="empty-icon"
            size={96}
            strokeWidth={1.2}
            aria-hidden="true"
          />
          <p>Cari lagu, album, atau tempel link TIDAL untuk mulai</p>
        </div>
      )}
    </main>
  );
}

