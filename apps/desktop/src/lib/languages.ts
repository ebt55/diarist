// Languages for recordings and imports (docs/04 section 6): the tier table
// generated from `diarist_core::models::languages`, matched against the ASR
// models' languages the way the backend's `models::select_for_language`
// decides, so a sheet shows what the job will do: run on the selected
// model, switch to an installed model that lists the language, or offer the
// Nemotron 3.5 download. The backend checks again when the job starts.
import type { ModelFile } from "./generated/ModelFile";
import {
  LANGUAGE_TIERS,
  MULTILINGUAL_MODEL,
  TIER_EXPECTATIONS,
  TIER_TITLES,
  type LanguageTier,
} from "./generated/languageTiers";

export { LANGUAGE_TIERS, MULTILINGUAL_MODEL, TIER_EXPECTATIONS, TIER_TITLES, type LanguageTier };

/** Automatic detection; never the default. */
export const AUTO = "auto";
/** The "Other languages: install Nemotron 3.5" entry (not a language). */
export const OTHER = "other";
export const AUTO_NOTE = "Automatic detection loses about a second of the first utterance.";

export type LanguageEntry = (typeof LANGUAGE_TIERS)[number];

export function primarySubtag(tag: string): string {
  return (tag.trim().split(/[-_]/)[0] ?? "").toLowerCase();
}

export function isAuto(tag: string): boolean {
  return tag.trim().toLowerCase() === AUTO;
}

/** The entry `tag` stands for: its own, or for a bare subtag the one marked `bare` (`ja` is ja-JP). */
export function findLanguage(tag: string): LanguageEntry | null {
  const t = tag.trim().toLowerCase();
  const exact = LANGUAGE_TIERS.find((l) => l.code.toLowerCase() === t);
  if (exact) return exact;
  if (/[-_]/.test(t)) return null;
  return LANGUAGE_TIERS.find((l) => l.bare && primarySubtag(l.code) === t) ?? null;
}

/** The tag to offer and store for `tag` (a stored `ja` gives `ja-JP`), like `languages::canonical`. */
export function canonical(tag: string): string {
  if (isAuto(tag)) return AUTO;
  return findLanguage(tag)?.code ?? tag.trim();
}

/** "German", "Spanish, Spain", "Detect automatically", or the tag. */
export function languageName(tag: string): string {
  if (isAuto(tag)) return "Detect automatically";
  return findLanguage(tag)?.name ?? tag.trim();
}

/** "German (de)", "Detect automatically", or the tag. */
export function languageLabel(tag: string): string {
  if (isAuto(tag)) return "Detect automatically";
  const l = findLanguage(tag);
  return l ? `${l.name} (${tag.trim()})` : tag.trim();
}

/** The tier-table entry for the system language (`navigator.languages`), else English. */
export function systemLanguage(tags: readonly string[]): string {
  for (const t of tags) {
    const l = findLanguage(t) ?? findLanguage(primarySubtag(t));
    if (l && l.tier !== "adaptation") return l.code;
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

/** An installed streaming model other than `selected` that lists `tag`, Nemotron 3.5 first (`models::asr_for_language`). */
export function asrForLanguage(selected: ModelFile, files: ModelFile[], tag: string): ModelFile | null {
  const found = files
    .filter((f) => f.role === "asr" && !f.missing && f.id !== selected.id && f.capabilities.streaming && lists(f, tag))
    .sort(
      (a, b) =>
        Number(a.manifest_name !== MULTILINGUAL_MODEL) - Number(b.manifest_name !== MULTILINGUAL_MODEL) || a.id - b.id,
    );
  return found[0] ?? null;
}

/** "Nemotron 3.5" for the manifest model, else the file's name. */
export function modelName(file: ModelFile): string {
  if (file.manifest_name === MULTILINGUAL_MODEL) return "Nemotron 3.5";
  return file.display_name.replace(/^\./, "").replace(/\.gguf$/i, "");
}

function notCovered(selected: ModelFile, tag: string): string {
  const only = onlyLanguage(selected);
  if (only) return `the selected model is ${findLanguage(only)?.name ?? only}-only`;
  if (isAuto(tag)) return "the selected model cannot detect the language";
  return `the selected model does not cover ${languageName(tag)}`;
}

export interface LanguagePlan {
  /**
   * `selected`: the selected model runs it. `switch`: `file` runs it instead
   * (the selection stays). `install`: nothing installed covers it.
   * `unchecked`: the selected model lists no languages, so nothing is checked.
   */
  kind: "selected" | "switch" | "install" | "unchecked";
  file: ModelFile | null;
  /** One sentence for the sheet, or null. */
  note: string | null;
}

/** What a job in `tag` runs on, given the scanned ASR files and the selected one. */
export function planFor(files: ModelFile[], selectedId: number | null, tag: string): LanguagePlan {
  if (tag === OTHER) {
    return { kind: "install", file: null, note: "Other languages need Nemotron 3.5 (40+ languages)." };
  }
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
    return { kind: "switch", file: other, note: `Transcribing with ${modelName(other)}: ${notCovered(selected, tag)}.` };
  }
  const what = isAuto(tag) ? "Automatic detection" : languageName(tag);
  return { kind: "install", file: null, note: `${what} needs Nemotron 3.5: ${notCovered(selected, tag)}.` };
}

export interface LanguageOption {
  code: string;
  label: string;
  disabled: boolean;
}

function optionLabel(l: LanguageEntry): string {
  return l.tier === "broad" ? `${l.name} (reduced accuracy)` : l.name;
}

/** Offered languages: transcription-ready, then broad coverage, each by name. */
export function offeredLanguages(): LanguageEntry[] {
  const byName = (a: LanguageEntry, b: LanguageEntry) => a.name.localeCompare(b.name);
  return [
    ...LANGUAGE_TIERS.filter((l) => l.tier === "ready").sort(byName),
    ...LANGUAGE_TIERS.filter((l) => l.tier === "broad").sort(byName),
  ];
}

/**
 * The language list for a sheet or Settings: "Detect automatically" and the
 * offered languages that the selected model or another installed one
 * covers; with a single-language model and nothing else installed, its
 * language plus "Other languages: install Nemotron 3.5" (disabled unless
 * `otherSelectable`). `current` stays listed even when nothing covers it.
 */
export function languageOptions(
  files: ModelFile[],
  selectedId: number | null,
  current: string,
  otherSelectable: boolean,
): LanguageOption[] {
  const selected = files.find((f) => f.id === selectedId && !f.missing) ?? null;
  const runs = (code: string) => !selected || covers(selected, code) || asrForLanguage(selected, files, code) !== null;
  const out: LanguageOption[] = [];
  if (runs(AUTO)) out.push({ code: AUTO, label: "Detect automatically", disabled: false });
  let hidden = false;
  for (const l of offeredLanguages()) {
    if (runs(l.code)) out.push({ code: l.code, label: optionLabel(l), disabled: false });
    else hidden = true;
  }
  const cur = canonical(current);
  if (cur && cur !== OTHER && !out.some((o) => o.code === cur)) {
    out.unshift({ code: cur, label: `${languageName(cur)} (needs Nemotron 3.5)`, disabled: false });
  }
  if (hidden) {
    out.push({ code: OTHER, label: "Other languages: install Nemotron 3.5", disabled: !otherSelectable });
  }
  return out;
}
