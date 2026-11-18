// What a key does in an open menu (`Menu.svelte`). Every key is the menu's
// while it is open: one it has no use for is swallowed, so P, R, Delete or
// Space never reach the window's shortcuts (pin, record, delete, play).

export type MenuKeyAction =
  | { kind: "next" }
  | { kind: "prev" }
  | { kind: "first" }
  | { kind: "last" }
  | { kind: "swatch"; step: 1 | -1 }
  | { kind: "activate" }
  | { kind: "close" }
  | { kind: "accelerator"; index: number }
  /** Kept from the window; `keepDefault` leaves system chords (Alt+F4) to the OS. */
  | { kind: "swallow"; keepDefault: boolean };

interface Mods {
  ctrlKey: boolean;
  altKey: boolean;
  metaKey: boolean;
}

/**
 * The action of `key`. `hints` holds, per item index, the hint of an
 * enabled item (digit and letter hints act as accelerators), else undefined.
 */
export function menuKey(key: string, mods: Mods, hints: readonly (string | undefined)[]): MenuKeyAction {
  switch (key) {
    case "ArrowDown":
      return { kind: "next" };
    case "ArrowUp":
      return { kind: "prev" };
    case "Home":
      return { kind: "first" };
    case "End":
      return { kind: "last" };
    case "ArrowRight":
      return { kind: "swatch", step: 1 };
    case "ArrowLeft":
      return { kind: "swatch", step: -1 };
    case "Enter":
    case " ":
      return { kind: "activate" };
    case "Escape":
    case "Tab":
      return { kind: "close" };
  }
  if (mods.altKey && !mods.ctrlKey) return { kind: "swallow", keepDefault: true };
  // Ctrl chords are swallowed whole: Ctrl+R must not reload the window either.
  if (mods.ctrlKey || mods.metaKey) return { kind: "swallow", keepDefault: false };
  const k = key.toLowerCase();
  const index = hints.findIndex((h) => h !== undefined && h.toLowerCase() === k);
  return index >= 0 ? { kind: "accelerator", index } : { kind: "swallow", keepDefault: false };
}
