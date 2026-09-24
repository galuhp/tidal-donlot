import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { confirm, open } from "@tauri-apps/plugin-dialog";
import "./App.css";

type Track = {
  id: string;
  title: string;
  artists: string;
  album: string;
  album_id: string | null;
  cover_url: string | null;
  duration: number | null;
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

  async function copyToClipboard(text: string | null) {
    if (!text) return;
    try {
      await invoke("copy_text", { text });
    } catch (e) {
      setError(String(e));
    }
  }

  async function doSearch(e?: React.FormEvent) {
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
      const ids = tracks.map((t) => t.id).filter((id) => !q.includes(id));
      return [...q, ...ids];
    });
    setTimeout(runQueue, 0);
  }

  if (!loggedIn) {
    const waiting = device.stage === "code";
    return (
      <main className="login-screen">
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

  return (
    <main className="container">
      <header className="topbar">
        <div className="logo-mark small">T</div>
        <h1>Tidal Downloader</h1>
        <div className="spacer" />
        <select value={quality} onChange={(e) => setQuality(e.target.value)}>
          {QUALITIES.map((q) => (
            <option key={q.value} value={q.value}>
              {q.label}
            </option>
          ))}
        </select>
        <button className="btn" onClick={pickDir}>
          {outDir ? `📁 ${outDir.split(/[\\/]/).pop()}` : "📁 Pilih Folder"}
        </button>
        <button className="btn" onClick={logout} disabled={logoutBusy}>
          {logoutBusy ? "Keluar…" : "Logout"}
        </button>
      </header>

      <form className="searchbar" onSubmit={doSearch}>
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Cari lagu, artis, album… atau tempel link TIDAL"
        />
        <button className="btn primary" type="submit" disabled={searching}>
          {searching ? "Mencari…" : "Cari"}
        </button>
      </form>

      {error && <p className="error">{error}</p>}

      {tracks.length > 0 && (
        <div className="results">
          <div className="results-head">
            <span>{tracks.length} hasil</span>
            <button className="btn" onClick={enqueueAll}>
              ⬇ Download semua
            </button>
          </div>
          <ul className="track-list">
            {tracks.map((t) => {
              const p = progress[t.id];
              const inQueue = queue.includes(t.id);
              const pct =
                p && p.total > 0 ? Math.round((p.downloaded / p.total) * 100) : 0;
              return (
                <li key={t.id} className={p?.status === "done" ? "done" : ""}>
                  {t.cover_url ? (
                    <img src={t.cover_url} alt="" />
                  ) : (
                    <div className="cover-fallback" />
                  )}
                  <div className="meta">
                    <strong>{t.title}</strong>
                    <span>
                      {t.artists}
                      {t.album ? ` — ${t.album}` : ""}{" "}
                      {t.duration ? `· ${fmtDuration(t.duration)}` : ""}
                    </span>
                    {p && p.status === "progress" && p.total > 0 && (
                      <>
                        <div className="bar">
                          <div style={{ width: `${pct}%` }} />
                        </div>
                        <span>
                          {fmtBytes(p.downloaded)} / {fmtBytes(p.total)}
                        </span>
                      </>
                    )}
                    {p?.status === "done" && <span className="ok">✓ {p.path}</span>}
                    {p?.status === "error" && (
                      <span className="error">{p.message}</span>
                    )}
                  </div>
                  <button
                    className="btn primary"
                    disabled={inQueue || p?.status === "done"}
                    onClick={() => enqueue(t)}
                  >
                    {inQueue ? "…" : p?.status === "done" ? "✓" : "⬇"}
                  </button>
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </main>
  );
}

