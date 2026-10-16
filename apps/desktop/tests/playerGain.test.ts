// `bun test` (apps/desktop): the player's normalisation gain, its cap, the
// robust peak and the volume setting.
import { describe, expect, test } from "bun:test";
import {
  LIMITER,
  MAX_GAIN_DB,
  dbfs,
  earEnvelope,
  fromDb,
  limiterMakeupDb,
  normalizeGainDb,
  parseVolume,
  percentile,
  playbackGain,
  softClipCurve,
} from "../src/lib/playerGain";

/** A u8 bucket peak (as core's waveform stores it) for a level in dBFS. */
const u8 = (db: number) => Math.round(fromDb(db) * 255);
/** `n` buckets at `db`. */
const flat = (n: number, db: number) => Array.from({ length: n }, () => u8(db));

describe("normalizeGainDb", () => {
  test("brings the peak to -1 dBFS", () => {
    expect(normalizeGainDb(-13)).toBeCloseTo(12, 9);
    expect(normalizeGainDb(-1)).toBeCloseTo(0, 9);
    // A track that already peaks above -1 dBFS comes down to it.
    expect(normalizeGainDb(0)).toBeCloseTo(-1, 9);
    // The linear gain is 10^((-1 - peak) / 20).
    expect(fromDb(normalizeGainDb(-21))).toBeCloseTo(10 ** (20 / 20), 9);
  });

  test("is capped at +24 dB", () => {
    expect(MAX_GAIN_DB).toBe(24);
    expect(normalizeGainDb(-25)).toBeCloseTo(24, 9);
    expect(normalizeGainDb(-40)).toBe(24);
    expect(normalizeGainDb(-100)).toBe(24);
  });

  test("leaves silence alone", () => {
    expect(normalizeGainDb(dbfs(0))).toBe(0);
    expect(normalizeGainDb(Number.NaN)).toBe(0);
  });
});

describe("the track's peak", () => {
  test("percentile ignores silent buckets and the top 1 %", () => {
    const values = [...Array.from({ length: 99 }, () => 0.1), 0.9, 0, 0, 0];
    expect(percentile(values, 0.99)).toBeCloseTo(0.1, 9);
    expect(percentile(values, 1)).toBeCloseTo(0.9, 9);
    expect(percentile([0, 0], 0.99)).toBe(0);
    expect(percentile([], 0.99)).toBe(0);
  });

  test("the mix halves each channel in each ear; a mono track plays as left = right", () => {
    const stereo = [[0, 100, 200], [255, 100, 0]];
    expect(earEnvelope(stereo, "mix").map((v) => Math.round(v * 255))).toEqual([128, 100, 100]);
    expect(earEnvelope(stereo, "left").map((v) => Math.round(v * 255))).toEqual([0, 100, 200]);
    expect(earEnvelope(stereo, "right").map((v) => Math.round(v * 255))).toEqual([255, 100, 0]);
    const mono = [[10, 20]];
    for (const mode of ["mix", "left", "right"] as const) {
      expect(earEnvelope(mono, mode).map((v) => Math.round(v * 255))).toEqual([10, 20]);
    }
  });
});

