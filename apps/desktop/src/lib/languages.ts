// Languages for recordings and imports (docs/04 section 6). The table is
// data: the backend's `language_table` (manifests/languages.json plus the
// data root's languages.user.json), with the generated copy of the shipped
// table as the fallback. The lists offer every language an installed model
// declares (named from the table, else by the platform's language names,
// else by the tag), plus a typed tag. The decision mirrors the backend's
// `models::select_for_language`, so a sheet shows what the job will do: run
// on the selected model, switch to an installed model that lists the
// language, offer the Nemotron 3.5 download for a known language nothing
// installed lists, or pass an unknown tag through unchecked. The backend
// checks again when the job starts.
import type { LanguageEntry } from "./generated/LanguageEntry";
import type { LanguageTier } from "./generated/LanguageTier";
import type { ModelFile } from "./generated/ModelFile";
import { LANGUAGE_TIERS, MULTILINGUAL_MODEL, TIER_EXPECTATIONS, TIER_TITLES } from "./generated/languageTiers";

export { LANGUAGE_TIERS, MULTILINGUAL_MODEL, TIER_EXPECTATIONS, TIER_TITLES, type LanguageEntry, type LanguageTier };

/** A language table: the backend's effective one, or the shipped fallback. */
export type LanguageTable = readonly LanguageEntry[];

/** Automatic detection; never the default. */
export const AUTO = "auto";
/** The "Other languages: install Nemotron 3.5" entry (not a language). */
export const OTHER = "other";
/** The "Other tag…" entry: the user types a BCP-47 tag (not a language). */
export const TYPED = "typed";
export const AUTO_NOTE = "Automatic detection loses about a second of the first utterance.";

export function primarySubtag(tag: string): string {
  return (tag.trim().split(/[-_]/)[0] ?? "").toLowerCase();
}

export function isAuto(tag: string): boolean {
  return tag.trim().toLowerCase() === AUTO;
}

/** A plausible BCP-47 tag (`sw`, `sw-KE`, `zh-Hant-TW`, `x-private`). */
export function isTag(tag: string): boolean {
  return /^([A-Za-z]{2,3}|x)([-_][A-Za-z0-9]{1,8})*$/.test(tag.trim());
}

/** The entry `tag` stands for: its own, or for a bare subtag the one marked `bare` (`ja` is ja-JP). */
export function findLanguage(tag: string, table: LanguageTable = LANGUAGE_TIERS): LanguageEntry | null {
  const t = tag.trim().toLowerCase();
  const exact = table.find((l) => l.code.toLowerCase() === t);
  if (exact) return exact;
  if (/[-_]/.test(t)) return null;
  return table.find((l) => l.bare && primarySubtag(l.code) === t) ?? null;
}

/** Whether the table has the tag's language (any locale of it), or it is `auto` (`LanguageTable::knows`). */
export function knows(tag: string, table: LanguageTable = LANGUAGE_TIERS): boolean {
  const p = primarySubtag(tag);
  return isAuto(tag) || table.some((l) => primarySubtag(l.code) === p);
}

/** The tag to offer and store (a stored `ja` gives `ja-JP`; an unknown tag stays as typed), like `canonical`. */
export function canonical(tag: string, table: LanguageTable = LANGUAGE_TIERS): string {
  if (isAuto(tag)) return AUTO;
  return findLanguage(tag, table)?.code ?? tag.trim();
}

let displayNames: Intl.DisplayNames | null | undefined;

/** The platform's English name for a tag the table does not know, if it has one. */
function platformName(tag: string): string | null {
  if (displayNames === undefined) {
    try {
      displayNames = new Intl.DisplayNames(["en"], { type: "language", fallback: "none" });
    } catch {
      displayNames = null;
    }
  }
  try {
    return displayNames?.of(tag.trim()) ?? null;
  } catch {
    return null;
  }
}

/** "German", "Spanish, Spain", "Tamil" (platform names), "Detect automatically", or the tag. */
export function languageName(tag: string, table: LanguageTable = LANGUAGE_TIERS): string {
  if (isAuto(tag)) return "Detect automatically";
  return findLanguage(tag, table)?.name ?? platformName(tag) ?? tag.trim();
}

/** "German (de)", "Detect automatically", or the tag. */
export function languageLabel(tag: string, table: LanguageTable = LANGUAGE_TIERS): string {
  if (isAuto(tag)) return "Detect automatically";
  const name = languageName(tag, table);
  return name === tag.trim() ? name : `${name} (${tag.trim()})`;
}

