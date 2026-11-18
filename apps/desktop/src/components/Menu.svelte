<script lang="ts" module>
  export type MenuItem =
    | { kind: "header"; label: string }
    | { kind: "separator" }
    | {
        kind: "item";
        label: string;
        hint?: string;
        color?: string;
        disabled?: boolean;
        danger?: boolean;
        run: () => void;
      }
    | {
        kind: "swatches";
        label: string;
        colors: readonly string[];
        current?: string;
        pick: (color: string) => void;
      };

  let menuSeq = 0;
</script>

<script lang="ts">
  import { onMount, tick } from "svelte";
  import { menuKey } from "../lib/menuKeys";

  interface Props {
    items: MenuItem[];
    x: number;
    y: number;
    label: string;
    onclose: () => void;
  }

  let { items, x, y, label, onclose }: Props = $props();

  const uid = `menu-${++menuSeq}`;
  let el: HTMLDivElement | undefined = $state();
  let left = $state(0);
  let top = $state(0);
  let active = $state(0);
  let swatch = $state(0);
  let returnFocus: HTMLElement | null = null;

  const stops = $derived(
    items.flatMap((it, i) =>
      (it.kind === "item" && !it.disabled) || it.kind === "swatches" ? [i] : [],
    ),
  );
  const activeItem = $derived(items[stops[active] ?? -1]);

  onMount(() => {
    returnFocus = document.activeElement as HTMLElement | null;
    left = x;
    top = y;
    void tick().then(() => {
      if (!el) return;
      const r = el.getBoundingClientRect();
      left = Math.max(4, Math.min(x, window.innerWidth - r.width - 4));
      top = Math.max(4, Math.min(y, window.innerHeight - r.height - 4));
      el.focus();
    });
    const outside = (e: PointerEvent) => {
      if (el && !el.contains(e.target as Node)) close(false);
    };
    // A key while the focus is not in the menu yet (or no longer) is the menu's too.
    const strayKey = (e: KeyboardEvent) => {
      if (el && !el.contains(e.target as Node)) onkeydown(e);
    };
    window.addEventListener("pointerdown", outside, true);
    window.addEventListener("keydown", strayKey, true);
    return () => {
      window.removeEventListener("pointerdown", outside, true);
      window.removeEventListener("keydown", strayKey, true);
    };
  });

  function close(restore = true) {
    onclose();
    if (restore) returnFocus?.focus();
  }

  function activate(i: number) {
    const it = items[i];
    if (!it) return;
    if (it.kind === "item" && !it.disabled) {
      close();
      it.run();
    }
  }

  function pickSwatch(it: Extract<MenuItem, { kind: "swatches" }>, color: string) {
    close();
    it.pick(color);
  }

  /** Every key stays in the open menu (`menuKey`): none reaches the window's shortcuts. */
  function onkeydown(e: KeyboardEvent) {
    const n = stops.length;
    const it = activeItem;
    // Hints ("1".. on reassign and export items, "R", "L") act as accelerators.
    const hints = items.map((x) => (x.kind === "item" && !x.disabled ? x.hint : undefined));
    const action = menuKey(e.key, e, hints);
    e.stopPropagation();
    if (action.kind !== "swallow" || !action.keepDefault) e.preventDefault();
    switch (action.kind) {
      case "next":
        active = (active + 1) % Math.max(1, n);
        break;
      case "prev":
        active = (active - 1 + n) % Math.max(1, n);
        break;
      case "first":
        active = 0;
        break;
      case "last":
        active = n - 1;
        break;
      case "swatch":
        if (it?.kind === "swatches") swatch = (swatch + action.step + it.colors.length) % it.colors.length;
        break;
      case "activate":
        if (it?.kind === "swatches") {
          const c = it.colors[swatch];
          if (c) pickSwatch(it, c);
        } else {
          activate(stops[active] ?? -1);
        }
        break;
      case "close":
        close();
        break;
      case "accelerator":
        activate(action.index);
        break;
      case "swallow":
        break;
    }
  }
</script>

<div
  bind:this={el}
  class="menu"
  role="menu"
  aria-label={label}
  tabindex="-1"
  aria-activedescendant={stops[active] !== undefined ? `${uid}-${stops[active]}` : undefined}
  style="left: {left}px; top: {top}px"
  {onkeydown}
>
  {#each items as it, i (i)}
    {#if it.kind === "header"}
      <div class="menu-header" role="presentation">{it.label}</div>
    {:else if it.kind === "separator"}
      <div class="menu-sep" role="separator"></div>
    {:else if it.kind === "item"}
      <div
        id="{uid}-{i}"
        class="menu-item"
        class:active={stops[active] === i}
        class:danger={it.danger}
        role="menuitem"
        tabindex="-1"
        aria-disabled={it.disabled ? "true" : undefined}
        onpointerenter={() => {
          const s = stops.indexOf(i);
          if (s >= 0) active = s;
        }}
        onclick={() => activate(i)}
        onkeydown={() => {}}
      >
        {#if it.color}<span class="dot" style="background: {it.color}"></span>{/if}
        <span class="menu-label">{it.label}</span>
        {#if it.hint}<kbd>{it.hint}</kbd>{/if}
      </div>
    {:else}
      <div id="{uid}-{i}" class="menu-swatches" class:active={stops[active] === i} role="group" aria-label={it.label}>
        {#each it.colors as c, k (c)}
          <button
            type="button"
            tabindex="-1"
            class="swatch"
            class:current={c.toLowerCase() === it.current?.toLowerCase()}
            class:kbd={stops[active] === i && swatch === k}
            style="background: {c}"
            aria-label="Colour {c}"
            onclick={() => pickSwatch(it, c)}
          ></button>
        {/each}
      </div>
    {/if}
  {/each}
</div>

<style>
  .menu {
    position: fixed;
    z-index: 50;
    min-width: 160px;
    max-width: 340px;
    max-height: min(70vh, 520px);
    overflow-y: auto;
    padding: 4px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg-raised);
    box-shadow: var(--shadow);
    outline: none;
  }
  .menu-header {
    padding: 6px 8px 2px;
    color: var(--fg-muted);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.03em;
    text-transform: uppercase;
  }
  .menu-sep {
    height: 1px;
    margin: 4px 2px;
    background: var(--border);
  }
  .menu-item {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 5px 8px;
    border-radius: 4px;
    cursor: default;
  }
  .menu-item.active {
    background: var(--bg-selected);
  }
  .menu-item[aria-disabled="true"] {
    color: var(--fg-faint);
  }
  .menu-item.danger {
    color: var(--danger);
  }
  .menu-label {
    flex: 1;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .dot {
    flex: none;
    width: 10px;
    height: 10px;
    border-radius: 50%;
  }
  .menu-swatches {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 6px 8px;
    border-radius: 4px;
  }
  .menu-swatches.active {
    background: var(--bg-hover);
  }
  .swatch {
    width: 20px;
    height: 20px;
    padding: 0;
    border: 2px solid transparent;
    border-radius: 50%;
    cursor: default;
  }
  .swatch.current {
    border-color: var(--fg);
  }
  .swatch.kbd,
  .swatch:hover {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
</style>
