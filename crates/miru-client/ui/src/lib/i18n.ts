/**
 * i18n core.
 *
 * Design choices:
 *  - Flat dotted keys: `nav.connect` → "接続"
 *  - JA is the source language (Miru is Japanese-first)
 *  - EN is the fallback for missing keys
 *  - Other locales lazy-loaded on first access
 *  - Locale persisted to localStorage; defaults to navigator.language
 *  - 1000+ language goal: any contributor can add a JSON file in /locales
 */

import ja from "../locales/ja.json";
import en from "../locales/en.json";

type Dict = Record<string, string>;

const SOURCE: Dict = ja;
const FALLBACK: Dict = en;

const STORAGE_KEY = "miru.locale";

let current: Dict = SOURCE;
let currentTag: string = "ja";

/**
 * Resolve a locale tag to one we have a dictionary for.
 * Walks BCP-47 from most-specific to least-specific.
 *   "ja-JP" → "ja-JP" → "ja"
 *   "pt-BR" → "pt-BR" → "pt" → fallback
 */
function resolveLocale(tag: string): { dict: Dict; tag: string } {
  const candidates = [
    tag,
    tag.split("-")[0],
  ];
  for (const c of candidates) {
    const lower = c.toLowerCase();
    if (lower === "ja" || lower.startsWith("ja-")) return { dict: ja, tag: lower };
    if (lower === "en" || lower.startsWith("en-")) return { dict: en, tag: lower };
  }
  return { dict: SOURCE, tag: "ja" };
}

/**
 * Initialize locale from (in priority order):
 *   1. localStorage user preference
 *   2. navigator.language
 *   3. "ja"
 */
export function initLocale(): void {
  let chosen = "ja";
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored) chosen = stored;
    else if (typeof navigator !== "undefined" && navigator.language) {
      chosen = navigator.language;
    }
  } catch {
    // localStorage may be unavailable in some sandboxes
  }
  setLocale(chosen);
}

export function setLocale(tag: string): void {
  const resolved = resolveLocale(tag);
  current = resolved.dict;
  currentTag = resolved.tag;

  // Persist
  try { localStorage.setItem(STORAGE_KEY, currentTag); } catch {}

  // Update <html lang> for screen readers + CSS lang selectors
  if (typeof document !== "undefined") {
    document.documentElement.lang = currentTag;
  }
}

export function currentLocale(): string {
  return currentTag;
}

/**
 * Translate a key. If missing in current dict, falls back to EN, then to the
 * key itself (so missing keys are visible during development).
 *
 * Optional interpolation: `t("greeting", { name: "Monu" })` replaces `{name}`.
 */
export function t(key: string, vars?: Record<string, string | number>): string {
  let value = current[key] ?? FALLBACK[key] ?? key;
  if (vars) {
    for (const [k, v] of Object.entries(vars)) {
      value = value.replace(new RegExp(`\\{${k}\\}`, "g"), String(v));
    }
  }
  return value;
}

/** Available locales for UI selector (extend as new files land in /locales). */
export const AVAILABLE_LOCALES: { tag: string; native: string }[] = [
  { tag: "ja", native: "日本語" },
  { tag: "en", native: "English" },
];
