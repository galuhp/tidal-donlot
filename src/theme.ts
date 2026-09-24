/**
 * Helper tema (light / dark).
 *
 * Sumber kebenaran ada di atribut `data-theme` pada elemen <html>; semua warna
 * di App.css memakai CSS variables sehingga pergantian tema hanya perlu
 * mengganti atribut ini (tanpa re-render style apapun).
 */

export type Theme = "dark" | "light";

/** Key localStorage untuk menyimpan pilihan tema user. */
export const THEME_KEY = "theme";

/** Baca tema tersimpan; `null` kalau belum pernah dipilih / nilainya tidak valid. */
export function readStoredTheme(): Theme | null {
  try {
    const v = localStorage.getItem(THEME_KEY);
    return v === "dark" || v === "light" ? v : null;
  } catch {
    return null;
  }
}

/** Preferensi sistem — dipakai sebagai default kalau belum ada yang tersimpan. */
export function systemTheme(): Theme {
  const dark = window.matchMedia?.("(prefers-color-scheme: dark)")?.matches;
  return dark ? "dark" : "light";
}

/**
 * Set / ganti tema aktif di <html>.
 * `persist = true` dipakai saat user memilih sendiri lewat toggle supaya
 * pilihannya tersimpan di localStorage.
 */
export function applyTheme(theme: Theme, persist = false): void {
  document.documentElement.setAttribute("data-theme", theme);
  if (!persist) return;
  try {
    localStorage.setItem(THEME_KEY, theme);
  } catch {
    /* localStorage bisa tidak tersedia — tema tetap jalan untuk sesi ini. */
  }
}