describe("playbackGain", () => {
  // The measured live recording: mic (left) silent, loopback (right) held at
  // about -20 dBFS by the capture AGC, one burst at -1 dBFS in the first second.
  const live = (() => {
    const right = flat(1600, -18);
    right[1] = u8(-1);
    return [Array.from({ length: 1600 }, () => 0), right];
  })();

  test("a quiet live recording is raised by its loud speech, not by its one burst", () => {
    const g = playbackGain(live, "mix", true, 1);
    // The mix is -24 dBFS (-18 halved): +23 dB, under the cap.
    expect(g.normalizeDb).toBeCloseTo(23, 0);
    expect(g.limit).toBe(true);
    // The compressor's own makeup gain is divided out.
    expect(dbfs(g.gain)).toBeCloseTo(g.normalizeDb - limiterMakeupDb(LIMITER.thresholdDb, LIMITER.ratio), 6);
    // Solo on the system channel: the full -18 dBFS, so 6 dB less gain.
    expect(playbackGain(live, "right", true, 1).normalizeDb).toBeCloseTo(17, 0);
    // The silent mic gets no gain at all.
    expect(playbackGain(live, "left", true, 1)).toEqual({ gain: 1, normalizeDb: 0, limit: false });
  });

  test("the cap holds for a near-silent track", () => {
    // -40 dBFS is 3 of 255 in the waveform; anything quieter reads as silence.
    const g = playbackGain([flat(100, -40)], "mix", true, 1);
    expect(g.normalizeDb).toBe(24);
  });

  test("the volume multiplies the normalisation", () => {
    const full = playbackGain([flat(100, -13)], "mix", true, 1);
    const half = playbackGain([flat(100, -13)], "mix", true, 0.5);
    expect(full.normalizeDb).toBeCloseTo(12, 0);
    expect(half.normalizeDb).toBe(full.normalizeDb);
    // Every bucket is at the peak, so nothing passes -1 dBFS: no limiter.
    expect(full.limit).toBe(false);
    expect(half.limit).toBe(false);
    expect(half.gain).toBeCloseTo(full.gain * 0.5, 9);
    expect(playbackGain([flat(100, -13)], "mix", true, 0).gain).toBe(0);
  });

  test("off, or without peaks, the track plays as stored times the volume", () => {
    expect(playbackGain(live, "mix", false, 1)).toEqual({ gain: 1, normalizeDb: 0, limit: false });
    expect(playbackGain(live, "mix", false, 0.4)).toEqual({ gain: 0.4, normalizeDb: 0, limit: false });
    expect(playbackGain(null, "mix", true, 0.8)).toEqual({ gain: 0.8, normalizeDb: 0, limit: false });
    expect(playbackGain([], "mix", true, 1)).toEqual({ gain: 1, normalizeDb: 0, limit: false });
  });

  test("a loud track is not raised and needs no limiter", () => {
    const g = playbackGain([flat(100, -0.5)], "mix", true, 1);
    expect(g.normalizeDb).toBeCloseTo(-0.5, 1);
    expect(g.limit).toBe(false);
    expect(g.gain).toBeLessThan(1);
  });
});

describe("softClipCurve", () => {
  test("passes the audio up to -1 dBFS and never reaches full scale", () => {
    const c = softClipCurve(4097);
    const n = c.length;
    const knee = fromDb(-1);
    let unchanged = 0;
    for (let i = 0; i < n; i++) {
      const x = (i / (n - 1)) * 2 - 1;
      const y = c[i] ?? Number.NaN;
      if (Math.abs(x) <= knee) {
        expect(Math.abs(y - x)).toBeLessThan(1e-6);
        unchanged++;
      }
      expect(Math.abs(y)).toBeLessThan(1);
      if (i > 0) expect(y).toBeGreaterThanOrEqual(c[i - 1] ?? Number.NaN);
      expect(y).toBeCloseTo(-(c[n - 1 - i] ?? Number.NaN), 6);
    }
    expect(unchanged).toBeGreaterThan(0.88 * n);
    expect(c[n - 1]).toBeGreaterThan(knee);
  });
});

describe("playback.volume", () => {
  test("reads 0..1 and falls back to 1", () => {
    expect(parseVolume("1.0")).toBe(1);
    expect(parseVolume(" 0.35 ")).toBeCloseTo(0.35, 9);
    expect(parseVolume("0")).toBe(0);
    expect(parseVolume("3")).toBe(1);
    expect(parseVolume("-1")).toBe(0);
    expect(parseVolume("")).toBe(1);
    expect(parseVolume("loud")).toBe(1);
    expect(parseVolume(undefined)).toBe(1);
  });
});
