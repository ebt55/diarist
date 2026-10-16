import { describe, expect, test } from "bun:test";
import type { ModelFile } from "../src/lib/generated/ModelFile";
import {
  LANGUAGE_TIERS,
  OTHER,
  TYPED,
  UNSUPPORTED_NOTE,
  canonical,
  isTag,
  languageLabel,
  languageOptions,
  offeredLanguages,
  planFor,
  systemLanguage,
} from "../src/lib/languages";

function asr(id: number, languages: string[], extra: Partial<ModelFile> = {}): ModelFile {
  return {
    id,
    path: `models/${id}.gguf`,
    role: "asr",
    architecture: "asr",
    display_name: `.model-${id}.gguf`,
    quant: null,
    size_bytes: 1,
    sha256: null,
    source: "manifest",
    capabilities: { streaming: true, diarize: false, languages, vram_mb_measured: null, vram_mb_estimate: null },
    manifest_name: null,
    first_seen_at: 0,
    last_seen_at: 0,
    missing: false,
    ...extra,
  };
}

const EN = asr(1, ["en"], { manifest_name: "nemotron-en" });
const N35 = asr(2, ["en-US", "en", "de-DE", "de", "es-ES", "es", "ja-JP", "zh-CN", "pl", "auto"], {
  manifest_name: "nemotron-3.5",
});
const PARAKEET = asr(3, [], { capabilities: { ...EN.capabilities, languages: [], streaming: false } });

/** Nemotron 3.5's whole prompt dictionary as `classify` reads it: 121 tags, far more than it was trained on. */
const N35_TAGS = [
  "en-US", "en", "en-GB", "enGB", "es-ES", "esES", "es-US", "es", "zh-CN", "zh-ZH", "zh-TW", "hi-IN", "hi", "hi-HI",
  "ar-AR", "ar", "fr-FR", "fr", "de-DE", "de", "ja-JP", "ja-JA", "ru-RU", "ru", "pt-BR", "pt-PT", "pt", "ko-KR", "ko",
  "ko-KO", "it-IT", "it", "nl-NL", "nl", "pl-PL", "pl", "tr-TR", "tr", "uk-UA", "uk", "ro-RO", "ro", "el-GR", "el",
  "cs-CZ", "cs", "hu-HU", "hu", "sv-SE", "sv", "da-DK", "da", "fi-FI", "fi", "no-NO", "no", "nb-NO", "nb", "nn-NO",
  "nn", "sk-SK", "sk", "hr-HR", "hr", "bg-BG", "bg", "lt-LT", "lt", "et-EE", "et", "lv-LV", "lv", "sl-SI", "sl",
  "th-TH", "vi-VN", "id-ID", "ms-MY", "bn-IN", "ur-PK", "fa-IR", "ta-IN", "te-IN", "mr-IN", "gu-IN", "kn-IN", "ml-IN",
  "si-LK", "ne-NP", "km-KH", "sw-KE", "am-ET", "ha-NG", "zu-ZA", "yo-NG", "ig-NG", "af-ZA", "rw-RW", "so-SO", "ny-MW",
  "ln-CD", "or-KE", "he-IL", "ku-TR", "az-AZ", "ka-GE", "hy-AM", "uz-UZ", "tg-TJ", "ky-KG", "qu-PE", "ay-BO", "gn-PY",
  "nah-MX", "mi-NZ", "haw-US", "sm-WS", "to-TO", "fr-CA", "mt-MT", "auto",
];
const N35_FULL = asr(5, N35_TAGS, { manifest_name: "nemotron-3.5" });

describe("tags", () => {
  test("stored bare tags map to the table", () => {
    expect(canonical("ja")).toBe("ja-JP");
    expect(canonical("zh")).toBe("zh-CN");
    expect(canonical("es")).toBe("es-US");
    expect(canonical("pt")).toBe("pt-PT");
    expect(canonical("de")).toBe("de");
    expect(canonical("zh-TW")).toBe("zh-TW");
    expect(languageLabel("de")).toBe("German (de)");
  });

  test("the system language picks an offered entry, else English", () => {
    expect(systemLanguage(["de-AT", "en"])).toBe("de");
    expect(systemLanguage(["pt-BR"])).toBe("pt-BR");
    expect(systemLanguage(["es-MX"])).toBe("es-US");
    expect(systemLanguage(["he-IL", "fr-FR"])).toBe("fr");
    expect(systemLanguage(["xx"])).toBe("en");
  });
});

