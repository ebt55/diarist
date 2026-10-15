<script lang="ts">
  import { tick, untrack } from "svelte";
  import { VList, type VListHandle } from "virtua/svelte";
  import { api, errorText } from "../lib/api";
  import { formatClock } from "../lib/format";
  import type { MeetingId } from "../lib/generated/MeetingId";
  import type { SegmentId } from "../lib/generated/SegmentId";
  import type { SegmentView } from "../lib/generated/SegmentView";
  import type { SpeakerId } from "../lib/generated/SpeakerId";
  import type { SpeakerView } from "../lib/generated/SpeakerView";
  import type { Word } from "../lib/generated/Word";
  import type { Player } from "../lib/player.svelte";
  import { app } from "../lib/state.svelte";
  import Menu, { type MenuItem } from "./Menu.svelte";

  interface Props {
    meetingId: MeetingId;
    segments: SegmentView[];
    roots: SpeakerView[];
    player: Player;
    /** Reload after an edit; the cursor then moves to `focus` if given. */
    onchanged: (focus?: SegmentId) => Promise<void>;
    onrename: (speaker: SpeakerId, anchor: SegmentId) => void;
  }
  let { meetingId, segments, roots, player, onchanged, onrename }: Props = $props();

  let vlist: VListHandle | undefined = $state();
  let cursor = $state(0);
  let flashId = $state<SegmentId | null>(null);
  let words = $state.raw(new Map<SegmentId, Word[]>());
  let menu = $state.raw<{ items: MenuItem[]; x: number; y: number; label: string } | null>(null);
  let follow = $state(true);
  let lastUserScroll = 0;
  let loadTimer: ReturnType<typeof setTimeout> | undefined;
  let loadSeq = 0;
  let pendingFocus: SegmentId | null = null;

  const cursorSeg = $derived(segments[cursor]);
  const long = $derived((segments[segments.length - 1]?.end_ms ?? 0) >= 3_600_000);

  /** Index of the segment under the playhead (binary search by start). */
  const activeIndex = $derived.by(() => {
    const t = player.currentMs;
    let lo = 0;
    let hi = segments.length - 1;
    let found = -1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if ((segments[mid]?.start_ms ?? 0) <= t) {
        found = mid;
        lo = mid + 1;
      } else hi = mid - 1;
    }
    const seg = segments[found];
    return seg && t < seg.end_ms + 300 ? found : -1;
  });

  const activeWord = $derived.by((): Word | null => {
    const seg = segments[activeIndex];
    const ws = seg ? words.get(seg.id) : undefined;
    if (!ws) return null;
    const t = player.currentMs;
    let hit: Word | null = null;
    for (const w of ws) {
      if (w.start_ms <= t) hit = w;
      else break;
    }
    return hit && t < hit.end_ms + 250 ? hit : null;
  });
  const activeWordId = $derived(activeWord?.id ?? null);

  // New segment list (open, edit): word ids may have moved; refetch visibly.
  $effect(() => {
    const list = segments;
    untrack(() => {
      words = new Map();
      if (cursor >= list.length) cursor = Math.max(0, list.length - 1);
      if (pendingFocus !== null) {
        const i = list.findIndex((s) => s.id === pendingFocus);
        pendingFocus = null;
        if (i >= 0) {
          cursor = i;
          void tick().then(() => vlist?.scrollToIndex(i, { align: "nearest" }));
        }
      }
      scheduleWords(0);
    });
  });

  // Follow the playhead unless the user scrolled in the last 3 s.
  $effect(() => {
    const i = activeIndex;
    if (i < 0 || !player.playing || !follow || !vlist) return;
    if (performance.now() - lastUserScroll < 3000) return;
    const top = vlist.getScrollOffset();
    const bottom = top + vlist.getViewportSize();
    const y = vlist.getItemOffset(i);
    if (y < top || y + vlist.getItemSize(i) > bottom) {
      vlist.scrollToIndex(i, { align: "center" });
    }
  });

  function scheduleWords(delay = 80) {
    clearTimeout(loadTimer);
    loadTimer = setTimeout(() => void loadVisibleWords(), delay);
  }

  async function loadVisibleWords() {
    if (!vlist || segments.length === 0) return;
    const off = vlist.getScrollOffset();
    const a = Math.max(0, vlist.findItemIndex(off) - 8);
    const b = Math.min(segments.length - 1, vlist.findItemIndex(off + vlist.getViewportSize()) + 8);
    const range = segments.slice(a, b + 1);
    if (range.every((s) => words.has(s.id))) return;
    const first = range[0];
    const last = range[range.length - 1];
    if (!first || !last) return;
    const seq = ++loadSeq;
    const list = segments;
    try {
      const fetched = await api.getWords(meetingId, first.start_ms, last.end_ms);
      if (seq !== loadSeq || list !== segments) return;
      const bySeg = new Map<SegmentId, Word[]>();
      for (const w of fetched) {
        const arr = bySeg.get(w.segment_id);
        if (arr) arr.push(w);
        else bySeg.set(w.segment_id, [w]);
      }
      const next = new Map(words);
      for (const s of range) next.set(s.id, bySeg.get(s.id) ?? []);
      words = next;
    } catch (e) {
      void api.log("warn", `get_words failed: ${errorText(e)}`);
    }
  }

  export function focus() {
    document.getElementById("transcript")?.focus();
  }

  /** Scrolls to a segment, puts the cursor on it and flashes it. */
  export function reveal(id: SegmentId) {
    const i = segments.findIndex((s) => s.id === id);
    if (i < 0) return;
    cursor = i;
    lastUserScroll = performance.now();
    vlist?.scrollToIndex(i, { align: "center" });
    flashId = id;
    setTimeout(() => {
      if (flashId === id) flashId = null;
    }, 1600);
    scheduleWords(30);
  }

  /** Splits the cursor segment before the word under the playhead (S). */
  export async function splitAtPlayhead() {
    const seg = segments[cursor];
    const w = activeWord;
    const ws = seg ? words.get(seg.id) : undefined;
    if (!seg || !w || w.segment_id !== seg.id || ws?.[0]?.id === w.id) {
      app.toast("info", "Put the playhead inside this segment, after its first word, to split it (S).");
      return;
    }
    await split(seg, w, null);
  }

  function moveCursor(i: number) {
    if (segments.length === 0) return;
    cursor = Math.max(0, Math.min(segments.length - 1, i));
    lastUserScroll = performance.now();
    vlist?.scrollToIndex(cursor, { align: "nearest" });
  }

  function seekTo(ms: number) {
    if (player.durationMs > 0 && ms > player.durationMs) {
      app.toast("info", `The audio ends at ${formatClock(player.durationMs)}; this point is past its end.`);
    }
    player.seek(ms);
  }

  async function reassign(seg: SegmentView, to: SpeakerView) {
    try {
      await api.reassignSegment(seg.id, to.id);
      await onchanged(seg.id);
    } catch (e) {
      app.error(`Reassign failed: ${errorText(e)}`);
    }
  }

  async function split(seg: SegmentView, at: Word, to: SpeakerView | null) {
    try {
      const created = await api.splitSegment(seg.id, at.id);
      if (to && to.id !== seg.speaker_id) await api.reassignSegment(created, to.id);
      await onchanged(created);
    } catch (e) {
      app.error(`Split failed: ${errorText(e)}`);
    }
  }

  function openMenu(seg: SegmentView, word: Word | null, x: number, y: number) {
    const ws = words.get(seg.id);
    const canSplit = word !== null && ws !== undefined && ws[0]?.id !== word.id;
    const items: MenuItem[] = [{ kind: "header", label: "Reassign segment to" }];
    roots.forEach((sp, k) =>
      items.push({
        kind: "item",
        label: sp.name,
        color: sp.color,
        hint: k < 9 ? String(k + 1) : undefined,
        disabled: sp.id === seg.speaker_id,
        run: () => void reassign(seg, sp),
      }),
    );
    if (canSplit && word) {
      items.push({ kind: "header", label: `Split before "${word.text}", rest to` });
      for (const sp of roots) {
        items.push({
          kind: "item",
          label: sp.id === seg.speaker_id ? `${sp.name} (split only)` : sp.name,
          color: sp.color,
          run: () => void split(seg, word, sp),
        });
      }
    }
    items.push(
      { kind: "separator" },
      { kind: "item", label: `Rename ${seg.speaker_name}…`, hint: "R", run: () => onrename(seg.speaker_id, seg.id) },
      {
        kind: "item",
        label: "Play from here",
        hint: "Enter",
        disabled: !player.ready,
        run: () => {
          seekTo(word?.start_ms ?? seg.start_ms);
          player.play();
        },
      },
      {
        kind: "item",
        label: "Copy text",
        run: () =>
          void navigator.clipboard
            .writeText(`[${formatClock(seg.start_ms, true)}] ${seg.speaker_name}: ${seg.text}`)
            .then(() => app.toast("info", "Copied segment text")),
      },
    );
    menu = { items, x, y, label: "Segment actions" };
  }

  function wordFromEvent(e: Event, seg: SegmentView): Word | null {
    const el = (e.target as HTMLElement).closest<HTMLElement>("[data-w]");
    const id = el ? Number(el.dataset.w) : NaN;
    return words.get(seg.id)?.find((w) => w.id === id) ?? null;
  }

  function onRowClick(e: MouseEvent, seg: SegmentView, i: number) {
    cursor = i;
    if (!window.getSelection()?.isCollapsed) return;
    const w = wordFromEvent(e, seg);
    if (w) seekTo(w.start_ms);
  }

  function onRowContext(e: MouseEvent, seg: SegmentView, i: number) {
    e.preventDefault();
    cursor = i;
    openMenu(seg, wordFromEvent(e, seg), e.clientX, e.clientY);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.ctrlKey || e.altKey || e.metaKey) return;
    const seg = segments[cursor];
    const k = e.key;
    if (k === "ArrowDown" || k === "j") moveCursor(cursor + 1);
    else if (k === "ArrowUp" || k === "k") moveCursor(cursor - 1);
    else if (k === "PageDown") moveCursor(cursor + 10);
    else if (k === "PageUp") moveCursor(cursor - 10);
    else if (k === "Home") moveCursor(0);
    else if (k === "End") moveCursor(segments.length - 1);
    else if (k === "Enter" && seg) {
      seekTo(seg.start_ms);
      player.play();
    } else if ((k === "r" || k === "R") && seg) onrename(seg.speaker_id, seg.id);
    else if ((k === "s" || k === "S") && seg) void splitAtPlayhead();
    else if (/^[1-9]$/.test(k) && seg) {
      const sp = roots[Number(k) - 1];
      if (sp && sp.id !== seg.speaker_id) void reassign(seg, sp);
    } else if ((k === "ContextMenu" || (k === "F10" && e.shiftKey)) && seg) {
      const row = document.getElementById(`seg-${seg.id}`)?.getBoundingClientRect();
      const w = activeWord && activeWord.segment_id === seg.id ? activeWord : null;
      openMenu(seg, w, (row?.left ?? 200) + 80, (row?.top ?? 200) + 20);
    } else if (k === "f" || k === "F") {
      follow = !follow;
      app.toast("info", follow ? "Transcript follows playback" : "Transcript stays put during playback");
    } else return;
    e.preventDefault();
  }

  // Exposed for MeetingView's "." (jump to playhead).
  export function revealPlayhead() {
    if (activeIndex >= 0) {
      lastUserScroll = 0;
      moveCursor(activeIndex);
    }
  }

  export function setPendingFocus(id: SegmentId) {
    pendingFocus = id;
  }
