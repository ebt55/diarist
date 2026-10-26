// Web addresses: inside a sentence (a Diagnostics row's detail names the
// Microsoft Visual C++ download page), so the row can offer Copy link; and
// every address of a download, a multi-file one (a punctuation model: its
// tokenizer, licences, ONNX Runtime library and model) included. Plain
// functions, so `bun test` covers them (tests/links.test.ts).
import type { DownloadItem } from "./generated/DownloadItem";

export interface TextPart {
  text: string;
  /** `text` is a web address. */
  url: boolean;
}

const URL_RE = /https?:\/\/[^\s<>"')]+/g;

/** `s` split around its web addresses; a sentence's own full stop or comma after one is not part of it. */
export function linkParts(s: string): TextPart[] {
  const out: TextPart[] = [];
  let last = 0;
  for (const m of s.matchAll(URL_RE)) {
    const trail = /[.,;:!?]+$/.exec(m[0])?.[0] ?? "";
    const url = m[0].slice(0, m[0].length - trail.length);
    const at = m.index ?? 0;
    if (at > last) out.push({ text: s.slice(last, at), url: false });
    out.push({ text: url, url: true });
    last = at + url.length;
  }
  if (last < s.length) out.push({ text: s.slice(last), url: false });
  return out;
}

/** The web addresses in `s`, in order. */
export function urlsIn(s: string): string[] {
  return linkParts(s)
    .filter((p) => p.url)
    .map((p) => p.text);
}

/** Every address of `items`: each file of a multi-file download (in download order), else its one URL. */
export function downloadLinks(items: readonly DownloadItem[]): string[] {
  return items.flatMap((i) => (i.files && i.files.length > 0 ? i.files.map((f) => f.url) : [i.url]));
}

/** A multi-file download's files that are not there yet. */
export function missingFiles(item: DownloadItem): number {
  return (item.files ?? []).filter((f) => !f.present).length;
}
