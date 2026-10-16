import { describe, expect, test } from "bun:test";
import type { ModelFile } from "../src/lib/generated/ModelFile";
import {
  LANGUAGE_TIERS,
  OTHER,
  TYPED,
  canonical,
  isTag,
  languageLabel,
  languageName,
  languageOptions,
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

  test("every language a model declares is offered, unknown ones by name or tag", () => {
    const wide = asr(4, ["en", "de", "el-GR", "ta-IN", "qq-ZZ", "auto"]);
    const opts = languageOptions([wide], 4, "en", false);
    const ta = opts.find((o) => o.code === "ta");
    expect(ta).toBeDefined();
    expect(ta!.label).toBe(languageName("ta")); // "Tamil" from the platform, else "ta"
    expect(opts.find((o) => o.code === "qq")?.label).toBe("qq");
    // The adaptation tier shows, marked, because the model lists it.
    expect(opts.find((o) => o.code === "el")?.label).toBe("Greek (needs fine-tuning)");
    // Untiered languages come after the tiers.
    const codes = opts.map((o) => o.code);
    expect(codes.indexOf("ta")).toBeGreaterThan(codes.indexOf("el"));
  });

  test("the effective table names and marks languages", () => {
    const table = [...LANGUAGE_TIERS, { code: "ta", name: "Tamil (user)", tier: null, bare: true }];
    const wide = asr(4, ["en", "ta-IN"]);
    expect(languageOptions([wide], 4, "en", false, table).find((o) => o.code === "ta")?.label).toBe("Tamil (user)");
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
});
