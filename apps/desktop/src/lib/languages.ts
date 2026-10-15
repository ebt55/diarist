/** Languages offered for new recordings and imports: BCP-47 code and name. */
export const LANGUAGES: [string, string][] = [
  ["auto", "Detect automatically"],
  ["en", "English"],
  ["de", "German"],
  ["es", "Spanish"],
  ["fr", "French"],
  ["it", "Italian"],
  ["nl", "Dutch"],
  ["pt", "Portuguese"],
  ["pl", "Polish"],
  ["ja", "Japanese"],
  ["zh", "Chinese"],
];

export function languageLabel(code: string, name: string): string {
  return code === "auto" ? name : `${name} (${code})`;
}
