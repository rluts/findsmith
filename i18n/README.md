# FindSmith messages

`en.json` is the English source catalog. `qqq.json` documents each message for translators. Both use JSON in the format accepted by translatewiki.net. Other language catalogs will be added only after the translatewiki.net project is configured and translations are exported; do not edit translated files in the running application.

The backend bundles every language catalog in `i18n/` at build time. It chooses the user's stored language preference (then the private language cookie, then English), translates API messages, and serves a merged catalog from `GET /api/translations/{language}`. Missing translated keys fall back to English. React retrieves that catalog; no translation service is contacted at runtime.

Keep message keys stable. A positional parameter such as `$1` must stay in translated text. For a count, use named CLDR plural forms, for example `$1 {{PLURAL:$1|one=page|other=pages}}`. Translators may add `zero`, `two`, `few`, or `many` forms as their language requires, but every plural needs `other`. Rust uses ICU4X cardinal plural rules; the frontend uses `Intl.PluralRules`. Plural categories depend on the selected interface language, not the wiki language.

Run `python3 scripts/check_i18n.py` before committing a catalog. The checker rejects duplicate or unknown keys, invalid language codes, malformed plural forms, and changed placeholder sets. Build the backend and frontend after changing message syntax. Translate complete sentences rather than stitching translated fragments together. Keep product names, MediaWiki API identifiers, wikitext examples, and placeholders unchanged where the `qqq` description says so.

Translatewiki.net onboarding still requires a project contact, repository and branch details, license confirmation, and agreement on how translated catalogs will be exported. No language catalog or bot configuration should be inferred from this guide alone.