/** The table entry for the system language (`navigator.languages`), else English. */
export function systemLanguage(tags: readonly string[], table: LanguageTable = LANGUAGE_TIERS): string {
  for (const t of tags) {
    const l = findLanguage(t, table) ?? findLanguage(primarySubtag(t), table);
    if (l && (l.tier === "ready" || l.tier === "broad")) return l.code;
  }
  return "en";
}

function subtags(file: ModelFile): string[] {
  const out: string[] = [];
  for (const t of file.capabilities.languages) {
    const p = primarySubtag(t);
    if (p && p !== AUTO && !out.includes(p)) out.push(p);
  }
  return out;
}

/** Whether the model lists its languages (Nemotron models do; Parakeet and most added GGUFs do not). */
export function listsLanguages(file: ModelFile): boolean {
  return subtags(file).length > 0;
}

/** Whether the model lists `tag` (`auto`: more than one language). */
export function lists(file: ModelFile, tag: string): boolean {
  const t = subtags(file);
  return isAuto(tag) ? t.length > 1 : t.includes(primarySubtag(tag));
}

/** Whether the model can be given `tag`: it lists it, or lists nothing (not checked, never switched). */
export function covers(file: ModelFile, tag: string): boolean {
  return tag.trim() === "" || !listsLanguages(file) || lists(file, tag);
}

function onlyLanguage(file: ModelFile): string | null {
  const t = subtags(file);
  return t.length === 1 ? t[0]! : null;
}

/** Installed streaming models other than `selected` that list languages: what a switch can use. */
function switchable(selected: ModelFile, files: ModelFile[]): ModelFile[] {
  return files
    .filter((f) => f.role === "asr" && !f.missing && f.id !== selected.id && f.capabilities.streaming && listsLanguages(f))
    .sort(
      (a, b) =>
        Number(a.manifest_name !== MULTILINGUAL_MODEL) - Number(b.manifest_name !== MULTILINGUAL_MODEL) || a.id - b.id,
    );
}

/** An installed streaming model other than `selected` that lists `tag`, Nemotron 3.5 first (`models::asr_for_language`). */
export function asrForLanguage(selected: ModelFile, files: ModelFile[], tag: string): ModelFile | null {
  return switchable(selected, files).find((f) => lists(f, tag)) ?? null;
}

/** "Nemotron 3.5" for the manifest model, else the file's name. */
export function modelName(file: ModelFile): string {
  if (file.manifest_name === MULTILINGUAL_MODEL) return "Nemotron 3.5";
  return file.display_name.replace(/^\./, "").replace(/\.gguf$/i, "");
}

function notCovered(selected: ModelFile, tag: string, table: LanguageTable): string {
  const only = onlyLanguage(selected);
  if (only) return `the selected model is ${languageName(only, table)}-only`;
  if (isAuto(tag)) return "the selected model cannot detect the language";
  return `the selected model does not cover ${languageName(tag, table)}`;
}

export interface LanguagePlan {
  /**
   * `selected`: the selected model runs it. `switch`: `file` runs it instead
   * (the selection stays). `install`: a known language nothing installed
   * lists. `unchecked`: the selected model runs it without a check (it
   * lists no languages, or the tag is one nothing knows).
   */
  kind: "selected" | "switch" | "install" | "unchecked";
  file: ModelFile | null;
  /** One sentence for the sheet, or null. */
  note: string | null;
}

/** What a job in `tag` runs on, given the scanned ASR files and the selected one. */
export function planFor(
  files: ModelFile[],
  selectedId: number | null,
  tag: string,
  table: LanguageTable = LANGUAGE_TIERS,
): LanguagePlan {
  if (tag === OTHER) {
    return { kind: "install", file: null, note: "Other languages need Nemotron 3.5 (40+ languages)." };
  }
  if (tag === TYPED) return { kind: "selected", file: null, note: null };
  const selected = files.find((f) => f.id === selectedId && !f.missing) ?? null;
  // Nothing selected: the backend's readiness check says what is missing.
  if (!selected) return { kind: "selected", file: null, note: null };
  if (!listsLanguages(selected)) {
    return {
      kind: "unchecked",
      file: selected,
      note: "Not checked for this model: it does not list its languages, so it runs as selected.",
    };
  }
  if (covers(selected, tag)) return { kind: "selected", file: selected, note: null };
  const other = asrForLanguage(selected, files, tag);
  if (other) {
    return {
      kind: "switch",
      file: other,
      note: `Transcribing with ${modelName(other)}: ${notCovered(selected, tag, table)}.`,
    };
  }
  if (!knows(tag, table)) {
    return {
      kind: "unchecked",
      file: selected,
      note: `Not checked for this model: no installed model lists ${tag.trim()}, so it is passed to ${modelName(selected)} as it is.`,
    };
  }
  const what = isAuto(tag) ? "Automatic detection" : languageName(tag, table);
  return { kind: "install", file: null, note: `${what} needs Nemotron 3.5: ${notCovered(selected, tag, table)}.` };
}

