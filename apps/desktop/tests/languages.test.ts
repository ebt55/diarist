import { describe, expect, test } from "bun:test";
import type { ModelFile } from "../src/lib/generated/ModelFile";
import {
  OTHER,
  canonical,
  languageLabel,
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
    expect(opts.at(-1)).toEqual({ code: OTHER, label: "Other languages: install Nemotron 3.5", disabled: false });
  });

  test("a model with no language list gets the full list", () => {
    const codes = languageOptions([PARAKEET], 3, "en", false).map((o) => o.code);
    expect(codes.length).toBe(1 + 30);
    expect(codes).not.toContain(OTHER);
  });
});
