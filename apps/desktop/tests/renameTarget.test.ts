// `bun test` (apps/desktop): which live line R renames while recording.
import { describe, expect, test } from "bun:test";
import { renameTarget, UNKNOWN_LABEL } from "../src/lib/renameTarget";

interface Row {
  key: string;
  label: string;
  speakerId: number | null;
  final: boolean;
}

const row = (key: string, label: string, speakerId: number | null, final: boolean): Row => ({ key, label, speakerId, final });
const isFinal = (r: Row) => r.final;

describe("R on the live transcript", () => {
  const rows = [
    row("a", "spk_0", 1, true),
    row("b", "spk_1", 2, true),
    row("c", "spk_1", 2, false),
    row("d", UNKNOWN_LABEL, 3, false),
  ];

  test("a selected line is renamed, provisional or not", () => {
    expect(renameTarget(rows, 0, isFinal)?.key).toBe("a");
    expect(renameTarget(rows, 3, isFinal)?.key).toBe("d");
  });

  test("without a selection: the newest final line of a known speaker", () => {
    expect(renameTarget(rows, -1, isFinal)?.key).toBe("b");
    // A final row of the unknown label or without a speaker row is skipped too.
    const more = [...rows, row("e", UNKNOWN_LABEL, 3, true), row("f", "spk_2", null, true)];
    expect(renameTarget(more, -1, isFinal)?.key).toBe("b");
  });

  test("nothing final yet: no target", () => {
    expect(renameTarget([row("x", UNKNOWN_LABEL, 3, false), row("y", "spk_0", 1, false)], -1, isFinal)).toBeNull();
    expect(renameTarget([], -1, isFinal)).toBeNull();
    // A cursor past the end is no selection.
    expect(renameTarget([row("x", UNKNOWN_LABEL, 3, false)], 5, isFinal)).toBeNull();
  });
});