</script>

<div class="transcript-wrap">
  {#if segments.length === 0}
    <p class="empty">No transcript yet.</p>
  {:else}
    <VList
      id="transcript"
      class="transcript"
      role="listbox"
      tabindex={0}
      aria-label="Transcript"
      aria-activedescendant={cursorSeg ? `seg-${cursorSeg.id}` : undefined}
      data={segments}
      getKey={(s) => s.id}
      bind:this={vlist}
      onscroll={() => scheduleWords()}
      onwheel={() => (lastUserScroll = performance.now())}
      onfocusin={(e: FocusEvent) => {
        const list = e.currentTarget as HTMLElement;
        // Rows and timestamps are not tab stops; keep focus on the listbox.
        if (e.target !== list) list.focus({ preventScroll: true });
        else vlist?.scrollToIndex(cursor, { align: "nearest" });
      }}
      {onkeydown}
    >
      {#snippet children(seg: SegmentView, i: number)}
        {@const ws = words.get(seg.id)}
        <div
          id="seg-{seg.id}"
          class="seg"
          class:cursor={i === cursor}
          class:playing={i === activeIndex}
          class:flash={seg.id === flashId}
          style="--spk: {seg.speaker_color}"
          role="option"
          tabindex="-1"
          aria-selected={i === cursor}
          onclick={(e) => onRowClick(e, seg, i)}
          oncontextmenu={(e) => onRowContext(e, seg, i)}
          onkeydown={() => {}}
        >
          <button
            type="button"
            class="ts mono"
            tabindex="-1"
            title="Play from {formatClock(seg.start_ms, true)}"
            onclick={(e) => {
              e.stopPropagation();
              cursor = i;
              seekTo(seg.start_ms);
            }}>{formatClock(seg.start_ms, long)}</button
          >
          <div class="body">
            <span class="chip spk" class:review={seg.needs_review} title={seg.needs_review ? "New label: check who this is" : undefined}>{seg.speaker_name}</span>
            <span class="text"
              >{#if ws}{#each ws as w (w.id)}<span class="w" class:now={w.id === activeWordId} data-w={w.id}
                    >{w.text}</span
                  >{" "}{/each}{:else}{seg.text}{/if}</span
            >
          </div>
        </div>
      {/snippet}
    </VList>
  {/if}
</div>

{#if menu}
  <Menu items={menu.items} x={menu.x} y={menu.y} label={menu.label} onclose={() => (menu = null)} />
{/if}

<style>
  .transcript-wrap {
    position: relative;
    flex: 1;
    min-height: 0;
  }
  .transcript-wrap :global(.transcript) {
    padding: 6px 0 24px;
    outline: none;
  }
  .transcript-wrap :global(.transcript:focus-visible) {
    outline: none;
  }
  .empty {
    margin: 24px;
    color: var(--fg-muted);
  }
  .seg {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 10px;
    margin: 0 10px 0 8px;
    padding: 4px 8px 4px 6px;
    border-left: 3px solid var(--spk);
    border-radius: 0 4px 4px 0;
    line-height: 1.5;
  }
  .seg:hover {
    background: var(--bg-hover-soft);
  }
  .seg.playing {
    background: var(--bg-playing);
  }
  .seg.cursor {
    box-shadow: inset 0 0 0 1px var(--border-strong);
  }
  :global(.transcript:focus-visible) .seg.cursor {
    box-shadow: inset 0 0 0 2px var(--focus);
  }
  .seg.flash {
    animation: flash 1.6s ease-out;
  }
  @keyframes flash {
    0%,
    30% {
      background: var(--mark-bg);
    }
  }
  .ts {
    align-self: start;
    padding: 1px 2px;
    border: 0;
    background: none;
    color: var(--fg-muted);
    font-size: 12px;
    line-height: 21px;
    cursor: pointer;
  }
  .ts:hover {
    color: var(--accent);
    text-decoration: underline;
  }
  .body {
    min-width: 0;
  }
  .spk {
    margin-right: 6px;
    border-color: color-mix(in srgb, var(--spk) 55%, transparent);
    background: color-mix(in srgb, var(--spk) 16%, transparent);
    font-weight: 600;
    vertical-align: 1px;
  }
  .spk.review::after {
    content: "";
    display: inline-block;
    width: 6px;
    height: 6px;
    margin-left: 5px;
    border-radius: 50%;
    background: var(--warning-fg);
    vertical-align: 1px;
  }
  .text {
    overflow-wrap: anywhere;
  }
  .w {
    border-radius: 3px;
    cursor: pointer;
  }
  .w:hover {
    text-decoration: underline;
    text-decoration-color: var(--fg-faint);
  }
  .w.now {
    background: var(--word-now-bg);
    color: var(--word-now-fg);
    box-shadow: 0 0 0 1px var(--word-now-bg);
  }
</style>
