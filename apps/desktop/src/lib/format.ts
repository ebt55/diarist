const dateFormat = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

/** Unix ms to a short local date and time, e.g. "14 Oct 2026, 16:45". */
export function formatDate(unixMs: number): string {
  return dateFormat.format(new Date(unixMs));
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