describe("plans", () => {
  test("English on the EN model keeps the selection", () => {
    expect(planFor([EN, N35], 1, "en").kind).toBe("selected");
    expect(planFor([EN], 1, "en").kind).toBe("selected");
  });

  test("German on the EN model switches to an installed model that lists it", () => {
    const p = planFor([EN, N35], 1, "de");
    expect(p.kind).toBe("switch");
    expect(p.file?.id).toBe(2);
    expect(p.note).toBe("Transcribing with Nemotron 3.5: the selected model is English-only.");
    // By capability: a hand-dropped copy without a manifest name counts.
    expect(planFor([EN, { ...N35, manifest_name: null }], 1, "ja-JP").kind).toBe("switch");
  });

  test("German with nothing that covers it offers the download", () => {
    const p = planFor([EN], 1, "de");
    expect(p.kind).toBe("install");
    expect(p.note).toContain("German needs Nemotron 3.5");
    expect(planFor([EN], 1, OTHER).kind).toBe("install");
  });

  test("a model with no language list is not checked and never switches", () => {
    expect(planFor([PARAKEET, N35], 3, "de").kind).toBe("unchecked");
  });
});

describe("lists", () => {
  test("a single-language model lists its language and the install entry", () => {
    const codes = languageOptions([EN], 1, "en", false).map((o) => [o.code, o.disabled]);
    expect(codes).toEqual([
      ["en", false],
      [OTHER, true],
      [TYPED, false],
    ]);
  });

  test("with Nemotron 3.5 installed every covered language is offered, broad ones marked", () => {
    const opts = languageOptions([EN, N35], 1, "en", false);
    const codes = opts.map((o) => o.code);
    expect(codes[0]).toBe("auto");
    expect(codes).toContain("de");
    expect(codes).toContain("es-ES");
    expect(codes).toContain("ja-JP");
    expect(codes).not.toContain("fr");
    expect(opts.find((o) => o.code === "pl")?.label).toBe("Polish (reduced accuracy)");
    expect(codes).toContain(OTHER);
  });

  test("an uncovered stored language stays listed", () => {
    const opts = languageOptions([EN], 1, "de", true);
    expect(opts[0]).toEqual({ code: "de", label: "German (needs Nemotron 3.5)", disabled: false });
    expect(opts.at(-2)).toEqual({ code: OTHER, label: "Other languages: install Nemotron 3.5", disabled: false });
    expect(opts.at(-1)).toEqual({ code: TYPED, label: "Other tag…", disabled: false });
  });

  test("a model with no language list gets the full list", () => {
    const codes = languageOptions([PARAKEET], 3, "en", false).map((o) => o.code);
    expect(codes.length).toBe(1 + 30 + 1);
    expect(codes).not.toContain(OTHER);
    expect(codes).not.toContain("el");
  });

  test("Nemotron 3.5's 121 declared tags give only the table's ready and broad entries (no Malayalam)", () => {
    expect(N35_TAGS.length).toBe(121);
    const opts = languageOptions([N35_FULL], 5, "en", false);
    const codes = opts.map((o) => o.code);
    const offered = offeredLanguages().map((l) => l.code);
    expect(offered.length).toBe(30);
    expect(codes).toEqual(["auto", ...offered, TYPED]);
    for (const c of ["ml", "ta", "sw", "no", "el", "th", "he", "mt"]) expect(codes).not.toContain(c);
    expect(codes).toContain("de");
    expect(codes).toContain("hi");
    expect(opts.find((o) => o.code === "ja-JP")?.label).toBe("Japanese");
    expect(opts.find((o) => o.code === "zh-CN")?.label).toBe("Chinese (reduced accuracy)");
    // Every offered language is declared, so there is nothing to install.
    expect(codes).not.toContain(OTHER);
    // The same list when the EN model is selected and a switch can use Nemotron 3.5.
    expect(languageOptions([EN, N35_FULL], 1, "en", false).map((o) => o.code)).toEqual(codes);
  });

  test("a declared language the table does not offer is not listed", () => {
    const wide = asr(4, ["en", "de", "el-GR", "ta-IN", "qq-ZZ", "auto"]);
    const codes = languageOptions([wide], 4, "en", false).map((o) => o.code);
    expect(codes.slice(0, 3)).toEqual(["auto", "en", "de"]);
    for (const c of ["el", "ta", "qq"]) expect(codes).not.toContain(c);
    // Offered languages it does not declare: the install entry, then the typed tag.
    expect(codes.slice(3)).toEqual([OTHER, TYPED]);
  });

  test("a languages.user.json entry promotes an adaptation language or adds one", () => {
    const greek = { code: "el", name: "Greek", tier: "broad" as const, bare: true };
    const promoted = [...LANGUAGE_TIERS.filter((l) => l.code !== "el"), greek];
    expect(languageOptions([N35_FULL], 5, "en", false).some((o) => o.code === "el")).toBe(false);
    const opts = languageOptions([N35_FULL], 5, "en", false, promoted);
    expect(opts.find((o) => o.code === "el")?.label).toBe("Greek (reduced accuracy)");
    expect(planFor([N35_FULL], 5, "el", promoted).note).toBeNull();
    // Without a tier: listed plainly, after the tiers.
    const table = [...LANGUAGE_TIERS, { code: "ta", name: "Tamil (user)", tier: null, bare: true }];
    const tamil = languageOptions([N35_FULL], 5, "en", false, table);
    expect(tamil.find((o) => o.code === "ta")?.label).toBe("Tamil (user)");
    expect(tamil.at(-2)?.code).toBe("ta");
    // Still listed only for a model that declares it.
    expect(languageOptions([asr(4, ["en", "de"])], 4, "en", false, table).some((o) => o.code === "ta")).toBe(false);
  });
});

