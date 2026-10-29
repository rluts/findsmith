# Contributing to FindSmith

Thank you for helping improve FindSmith. This guide covers how to set up a development environment, the checks a change must pass, and the project rules that keep editing safe.

## Ways to contribute

- Report bugs and suggest features to [Wikimedia Phabricator](https://phabricator.wikimedia.org/tag/tool-findsmith/)
<!-- - Translate the interface on translatewiki.net: TODO: add translatewiki link -->
- Improve the code or documentation through merge requests on [Wikimedia GitLab](https://gitlab.wikimedia.org/toolforge-repos/findsmith).

## Code of conduct

FindSmith is a Wikimedia community tool. Contributors are expected to follow the [Wikimedia Code of Conduct for technical spaces](https://www.mediawiki.org/wiki/Code_of_Conduct).

## LLM usage

You may use large language models and AI coding assistants to help write code, tests, documentation, or messages. You remain the author and are responsible for everything you submit:

- Do not submit code or text you do not understand. You should be able to explain every change and answer review questions about it yourself.
- Review generated code as carefully as code you wrote by hand. Everything in [Before you open a merge request](#before-you-open-a-merge-request) still applies, including testing the change yourself; an assistant saying it works is not enough.
- Do not submit large amounts of generated code at once. A person must be able to review every line, so split generated changes into small, focused merge requests.
- Check generated content for licensing problems; do not submit material copied from sources that are incompatible with `GPL-3.0-or-later`.
- Never paste secrets, tokens, `.env` contents, or private user data into an external AI service.
- Do not use AI tools to write interface translations; translations come from translatewiki.net.
- Mention significant AI assistance in the merge request description, for example with a `Co-Authored-By` line or a short note.

## Development setup

### Requirements

- Rust: the toolchain version is pinned in `rust-toolchain.toml` and installed automatically by `rustup`.
- Node.js 22.12 or later, and npm 10 or later.
- MariaDB or MySQL. `mariadb-docker.sh` starts a local MariaDB container.
- A Wikimedia OAuth 2 consumer, or an owner-only access token for local testing. You can create it on [Meta](https://meta.wikimedia.org/wiki/Special:OAuthConsumerRegistration/propose/oauth2)

### Configuration

Create a `.env` file in the repository root. It is ignored by Git; never commit it.

| Variable                                     | Required                           | Purpose                                                                                                                                   |
|----------------------------------------------|------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------|
| `COOKIE_KEY`                                 | Yes                                | Base64 encoding of exactly 64 random bytes, for example `openssl rand -base64 64 \| tr -d '\n'`.                                          |
| `DATABASE_NAME`                              | Yes                                | Must start with `<database user>__`, for example `s12345__findsmith`.                                                                     |
| `TOOL_TOOLSDB_USER`, `TOOL_TOOLSDB_PASSWORD` | Set both, or neither               | Database credentials. Without them, `DATABASE_CREDENTIALS_FILE` or `replica.my.cnf` is read.                                              |
| `DATABASE_HOST`, `DATABASE_PORT`             | No                                 | Default to `tools.db.svc.wikimedia.cloud` and `3306`; use `127.0.0.1` locally.                                                            |
| `OAUTH_CLIENT_ID`, `OAUTH_CLIENT_SECRET`     | Unless `OWNER_ACCESS_TOKEN` is set | Wikimedia OAuth 2 consumer.                                                                                                               |
| `OAUTH_CALLBACK_URL`                         | No                                 | Defaults to `http://127.0.0.1:8000/oauth/callback`.                                                                                       |
| `OWNER_ACCESS_TOKEN`                         | No                                 | Owner-only access token that replaces the login flow; allowed only with `DEBUG=true`.                                                     |
| `DEBUG`                                      | No                                 | `true` or `false` (default). `true` requires a local callback URL and allows non-secure cookies.                                          |
| `PORT`                                       | No                                 | Defaults to `8000`.                                                                                                                       |
| `USER_AGENT`                                 | Recommended                        | User-Agent with contact information, as required by the [Wikimedia User-Agent policy](https://meta.wikimedia.org/wiki/User-Agent_policy). |

### Running locally

```sh
./mariadb-docker.sh    # first time only: start MariaDB in Docker
npm install
./run-local.sh         # builds the frontend, loads .env, runs cargo run
```

For frontend work with hot reload, run Axum and Vite side by side:

```sh
npm run dev --workspace findsmith-frontend
```

Database migrations run automatically when the server starts.

### Testing edits

Real edits made while developing must not touch content pages. Test only on pages in your own user space (for example `User:YourName/FindSmith test`) or in the Draft namespace on wikis that have one, and give test projects a page list made only of those pages. Clean up test pages when you are done, following the wiki's rules for deletion requests.

## Project layout

| Path                                                          | Contents                                                                |
|---------------------------------------------------------------|-------------------------------------------------------------------------|
| `src/web.rs`, `src/app.rs`                                    | HTTP handlers, JSON responses, and routes.                              |
| `src/mediawiki.rs`, `src/error.rs`                            | MediaWiki Action API client and failure categories.                     |
| `src/rules.rs`, `src/structural.rs`, `src/masking.rs`         | Replacement rules and Parsoid-based operations.                         |
| `src/jobs.rs`, `src/lists.rs`, `src/skip.rs`, `src/policy.rs` | Review queues, page lists, skip rules, and editing policy.              |
| `src/projects.rs`, `src/entities/`, `src/migration/`          | Persistence.                                                            |
| `frontend/src/`                                               | React and TypeScript interface.                                         |
| `i18n/`                                                       | Message catalogs (`en.json`) and translator documentation (`qqq.json`). |

## Before you open a merge request

- Test the change yourself: run FindSmith locally and try the affected feature by hand, saving edits only as described in [Testing edits](#testing-edits). Passing automated checks is not enough.
- Keep merge requests small enough for a person to review carefully. Split large changes into focused merge requests, each with one purpose.

Run every check locally; all of them must pass:

```sh
cargo fmt --check
cargo clippy --all-targets
cargo test
python scripts/check_i18n.py
npm run build
```

### Coding guidelines

- Match the style of the surrounding code; `cargo fmt` is authoritative for Rust.
- Add unit tests for new logic, especially rules, queue state, skip rules, and policy checks.
- Keep changes focused; unrelated refactoring belongs in a separate merge request.
- Database changes need a new migration in `src/migration/`; never edit a migration that has already been released.
- New fields in stored project or job state must have serde defaults so existing records still load.

### Interface messages

- Add every user-facing string to `i18n/en.json` and document it in `i18n/qqq.json` with its context and an example for each parameter.
- Use complete sentences with `$1`-style parameters instead of joining translated fragments; use `{{PLURAL:$1|…}}` for counts.
- Do not edit translated catalogs by hand; they are imported from translatewiki.net.

See the "Interface language" section of `README.md` for details.

## Safety and policy requirements

FindSmith edits Wikimedia wikis on behalf of its users. Changes must preserve these guarantees:

- Every edit is reviewed and explicitly confirmed by a person; no unattended or bot-mode editing path.
- OAuth client secrets stay on the server. A user's OAuth token is kept only in the encrypted, `HttpOnly` session cookie: it never appears in HTML, URLs, JSON responses, logs, the database, or storage that page scripts can read (`localStorage`, `sessionStorage`, IndexedDB, or non-`HttpOnly` cookies).
- Edits use `assert=user`, `maxlag`, base revision IDs, and timestamps, and stop on policy-sensitive failures.
- `{{bots}}` and `{{nobots}}` exclusions are respected.
- Page text and private data stay out of logs.

## Commit messages and merge requests

- Write a short summary line in the imperative mood, followed by details if needed.
- Describe what changed and how you tested it in the merge request.

## Releases

Versions follow [Semantic Versioning](https://semver.org/). Pre-releases use the `-alpha.N`, `-beta.N`, and `-rc.N` suffixes; keep the version in `Cargo.toml`, `package.json`, and `frontend/package.json` in sync.

Automatic deployment is not ready yet.

## License

FindSmith is licensed under the GNU General Public License, version 3 or later (`GPL-3.0-or-later`). By contributing, you agree that your contributions are licensed under the same terms.
