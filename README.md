# FindSmith

FindSmith is a browser-based Wikimedia find-and-replace tool inspired by AutoWikiBrowser. It is currently under development. Editors can review proposed changes and explicitly save individual pages to Wikimedia when their account has the required wiki access and edit rights.

The application has two parts:

- an Axum backend for Wikimedia OAuth, projects, MediaWiki requests, transformation jobs, CSRF validation, and JSON APIs;
- a React and TypeScript frontend built with Vite and served by Axum from `frontend/dist`.

OAuth client credentials stay on the backend. User OAuth tokens are carried in encrypted, HttpOnly session cookies; they are not stored in the application database or returned by its JSON APIs. Browser code receives workspace data and an application CSRF token.

## Interface language

English messages live in `i18n/en.json`; translator context lives in `i18n/qqq.json`. Put imported translatewiki.net catalogs in `i18n/<language>.json` using lowercase language codes. The Rust build automatically embeds every catalog in that directory except `qqq.json`; no per-language code change is needed. `GET /api/translations` lists available languages for the frontend selector, and `GET /api/translations/{language}` serves the requested catalog with missing messages filled from English. Requests for a valid language without a catalog receive the English fallback with `language: "en"` in the response. The frontend loads the catalog before rendering, and Axum resolves structured API errors, review messages, and page-source labels through the same catalog. Catalogs are delivered through source control and included in the Toolforge build, never fetched from translatewiki.net at runtime.

Run `python3 scripts/check_i18n.py` after changing messages; it checks for duplicate keys and English values (including values that differ only by a final period), missing translator documentation, unused messages, referenced keys absent from the English catalog, and translations accidentally added to internal repository error displays. Reuse one key for identical messages. If matching English text needs distinct translations because the contexts differ, document the exact key group and reason in the script. API responses are localized in `src/web.rs`; internal diagnostics do not need translation. Static checks cannot prove that every error branch is reachable through an API endpoint, so review new backend keys at their response boundary.

`GET /api/session` returns the selected language and a CSRF token. `POST /api/language` accepts `language` and `csrf_token` form fields. The selected language is stored in an encrypted, HttpOnly cookie; for signed-in users it is also stored by Wikimedia central ID in `user_preferences`. The database value takes precedence over the cookie and is copied back to it when the session is read. English is the fallback. Only the English catalog is available so far.

## Local development

See [CONTRIBUTING.md](CONTRIBUTING.md) for setup, configuration, running locally, and the checks a change must pass.

## Toolforge build service

The root `package.json` lets Toolforge add Node.js alongside the Rust buildpack. Its `heroku-postbuild` script creates `frontend/dist` before the image is finalized. `Procfile` starts the release binary, and `service.template` selects the build service with no NFS mount.

Start a new build from the Toolforge bastion:

```sh
become findsmith
toolforge build start https://gitlab.wikimedia.org/toolforge-repos/findsmith
```

After the build succeeds, deploy the new image:

```sh
toolforge webservice buildservice restart
```

For the first deployment, use:

```sh
toolforge webservice buildservice start --mount=none
```

Runtime configuration remains in Toolforge envvars. Do not put OAuth or database secrets in Vite variables, source files, or frontend assets.