describe("typed tags", () => {
  test("a typed tag round-trips and passes through unchecked when nothing lists it", () => {
    expect(isTag("sw-KE") && isTag("x-klingon") && !isTag("not a tag")).toBe(true);
    expect(canonical(" sw-KE ")).toBe("sw-KE");
    const p = planFor([EN], 1, "sw-KE");
    expect(p.kind).toBe("unchecked");
    expect(p.note).toContain("Not checked for this model");
    // Listed as typed, with the plan's label.
    const opts = languageOptions([EN], 1, "sw-KE", true);
    expect(opts[0]?.code).toBe("sw-KE");
    // A typed known language still asks for Nemotron 3.5.
    expect(planFor([EN], 1, canonical("ja")).kind).toBe("install");
    // A typed tag that an installed model lists switches like a chosen one.
    expect(planFor([EN, N35], 1, "pl").kind).toBe("switch");
    expect(planFor([EN], 1, TYPED).kind).toBe("selected");
  });

  test("Other tag with a declared tag the table does not offer warns that the output may be unusable", () => {
    // Nemotron 3.5 declares ml-IN; typed ml runs on it, with the note.
    const p = planFor([N35_FULL], 5, canonical("ml"));
    expect(p.kind).toBe("selected");
    expect(p.note).toBe(UNSUPPORTED_NOTE);
    expect(UNSUPPORTED_NOTE).toBe("Not in this model's supported list; the output may be unusable.");
    expect(planFor([N35_FULL], 5, "el").note).toBe(UNSUPPORTED_NOTE);
    // A switch to the model that declares it says both.
    const s = planFor([EN, N35_FULL], 1, "ml");
    expect(s.kind).toBe("switch");
    expect(s.note).toBe(`Transcribing with Nemotron 3.5: the selected model is English-only. ${UNSUPPORTED_NOTE}`);
    // Listed as typed once chosen.
    expect(languageOptions([N35_FULL], 5, "ml", false)[0]).toEqual({
      code: "ml",
      label: languageLabel("ml"),
      disabled: false,
    });
    // Offered languages and automatic detection have no note.
    expect(planFor([N35_FULL], 5, "de").note).toBeNull();
    expect(planFor([N35_FULL], 5, "auto").note).toBeNull();
    // Nothing declares it: the existing "not checked" note.
    const none = planFor([N35_FULL], 5, "x-klingon");
    expect(none.kind).toBe("unchecked");
    expect(none.note).toContain("Not checked for this model");
    expect(planFor([EN], 1, "ml").note).toContain("Not checked for this model");
  });
});
