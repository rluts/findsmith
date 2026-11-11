# Changelog

All notable changes to FindSmith are documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## **Unreleased**

### Added

- Typo fixing with the wiki's `Project:AutoWikiBrowser/Typos` list, as in AutoWikiBrowser's RegExTypoFix. It runs after the replacement rules, skips templates, links, quotations, italics, comments and references the same way, and adds AWB's "typos fixed: …" text to the suggested edit summary.
- A **Typos** tab showing which typo rules matched the current page, and the rules from the list that could not be used.
- Skip option "Skip pages where no typo is fixed".

### Changed

- Regex replacement rules and regex skip conditions now use .NET regular-expression syntax, as in AutoWikiBrowser, so AWB find-and-replace settings work unchanged. This adds lookbehind of any length, backreferences and `$1a`-style replacements. Rules written for Rust syntax may need updating: `(?P<name>…)` becomes `(?<name>…)`, `\pL` becomes `\p{L}`, `[[:alpha:]]` and `&&`/`--` class operations are not supported, and `$` also matches before a final newline. A rule that runs too long on a page now stops with an error instead of hanging.
- Typo lists are written for .NET regular expressions. A new compatibility layer runs them in Rust with .NET behaviour, including lookbehind of any length, backreferences, `$1a`-style replacements, `$` before a final newline and character-class subtraction.

## [0.1.0-alpha.5] - 2026-11-06

### Changed

- **Save and next** is now the primary review action. Skipped pages can still be saved, and their skip reason is kept.
- The diff updates automatically after the proposed wikitext is edited by hand.
- The review shows the page position and saved and skipped counts, plus a summary when the review is complete.
- The "Pre-parse queue" option is renamed to "Skip pages without changes".
- Project:AutoWikiBrowser/CheckPageJSON may now have any active edit protection (see testwiki, dewiki), from autoconfirmed up, instead of requiring sysop protection.
- CheckPageJSON errors in the project access notice are translated instead of showing raw message keys.


## [0.1.0-alpha.4] - 2026-11-05

### Added

- New skip option, on by default, for pages with an "in use" or "under construction" template. Each wiki's templates and their redirects are found through Wikidata.
- Registered wikis reload their namespaces and templates every 7 days in the background.
- Warnings about unsaved changes when navigating away or reloading the page.
- Recovery of queue builds that were interrupted or abandoned.
- `/readyz` database check, also available in owner-token mode, and graceful shutdown on SIGTERM.

### Changed

- Pages are skipped automatically only after the user starts reviewing.
- A project's wiki can no longer be changed after the project is created.
- Autosave keeps local settings and manually edited text more reliably.
- Almost 100 messages are now hard-coded in English instead of being translated, to make the catalogs easier to maintain.
- Updated dependencies, including `mwapi` and `mwbot` 0.8 release candidates, `parsoid` 0.11 release candidate, `mwoauth` 0.1 release candidate, and `reqwest` 0.13.

### Removed

- Unified diff view; the side-by-side diff remains.

### Fixed

- Replacement rules handle `$` correctly.
- Titles of templates and categories are normalized the way MediaWiki does it.
- Masking no longer treats markup inside tags such as `<nowiki>` and `<pre>` as parsed wikitext.
- Saves still in progress during a state change are recorded correctly, and revision history checks use SHA-1 hashes.
- The current job is determined by creation time.

## 0.1.0-alpha.3 - 2026-10-30

### Added

- Example `.env` file for local development.
- New interface text for alpha and beta versions.

### Changed

- Improved interface text, with separate "Add wiki" messages for the navigation and the submit button, and a clearer name for the check page.
- Reworked the local development scripts.

## [0.1.0-alpha.2] - 2026-10-29

### Added

- Per-project skip options: no change, redirects, pages the account cannot edit, pages outside the namespace filter, text or regex the page contains or does not contain, and minimum and maximum replacement counts.
- Skip reasons recorded in the queue, with automatic skipping during review and pre-parse. Missing pages are marked as missing.
- Pages whose `{{bots}}` or `{{nobots}}` templates exclude the account are skipped.
- Account checks for blocks and missing rights or grants before a review starts.
- Verify action that checks page history when an edit's outcome is unknown.
- Project autosave and project duplication.
- Notice when the latest review is outdated.

### Changed

- Saves by the same account are at least 5 seconds apart. A job stops after a policy-sensitive failure or 3 consecutive failed saves.
- MediaWiki clients set maxlag, retries, and connection and request timeouts.
- Aligned the skip option checkboxes and hint text.

### Removed

- Conditional rule groups.

## [0.1.0-alpha.1] - 2026-10-24

First tagged version.

### Added

- Find-and-replace projects with queue-based review of proposed changes, using Wikimedia OAuth sign-in and a MariaDB database.
- Page sources for building the queue, with sorting and pre-parsing.
- Structural rule operations through Parsoid, and reusable saved rules.
- Side-by-side diffs with inline highlighting, and rendered previews.
- Saving reviewed pages to the wiki with edit summaries, including after a review is complete.
- Registering Wikimedia wikis, with access checked against `Project:AutoWikiBrowser/CheckPageJSON`.
- React and TypeScript frontend built with Vite.
- Interface translation using the MediaWiki message format for translatewiki.net.
- The FindSmith logo and updated styles.

[0.1.0-alpha.5]: https://gitlab.wikimedia.org/toolforge-repos/findsmith/-/tags/0.1.0-alpha.5
[0.1.0-alpha.4]: https://gitlab.wikimedia.org/toolforge-repos/findsmith/-/tags/0.1.0-alpha.4
[0.1.0-alpha.2]: https://gitlab.wikimedia.org/toolforge-repos/findsmith/-/tags/0.1.0-alpha.2
[0.1.0-alpha.1]: https://gitlab.wikimedia.org/toolforge-repos/findsmith/-/tags/0.1.0-alpha.1
