# Adding a Language

Miru aims to support 1000+ languages. Adding one is a single file.

## Steps

1. Copy `ja.json` (the source) to `<your-locale>.json` using a BCP-47 tag:
   - `pt-BR.json` for Brazilian Portuguese
   - `zh-Hant.json` for Traditional Chinese
   - `de.json` if you want all German variants
2. Translate every value. Keep the keys identical.
3. Add an entry to `AVAILABLE_LOCALES` in `lib/i18n.ts` and to the resolver's
   match arms.
4. Submit a pull request titled `i18n: add <locale>`.

## Style

- Match the source register: terse, professional, no emoji.
- Brand names ("Miru", "Claude", "Constellation") stay in original spelling.
- Length matters: very long translations may overflow buttons. Aim for
  ≤ 1.5× the source length.
- Numbers and SHA hashes are not localized.

## Testing locally

```bash
cd crates/miru-client/ui
npm run dev
# In the running app, open settings → Language → choose your locale.
```

## Formatting

JSON files must:
- Be UTF-8 without BOM
- Use 2-space indentation
- End with a single newline
- Have keys in the same order as `ja.json` (helps reviewers spot missing keys)

A pre-commit check verifies these and warns on missing keys.
