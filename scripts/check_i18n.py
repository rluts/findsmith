"""Check translation catalogs for duplicate, undocumented, and unused messages."""

from collections import defaultdict
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read_catalog(path: Path) -> dict:
    def unique_pairs(pairs: list[tuple[str, object]]) -> dict:
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"{path}: duplicate key {key}")
            result[key] = value
        return result

    return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=unique_pairs)


english = read_catalog(ROOT / "i18n/en.json")
documentation = read_catalog(ROOT / "i18n/qqq.json")
english_keys = set(english) - {"@metadata"}
documented_keys = set(documentation) - {"@metadata"}
plural = re.compile(r"\{\{PLURAL:\s*\$([1-9][0-9]*)\s*\|([^{}]*)\}\}", re.IGNORECASE)
explicit_form = re.compile(r"[0-9]+=")
placeholder = re.compile(r"\$([1-9][0-9]*)")


def message_problem(value: object) -> str | None:
    """Mirror build.rs: MediaWiki PLURAL forms are positional, with optional N= forms."""
    if not isinstance(value, str) or not value:
        return "must be a non-empty string"
    for match in plural.finditer(value):
        if all(explicit_form.match(form) for form in match[2].split("|")):
            return "PLURAL needs at least one form without a number"
    if "{{PLURAL:" in plural.sub("", value).upper():
        return "has malformed or nested PLURAL syntax"
    return None


english_path = ROOT / "i18n/en.json"
for key in english_keys:
    if problem := message_problem(english[key]):
        raise SystemExit(f"{english_path}: {key} {problem}")
# Translations come from translatewiki.net. build.rs drops defective messages
# so English covers them, so report them without failing the check.
for path in sorted((ROOT / "i18n").glob("*.json")):
    if path.name in {"en.json", "qqq.json"}:
        continue
    code = path.stem
    if not re.fullmatch(r"[a-z]{2,8}(?:-[a-z0-9]{1,16})*", code):
        raise SystemExit(f"{path}: invalid language code")
    translated = read_catalog(path)
    for key, value in translated.items():
        if key == "@metadata":
            continue
        if key not in english_keys:
            problem = "is not in the English catalog"
        else:
            problem = message_problem(value)
            extra = set(placeholder.findall(value)) - set(placeholder.findall(english[key])) if not problem else set()
            if extra:
                problem = f"uses placeholders missing from English: {sorted(extra)}"
        if problem:
            print(f"warning: {path}: {key} {problem}; English will be used", file=sys.stderr)
if english_keys != documented_keys:
    raise SystemExit(
        f"Catalog documentation mismatch: missing {sorted(english_keys - documented_keys)}, "
        f"extra {sorted(documented_keys - english_keys)}"
    )

# Identical English messages usually need one key. When the same English text
# genuinely needs different translations in different contexts, record the
# exact key group and explain the distinction here.
intentional_duplicate_values: dict[frozenset[str], str] = {}
value_keys: dict[str, set[str]] = defaultdict(set)
for key in english_keys:
    value_keys[english[key]].add(key)
duplicate_groups = {frozenset(keys) for keys in value_keys.values() if len(keys) > 1}
unexplained = duplicate_groups - intentional_duplicate_values.keys()
stale_exceptions = intentional_duplicate_values.keys() - duplicate_groups
if unexplained or stale_exceptions:
    details = []
    for group in sorted(unexplained, key=lambda keys: sorted(keys)):
        details.append(f"Duplicate English value: {sorted(group)}")
    for group in sorted(stale_exceptions, key=lambda keys: sorted(keys)):
        details.append(f"Stale duplicate exception: {sorted(group)}")
    raise SystemExit("\n".join(details))
if any(not reason.strip() for reason in intentional_duplicate_values.values()):
    raise SystemExit("Every intentional duplicate needs a reason")

# A final full stop alone does not normally justify another translation key.
# Keep a documented exception only when sentence punctuation changes how the
# text must be translated in its specific UI contexts.
intentional_period_variants: dict[frozenset[str], str] = {}
period_keys: dict[str, set[str]] = defaultdict(set)
for key in english_keys:
    value = english[key]
    comparable = value[:-1] if value.endswith(".") and not value.endswith("..") else value
    period_keys[comparable].add(key)
period_groups = {
    frozenset(keys)
    for keys in period_keys.values()
    if len({english[key] for key in keys}) > 1
}
unexplained_periods = period_groups - intentional_period_variants.keys()
stale_period_exceptions = intentional_period_variants.keys() - period_groups
if unexplained_periods or stale_period_exceptions:
    details = []
    for group in sorted(unexplained_periods, key=lambda keys: sorted(keys)):
        details.append(f"English values differ only by final period: {sorted(group)}")
    for group in sorted(stale_period_exceptions, key=lambda keys: sorted(keys)):
        details.append(f"Stale final-period exception: {sorted(group)}")
    raise SystemExit("\n".join(details))
if any(not reason.strip() for reason in intentional_period_variants.values()):
    raise SystemExit("Every final-period exception needs a reason")

source_files = list((ROOT / "src").rglob("*.rs")) + list(
    (ROOT / "frontend/src").rglob("*.ts*")
)

# Repository errors are mapped to user-facing responses in web.rs. Their Display
# implementations are only for diagnostics; translating them creates catalog
# entries that cannot reach the API response.
for filename, error_type in (("projects.rs", "ProjectError"), ("jobs.rs", "JobError")):
    source = (ROOT / "src" / filename).read_text(encoding="utf-8")
    display = re.search(
        rf"impl fmt::Display for {error_type} \{{(.*?)\nimpl Error for {error_type}",
        source,
        flags=re.DOTALL,
    )
    if display is None:
        raise SystemExit(f"Cannot inspect {error_type} Display implementation")
    if "localization::" in display[1] or "findsmith-" in display[1]:
        raise SystemExit(
            f"{error_type} Display must not use translations; localize its API mapping in web.rs"
        )

code = "\n".join(
    re.split(r"\n#\[cfg\(test\)\]\nmod tests", path.read_text(encoding="utf-8"))[0]
    for path in source_files
    if path.name != "localization.rs"
)

# These families are assembled from known protocol values at runtime.
dynamic_keys = {
    "findsmith-project-mask-" + part
    for part in (
        "comments", "tags", "references", "links", "headings", "templates", "link-targets"
    )
} | {
    "findsmith-project-source-" + part
    for part in (
        "manual", "category", "backlinks", "transclusions", "search", "watchlist",
        "contributions", "links-on-page", "categories-on-page", "redirects-to-page"
    )
} | {
    "findsmith-project-result-" + part
    for part in (
        "in-flight", "authentication", "permission", "block", "protection", "captcha",
        "spam", "conflict", "maxlag", "abuse-filter", "network", "unknown", "not-saved",
        "bot-exclusion"
    )
}
unused = sorted(key for key in english_keys if key not in code and key not in dynamic_keys)
if unused:
    raise SystemExit("Unused catalog keys:\n" + "\n".join(unused))

referenced = set(re.findall(r"['\"](findsmith-[a-z0-9]+(?:-[a-z0-9]+)+)['\"]", code))
missing = sorted(referenced - english_keys)
if missing:
    raise SystemExit("Referenced keys missing from English catalog:\n" + "\n".join(missing))

print(f"Checked {len(english_keys)} translation keys")