export interface LanguageOption {
  code: string;
  label: string;
  disabled: boolean;
}

const TIER_ORDER: Record<LanguageTier, number> = { ready: 0, broad: 1, adaptation: 2 };
const TIER_MARK: Record<LanguageTier, string> = { ready: "", broad: " (reduced accuracy)", adaptation: " (needs fine-tuning)" };

function entryLabel(l: LanguageEntry): string {
  return l.name + (l.tier ? TIER_MARK[l.tier] : "");
}

/** Sorts by tier (none last), then by name. */
function byTierThenName(a: { tier: LanguageTier | null; name: string }, b: { tier: LanguageTier | null; name: string }): number {
  const ta = a.tier ? TIER_ORDER[a.tier] : 3;
  const tb = b.tier ? TIER_ORDER[b.tier] : 3;
  return ta - tb || a.name.localeCompare(b.name);
}

/** The table's transcription-ready and broad-coverage languages, ready first, each by name. */
export function offeredLanguages(table: LanguageTable = LANGUAGE_TIERS): LanguageEntry[] {
  return table.filter((l) => l.tier === "ready" || l.tier === "broad").sort(byTierThenName);
}

/**
 * The language list for a sheet or Settings. Every language an installed
 * model that the job can run on declares (the selected one, and the
 * streaming models a switch can use), named from the table when it knows
 * the language, else by name or tag; with a model that lists no languages,
 * the table's ready and broad languages. Then "Detect automatically" when a
 * model detects, "Other languages: install Nemotron 3.5" when table
 * languages are left out (disabled unless `otherSelectable`), and
 * "Other tag…". `current` stays listed even when nothing covers it.
 */
export function languageOptions(
  files: ModelFile[],
  selectedId: number | null,
  current: string,
  otherSelectable: boolean,
  table: LanguageTable = LANGUAGE_TIERS,
): LanguageOption[] {
  const selected = files.find((f) => f.id === selectedId && !f.missing) ?? null;
  const entries: { code: string; label: string; tier: LanguageTier | null; name: string }[] = [];
  const add = (code: string, name: string, tier: LanguageTier | null, label: string) => {
    if (!entries.some((e) => e.code.toLowerCase() === code.toLowerCase())) entries.push({ code, label, tier, name });
  };
  let detects = true;
  let hidden = false;
  if (!selected || !listsLanguages(selected)) {
    for (const l of offeredLanguages(table)) add(l.code, l.name, l.tier, entryLabel(l));
  } else {
    const models = [selected, ...switchable(selected, files)];
    detects = models.some((m) => lists(m, AUTO));
    const declared = [...new Set(models.flatMap(subtags))];
    for (const p of declared) {
      const known = table.filter((l) => primarySubtag(l.code) === p);
      if (known.length > 0) for (const l of known) add(l.code, l.name, l.tier, entryLabel(l));
      else add(p, languageName(p, table), null, languageName(p, table));
    }
    hidden = offeredLanguages(table).some((l) => !declared.includes(primarySubtag(l.code)));
  }
  entries.sort(byTierThenName);
  const out: LanguageOption[] = entries.map((e) => ({ code: e.code, label: e.label, disabled: false }));
  if (detects) out.unshift({ code: AUTO, label: "Detect automatically", disabled: false });
  const cur = canonical(current, table);
  if (cur && cur !== OTHER && cur !== TYPED && !out.some((o) => o.code.toLowerCase() === cur.toLowerCase())) {
    const plan = planFor(files, selectedId, cur, table);
    const label =
      plan.kind === "install" ? `${languageName(cur, table)} (needs Nemotron 3.5)` : languageLabel(cur, table);
    out.unshift({ code: cur, label, disabled: false });
  }
  if (hidden) out.push({ code: OTHER, label: "Other languages: install Nemotron 3.5", disabled: !otherSelectable });
  out.push({ code: TYPED, label: "Other tag…", disabled: false });
  return out;
}
