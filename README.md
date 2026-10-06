# FindSmith

FindSmith is a browser-based Wikimedia find-and-replace tool inspired by AutoWikiBrowser. It is currently under development. Editors can review proposed changes and explicitly save individual pages to Wikimedia when their account has the required wiki access and edit rights.

The application has two parts:

- an Axum backend for Wikimedia OAuth, projects, MediaWiki requests, transformation jobs, CSRF validation, and JSON APIs;
- a React and TypeScript frontend built with Vite and served by Axum from `frontend/dist`.

OAuth client credentials stay on the backend. User OAuth tokens are carried in encrypted, HttpOnly session cookies; they are not stored in the application database or returned by its JSON APIs. Browser code receives workspace data and an application CSRF token.

## Local development

Install frontend dependencies and build the production assets:

```sh
npm install
npm run build
```

Run Axum after configuring the required environment variables:

```sh
cargo run
```

For frontend development with hot reload, run Vite separately. It proxies `/api` and `/oauth` to Axum on port 8000:

```sh
npm run dev --workspace findsmith-frontend
```

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
