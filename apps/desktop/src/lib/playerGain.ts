// Playback level: the gain that evens out a quiet recording, from the
// waveform peaks the player already has (core `audio::peaks`: per bucket the
// largest absolute sample, 0..=255), so opening a meeting scans nothing new.
// Pure functions, tested by `bun test` without Web Audio.

/** What each ear hears: both channels as a mono mix, or one channel solo. */
export type ChannelMode = "mix" | "left" | "right";

/** Weight of the left and the right channel in each ear (the player's gain matrix). */
export const EAR_WEIGHTS: Record<ChannelMode, readonly [number, number]> = {
  mix: [0.5, 0.5],
  left: [1, 0],
  right: [0, 1],
};

/** Where normalisation puts the track's peak. */
export const TARGET_DBFS = -1;
/** The most normalisation raises a track. */
export const MAX_GAIN_DB = 24;
/**
 * The bucket peak that counts as the track's peak: the 99th percentile of
 * the buckets with sound. A short burst (the capture AGC settling in the
 * first second of a recording, a click) would otherwise set the gain for
 * the whole meeting; the limiter catches the buckets above it.
 */
export const PEAK_PERCENTILE = 0.99;
/**
 * The safety limiter after the gain (a DynamicsCompressorNode), then a soft
 * clip from -1 dBFS for the few samples its attack lets through.
 */
export const LIMITER = { thresholdDb: -3, kneeDb: 0, ratio: 20, attackS: 0, releaseS: 0.2, clipFromDb: -1 } as const;
/** Volume steps of the player's slider. */
export const VOLUME_STEP = 0.05;

export function dbfs(linear: number): number {
  return linear > 0 ? 20 * Math.log10(linear) : Number.NEGATIVE_INFINITY;
}

export function fromDb(db: number): number {
  return 10 ** (db / 20);
}

/**
 * The gain in dB that brings a peak of `peakDbfs` to -1 dBFS
 * (10^((-1 - peak) / 20) as a linear gain), at most +24 dB; 0 dB when
 * nothing was measured (silence or no peaks).
 */
export function normalizeGainDb(peakDbfs: number): number {
  if (!Number.isFinite(peakDbfs)) return 0;
  return Math.min(MAX_GAIN_DB, TARGET_DBFS - peakDbfs);
}

/**
 * Per bucket, the most either ear can reach in `mode` (0..1). For the mix
 * it is an upper bound: two channels' bucket peaks need not coincide. A
 * mono track plays as left = right.
 */
export function earEnvelope(peaks: readonly (readonly number[])[], mode: ChannelMode): number[] {
  const left = peaks[0] ?? [];
  const right = peaks[1] ?? left;
  const [wl, wr] = EAR_WEIGHTS[mode];
  const n = Math.max(left.length, right.length);
  return Array.from({ length: n }, (_, i) => (wl * (left[i] ?? 0) + wr * (right[i] ?? 0)) / 255);
}

/** The `q` quantile (nearest rank) of the values above zero; 0 when there are none. */
export function percentile(values: readonly number[], q: number): number {
  const v = values.filter((x) => x > 0).sort((a, b) => a - b);
  if (v.length === 0) return 0;
  const i = Math.min(v.length - 1, Math.max(0, Math.ceil(q * v.length) - 1));
  return v[i] ?? 0;
}

/**
 * The gain Web Audio's compressor adds on its own: (1 / curve(0 dBFS))^0.6
 * (Web Audio spec, DynamicsCompressorNode, "makeup gain"). The player
 * divides it out so the limiter only ever lowers the loud parts.
 */
export function limiterMakeupDb(thresholdDb: number, ratio: number): number {
  return -0.6 * thresholdDb * (1 - 1 / ratio);
}

/**
 * WaveShaper curve over -1..1: unchanged up to `fromDb` in level, then a
 * tanh bend that approaches full scale and never reaches it.
 */
export function softClipCurve(points = 4097, fromDbfs: number = LIMITER.clipFromDb): Float32Array<ArrayBuffer> {
  const knee = fromDb(fromDbfs);
  const curve = new Float32Array(points);
  for (let i = 0; i < points; i++) {
    const x = (i / (points - 1)) * 2 - 1;
    const a = Math.abs(x);
    const y = a <= knee ? a : knee + (1 - knee) * Math.tanh((a - knee) / (1 - knee));
    curve[i] = Math.sign(x) * y;
  }
  return curve;
}

/** `playback.volume`: 0..1, the default 1 for a blank or unreadable value. */
export function parseVolume(value: string | undefined): number {
  const s = value?.trim() ?? "";
  const v = s === "" ? Number.NaN : Number(s);
  return clampVolume(v);
}

export function clampVolume(v: number): number {
  return Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : 1;
}

export interface PlaybackGain {
  /** Linear gain before the limiter. */
  gain: number;
  /** The normalisation alone (0 when off or nothing was measured). */
  normalizeDb: number;
  /** The limiter works: the player raises the audio and the loudest bucket would pass -1 dBFS. */
  limit: boolean;
}

/**
 * The gain for `mode`: the normalisation (when `normalize` and the peaks are
 * known) times `volume`. Below unity the audio cannot get louder than it is
 * stored, so the limiter stays out (ratio 1 passes it unchanged).
 */
export function playbackGain(
  peaks: readonly (readonly number[])[] | null,
  mode: ChannelMode,
  normalize: boolean,
  volume: number,
): PlaybackGain {
  const env = peaks && peaks.length > 0 ? earEnvelope(peaks, mode) : [];
  const normalizeDb = normalize ? normalizeGainDb(dbfs(percentile(env, PEAK_PERCENTILE))) : 0;
  const total = clampVolume(volume) * fromDb(normalizeDb);
  const loudest = env.reduce((m, v) => Math.max(m, v), 0);
  // 0.01 dB of slack: a track whose loudest bucket is its peak lands on -1 dBFS exactly.
  const limit = total > 1 && dbfs(total * loudest) > TARGET_DBFS + 0.01;
  const gain = limit ? total / fromDb(limiterMakeupDb(LIMITER.thresholdDb, LIMITER.ratio)) : total;
  return { gain, normalizeDb, limit };
}
