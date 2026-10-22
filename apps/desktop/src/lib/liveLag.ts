// The live latency statistic ("live latency (UI)" in the log) counts each
// utterance revision once: the echo gate sends a revision again when what it
// hides changes (at the first coupling, or when the far end's words for the
// same moment arrive later), and those re-sends are not new words, so their
// age is not latency (plan 15 section 9).

/** Which utterance revisions have been seen, per channel and utterance. */
export class RevisionsSeen {
  #latest = new Map<string, number>();

  /** True for a revision newer than every one seen for its utterance (then remembered). */
  fresh(channel: string, utt: number, rev: number): boolean {
    const key = `${channel}:${utt}`;
    const seen = this.#latest.get(key);
    if (seen !== undefined && rev <= seen) return false;
    this.#latest.set(key, rev);
    return true;
  }

  clear(): void {
    this.#latest.clear();
  }
}
