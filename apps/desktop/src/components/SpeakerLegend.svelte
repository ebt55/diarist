<script lang="ts">
  import { formatClock } from "../lib/format";
  import type { SpeakerId } from "../lib/generated/SpeakerId";
  import type { SpeakerView } from "../lib/generated/SpeakerView";
  import Icon from "./Icon.svelte";

  interface Props {
    speakers: SpeakerView[];
    /** Speaker whose name is being edited. */
    renaming: SpeakerId | null;
    oncommit: (name: string) => void;
    oncancel: () => void;
    onstartrename: (id: SpeakerId) => void;
    onmenu: (speaker: SpeakerView, x: number, y: number) => void;
    onunmerge: (speaker: SpeakerView) => void;
  }
  let { speakers, renaming, oncommit, oncancel, onstartrename, onmenu, onunmerge }: Props = $props();

  const roots = $derived(speakers.filter((s) => s.merged_into === null));
  const merged = $derived(speakers.filter((s) => s.merged_into !== null));
  const nameOf = (id: SpeakerId | null) => speakers.find((s) => s.id === id)?.name ?? "?";

  let finished = false;

  function chipTitle(sp: SpeakerView, i: number): string {
    const key = i < 9 ? ` Key ${i + 1} on a segment gives it to this speaker.` : "";
    return `${sp.name}: double-click or R renames, Enter opens the menu.${key}`;
  }

  function editor(node: HTMLInputElement) {
    finished = false;
    node.focus();
    node.select();
  }

  function finish(value: string | null) {
    if (finished) return;
    finished = true;
    if (value === null) oncancel();
    else oncommit(value);
  }

  function menuAt(sp: SpeakerView, el: HTMLElement) {
    const r = el.getBoundingClientRect();
    onmenu(sp, r.left, r.bottom + 4);
  }

  function chipKeys(e: KeyboardEvent, sp: SpeakerView, i: number) {
    const chips = Array.from(
      (e.currentTarget as HTMLElement).parentElement?.parentElement?.querySelectorAll<HTMLElement>(".spk-chip") ?? [],
    );
    if (e.key === "r" || e.key === "R" || e.key === "F2") onstartrename(sp.id);
    else if (e.key === "Enter" || e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey))
      menuAt(sp, e.currentTarget as HTMLElement);
    else if (e.key === "ArrowRight") chips[Math.min(chips.length - 1, i + 1)]?.focus();
    else if (e.key === "ArrowLeft") chips[Math.max(0, i - 1)]?.focus();
    else return;
    e.preventDefault();
    e.stopPropagation();
  }
</script>

<div class="legend" role="toolbar" aria-label="Speakers (R renames, Enter opens the menu)">
  {#each roots as sp, i (sp.id)}
    <div class="item" style="--spk: {sp.color}">
      {#if renaming === sp.id}
        <span class="dot"></span>
        <input
          class="rename"
          aria-label="Name for {sp.name}"
          value={sp.display_name ?? ""}
          placeholder={sp.name}
          use:editor
          onkeydown={(e) => {
            if (e.key === "Enter") finish(e.currentTarget.value);
            else if (e.key === "Escape") finish(null);
            else return;
            e.preventDefault();
            e.stopPropagation();
          }}
          onblur={(e) => finish(e.currentTarget.value)}
        />
      {:else}
        <button
          type="button"
          class="spk-chip"
          title={chipTitle(sp, i)}
          ondblclick={() => onstartrename(sp.id)}
          oncontextmenu={(e) => {
            e.preventDefault();
            menuAt(sp, e.currentTarget);
          }}
          onkeydown={(e) => chipKeys(e, sp, i)}
        >
          <span class="dot"></span>
          <span class="name">{sp.name}</span>
          <span class="stat mono">{formatClock(sp.talk_ms)}</span>
          {#if sp.needs_review}<span class="chip badge-new">new label</span>{/if}
        </button>
        <button
          type="button"
          class="more"
          tabindex="-1"
          aria-label="Actions for {sp.name}"
          onclick={(e) => menuAt(sp, e.currentTarget)}
        >
          <Icon name="more" size={14} />
        </button>
      {/if}
    </div>
  {/each}
  {#if merged.length > 0}
    <div class="merged">
      {#each merged as sp (sp.id)}
        <span class="merged-item" style="--spk: {sp.color}">
          <span class="dot"></span>{sp.name} → {nameOf(sp.merged_into)}
          <button type="button" class="link" onclick={() => onunmerge(sp)}>Unmerge</button>
        </span>
      {/each}
    </div>
  {/if}
</div>

<style>
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 6px;
    align-items: center;
    padding: 0 14px 8px;
  }
  .item {
    display: inline-flex;
    align-items: center;
    border: 1px solid var(--border);
    border-left: 3px solid var(--spk);
    border-radius: var(--radius);
    background: var(--bg);
  }
  .spk-chip {
    display: inline-flex;
    gap: 5px;
    align-items: center;
    padding: 2px 4px 2px 7px;
    border: 0;
    border-radius: var(--radius) 0 0 var(--radius);
    background: none;
    color: inherit;
    font: inherit;
    cursor: default;
  }
  .spk-chip:focus-visible,
  .more:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: -2px;
  }
  .dot {
    flex: none;
    width: 10px;
    height: 10px;
    margin-left: 0;
    border-radius: 50%;
    background: var(--spk);
  }
  .item > .dot {
    margin-left: 8px;
  }
  .name {
    font-weight: 600;
  }
  .stat {
    color: var(--fg-muted);
    font-size: 11px;
  }
  .more {
    display: inline-flex;
    align-items: center;
    align-self: stretch;
    padding: 0 4px;
    border: 0;
    border-left: 1px solid var(--border);
    border-radius: 0 var(--radius) var(--radius) 0;
    background: none;
    color: var(--fg-muted);
    cursor: default;
  }
  .more:hover {
    background: var(--bg-hover);
    color: var(--fg);
  }
  .rename {
    width: 160px;
    margin: 2px 4px 2px 6px;
    padding: 2px 6px;
  }
  .merged {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    margin-left: 4px;
    color: var(--fg-muted);
    font-size: 12px;
  }
  .merged-item {
    display: inline-flex;
    gap: 4px;
    align-items: center;
  }
</style>
