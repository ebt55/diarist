const dateFormat = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});
const dayFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "full" });

/** Unix ms to a short local date and time, e.g. "14 Oct 2026, 16:45". */
export function formatDate(unixMs: number): string {
  return dateFormat.format(new Date(unixMs));
}

const shortThisYear = new Intl.DateTimeFormat(undefined, {
  day: "numeric",
  month: "short",
  hour: "numeric",
  minute: "2-digit",
});
const shortOtherYear = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });
const thisYear = new Date().getFullYear();

/** Compact date for list rows: "14 Oct, 2:12 pm", or "3 Mar 2025" for other years. */
export function formatShortDate(unixMs: number): string {
  const d = new Date(unixMs);
  return (d.getFullYear() === thisYear ? shortThisYear : shortOtherYear).format(d);
}

export function formatDay(unixMs: number): string {
  return dayFormat.format(new Date(unixMs));
}

/** Milliseconds to "1 h 05 min", "47 min" or "40 s"; "–" when unknown. */
export function formatDuration(ms: number | null): string {
  if (ms === null || ms < 0) return "–";
  const totalSeconds = Math.round(ms / 1000);
  if (totalSeconds < 60) return `${totalSeconds} s`;
  const totalMinutes = Math.round(totalSeconds / 60);
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (hours === 0) return `${minutes} min`;
  return `${hours} h ${String(minutes).padStart(2, "0")} min`;
}

const pad = (n: number) => String(n).padStart(2, "0");

/** Media time as "m:ss" or "h:mm:ss" (always "h:mm:ss" when `long`). */
export function formatClock(ms: number, long = false): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0 || long) return `${h}:${pad(m)}:${pad(s)}`;
  return `${m}:${pad(s)}`;
}

/** Bytes as "742 MB" or "1.08 GB" (decimal units, like file managers). */
export function formatBytes(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(2)} GB`;
  if (bytes >= 1e6) return `${Math.round(bytes / 1e6)} MB`;
  if (bytes >= 1e3) return `${Math.round(bytes / 1e3)} kB`;
  return `${bytes} B`;
}

/** Megabytes of VRAM as "1.15 GB" or "300 MB". */
export function formatVram(mb: number): string {
  return mb >= 1000 ? `${(mb / 1000).toFixed(2)} GB` : `${mb} MB`;
}

export function plural(n: number, one: string, many = `${one}s`): string {
  return `${n.toLocaleString()} ${n === 1 ? one : many}`;
}
