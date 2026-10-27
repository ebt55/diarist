// `bun test` (apps/desktop): when "Install and restart" waits, and what it says.
import { describe, expect, test } from "bun:test";
import { updateBlocker, type JobLike } from "../src/lib/updateBlocker";

const imp = (name: string, stage = "transcribing"): JobLike => ({ kind: "import", name, stage });
const post = (name: string, stage = "diarizing"): JobLike => ({ kind: "postpass", name, stage });

describe("updateBlocker", () => {
  test("nothing running: the update can start", () => {
    expect(updateBlocker(false, [])).toBeNull();
  });

  test("a recording comes first", () => {
    expect(updateBlocker(true, [imp("a.m4a")])).toBe("Stop the recording before updating.");
  });

  test("names the running import", () => {
    const why = updateBlocker(false, [imp("call.m4a · German")]);
    expect(why).toStartWith("The import of call.m4a · German is still running");
    expect(why).toContain("restarts Diarist");
    expect(why).toContain("cancel it");
  });

  test("names a running post-pass and counts the queue", () => {
    const why = updateBlocker(false, [imp("b.wav", "queued"), post("Standup"), imp("c.wav", "queued")]);
    expect(why).toStartWith('Finalizing "Standup" is still running and 2 more jobs wait in the queue');
    expect(why).toContain("cancel them");
    expect(updateBlocker(false, [post("Standup"), imp("b.wav", "queued")])).toContain("1 more job waits");
  });

  test("names a punctuation job like the backend", () => {
    const punct: JobLike = { kind: "punctuate", name: "Adding punctuation: Standup", stage: "saving" };
    expect(updateBlocker(false, [punct])).toStartWith("A punctuation job is still running");
  });

  test("only queued jobs (the queue waits between jobs)", () => {
    expect(updateBlocker(false, [imp("b.wav", "queued")])).toStartWith("The import of b.wav waits in the queue");
    expect(updateBlocker(false, [imp("b.wav", "queued"), imp("c.wav", "queued")])).toStartWith(
      "2 jobs wait in the queue",
    );
  });
});
