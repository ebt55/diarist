// Engine and model downloads, shared by the first-run screen and Settings:
// one item at a time (the engine first: model pulls run its downloader),
// with per-item progress from `models:progress`. The backend resumes a
// partial download and checks every file's SHA-256 before it is used.
import { api, errorText, onEvent } from "./api";
import type { DownloadItem } from "./generated/DownloadItem";
import type { DownloadProgress } from "./generated/DownloadProgress";

export type ItemState = "waiting" | "running" | "done" | "failed";

export interface ItemProgress {
  state: ItemState;
  done: number;
  total: number | null;
  error: string | null;
  /** Date.now() when the item started (for the speed). */
  startedAt: number | null;
  /** Bytes already on disk when it started (a resumed download). */
  startBytes: number;
}

/**
 * Megabytes the way docs/04 quotes download sizes: MiB, rounded up unless
 * barely over (engine 98 MB, diarizer 102 MB, EN model 668 MB, both 770 MB).
 */
export function sizeMb(bytes: number): string {
  const mb = bytes / 1048576;
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${Math.max(0, Math.ceil(mb - 0.1))} MB`;
}

class Downloads {
  progress = $state.raw<Record<string, ItemProgress>>({});
  busy = $state(false);
  #listening = false;

  #set(name: string, patch: Partial<ItemProgress>): void {
    const cur: ItemProgress = this.progress[name] ?? {
      state: "waiting",
      done: 0,
      total: null,
      error: null,
      startedAt: null,
      startBytes: 0,
    };
    this.progress = { ...this.progress, [name]: { ...cur, ...patch } };
  }

  #listen(): void {
    if (this.#listening) return;
    this.#listening = true;
    void onEvent<DownloadProgress>("models:progress", (p) => {
      const cur = this.progress[p.name];
      if (!cur || cur.state !== "running" || p.finished) return;
      // The first event carries what a resumed download already had.
      const first = cur.startedAt === null;
      this.#set(p.name, {
        done: p.done_bytes,
        total: p.total_bytes,
        ...(first ? { startedAt: Date.now(), startBytes: p.done_bytes } : {}),
      });
    });
  }

  of(name: string): ItemProgress | null {
    return this.progress[name] ?? null;
  }

  /** Bytes per second of a running item, once it has been running a while. */
  speed(name: string): number | null {
    const p = this.progress[name];
    if (!p || p.state !== "running" || p.startedAt === null) return null;
    const secs = (Date.now() - p.startedAt) / 1000;
    return secs >= 2 ? Math.max(0, p.done - p.startBytes) / secs : null;
  }

  /**
   * Downloads the items that are not installed, one after another, engine
   * first. Resolves to the number that failed. A failed engine stops the
   * rest (model pulls need it).
   */
  async install(items: DownloadItem[]): Promise<number> {
    this.#listen();
    if (this.busy) return 0;
    const todo = items
      .filter((i) => !i.installed && this.progress[i.name]?.state !== "done")
      .sort((a, b) => (a.kind === b.kind ? 0 : a.kind === "engine" ? -1 : 1));
    if (todo.length === 0) return 0;
    this.busy = true;
    for (const d of todo) this.#set(d.name, { state: "waiting", error: null, done: 0, total: d.size_bytes });
    let failed = 0;
    try {
      for (let i = 0; i < todo.length; i++) {
        const d = todo[i]!;
        this.#set(d.name, { state: "running", startedAt: null, startBytes: 0 });
        try {
          if (d.kind === "engine") await api.installEngine(d.name);
          else await api.pullModel(d.name);
          this.#set(d.name, { state: "done", done: d.size_bytes, total: d.size_bytes });
        } catch (e) {
          failed++;
          this.#set(d.name, { state: "failed", error: errorText(e) });
          void api.log("error", `download ${d.name} failed: ${errorText(e)}`);
          if (d.kind === "engine") {
            for (const rest of todo.slice(i + 1)) {
              failed++;
              this.#set(rest.name, { state: "failed", error: "Needs the engine, which did not install." });
            }
            break;
          }
        }
      }
    } finally {
      this.busy = false;
    }
    return failed;
  }
}

export const downloads = new Downloads();
