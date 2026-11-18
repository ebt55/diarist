// `bun test` (apps/desktop): keys in an open menu never reach the window's shortcuts.
import { describe, expect, test } from "bun:test";
import { menuKey } from "../src/lib/menuKeys";

const none = { ctrlKey: false, altKey: false, metaKey: false };
const ctrl = { ...none, ctrlKey: true };
const alt = { ...none, altKey: true };
// Open, Pin audio (no hint), a separator, Delete…
const listHints = ["Enter", undefined, undefined, "Delete"];
// Next line, Previous line, Rename…
const speakerHints = ["L", "H", "R"];

describe("menu keys", () => {
  test("navigation, activation and closing", () => {
    expect(menuKey("ArrowDown", none, listHints)).toEqual({ kind: "next" });
    expect(menuKey("ArrowUp", none, listHints)).toEqual({ kind: "prev" });
    expect(menuKey("Home", none, listHints)).toEqual({ kind: "first" });
    expect(menuKey("End", none, listHints)).toEqual({ kind: "last" });
    expect(menuKey("ArrowRight", none, listHints)).toEqual({ kind: "swatch", step: 1 });
    expect(menuKey("Enter", none, listHints)).toEqual({ kind: "activate" });
    expect(menuKey(" ", none, listHints)).toEqual({ kind: "activate" });
    expect(menuKey("Escape", none, listHints)).toEqual({ kind: "close" });
    expect(menuKey("Tab", none, listHints)).toEqual({ kind: "close" });
  });

  test("hints are accelerators, in any case", () => {
    expect(menuKey("Delete", none, listHints)).toEqual({ kind: "accelerator", index: 3 });
    expect(menuKey("l", none, speakerHints)).toEqual({ kind: "accelerator", index: 0 });
    expect(menuKey("R", none, speakerHints)).toEqual({ kind: "accelerator", index: 2 });
  });

  test("other keys are swallowed, so P, R, Delete and Space never leak", () => {
    const swallowed = { kind: "swallow", keepDefault: false };
    expect(menuKey("p", none, listHints)).toEqual(swallowed);
    expect(menuKey("r", none, listHints)).toEqual(swallowed);
    expect(menuKey("Delete", none, speakerHints)).toEqual(swallowed);
    expect(menuKey("F5", none, listHints)).toEqual(swallowed);
    expect(menuKey("?", none, listHints)).toEqual(swallowed);
  });

  test("chords never act as accelerators", () => {
    expect(menuKey("r", ctrl, speakerHints)).toEqual({ kind: "swallow", keepDefault: false });
    expect(menuKey("Delete", ctrl, listHints)).toEqual({ kind: "swallow", keepDefault: false });
    // Alt chords stay with the system (Alt+F4) but still not with the app.
    expect(menuKey("F4", alt, listHints)).toEqual({ kind: "swallow", keepDefault: true });
    expect(menuKey("l", alt, speakerHints)).toEqual({ kind: "swallow", keepDefault: true });
  });
});
