# Build, Publication, Deployment, and Rollback

## Release boundaries

There are three separate operations:

1. build and test native/WASM source;
2. publish verified CLI archives to a real GitHub Release;
3. publish the static site through NiceChunk's private, fixed, content-addressed
   full-site deployment path.

Success in one does not imply the others. A Git push is not a deployment. A
browser verifier result is not a chain submission. A generated Release workflow
does not mean a Release exists.

## Production build

From the miner root with Rust 1.88, the wasm32 target, Node 20+, and
wasm-bindgen-cli 0.2.126:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check -p pouw-core --no-default-features
cargo build -p pouw-cli
nicechunk-miner self-test

npm ci
node scripts/build-web.mjs
node scripts/check-web.mjs
node scripts/test-wasm-consistency.mjs
node scripts/test-browser.mjs
node scripts/test-nginx-release.mjs
node scripts/audit-secrets.mjs
```

In the NiceChunk source-of-truth checkout also run
`scripts/test-js-codec-compatibility.mjs`; it compares Rust semantics with the
current real ChunkBroken SDK, NCM3, and NCF1 JavaScript decoders.

`web/dist` uses relative asset URLs and is valid at `/miner/`. JS, CSS, WASM,
logo, locale, and fixture names contain a 12-hex SHA-256 prefix. HTML and the
release manifest are deliberately unhashed.

Create a content-addressed handoff artifact:

```bash
node scripts/build-web-release.mjs --dist web/dist --output artifacts/web
```

The command prints release ID, manifest/archive digests, file count, and byte
size. It refuses symlinks and special files, and normalizes archive ordering,
timestamps, ownership, and tar format so identical inputs produce the same
release ID and archive digest. This is a build artifact, not a server deployment
command.

## CLI Release workflow

`.github/workflows/release.yml` is intended for the miner repository root. It
uses native GitHub-hosted runners for Linux x86_64, Linux ARM64, Windows x86_64,
macOS Apple Silicon, and macOS Intel. Every runner tests the workspace, builds
its own target, runs `nicechunk-miner self-test`, writes extracted-binary
`SHA256SUMS`, archives the files, and publishes an adjacent archive checksum.

Only a matching `v<package-version>` tag enters the publish job. The job
requires all five artifacts, verifies each sidecar, creates a static
`release-manifest.json`, and then creates the GitHub Release. The web page must
continue using `available:false` until that real manifest has been reviewed and
deployed. It never probes the GitHub API at runtime.

The dedicated source repository is `https://github.com/nicechunk/miner`.
Verified CLI archives have not been published yet, so the static release
manifest remains `available:false` and the page exposes no download button.

## Nginx integration

The reviewed location snippet is `nginx/miner-location.conf`. It expects:

```text
/web/nicechunk/miner/releases/<release-id>/
/web/nicechunk/miner/current -> releases/<release-id>
```

It does not change the website document root. It adds a 308 canonical redirect,
strict static 404 behavior, explicit WASM MIME, immutable hashed assets,
no-cache HTML/manifest, CSP, nosniff, Referrer-Policy, Permissions-Policy, and
same-origin resource policy. The existing server supplies gzip. The separate
Brotli snippet is used only when the installed Nginx actually has that module.
COOP/COEP are intentionally absent.

## Authorized production procedure

NiceChunk's recursive repository rules permit production mutation only through
`/web/solgame/scripts/sync-to-server.sh` with a release produced by the fixed
private full-site builder. The public miner tree must not contain the pinned
host/key, installer, server address, private allowlist, or an alternative
deployment client.

Before the first miner deployment, an administrator must separately review and
install the Nginx snippet and extend the private builder/installer for the
content-addressed miner release plus atomic `current` switch. This cannot be
bypassed with SSH, SCP, rsync, or a manual copy.

After that prerequisite exists, an authorized release follows the mandatory
full-site sequence: fixed builder, private safety suite, fixed client dry run,
review of release/manifest/archive identifiers and counts, then the exact
confirmation command printed by that dry run. The fixed installer must run
`nginx -t`, reload rather than restart, verify `/miner/` and every asset plus
existing `/` and `/play/`, and roll back all changed targets on any failure.

## Atomic rollback

The installer retains at least the previous complete release. Publishing creates
a new same-directory temporary symlink to `releases/<new-id>` and atomically
renames it over `current`. Rollback performs the same operation pointing to the
recorded previous ID; it never reconstructs files in place. Only after a later
successful release may an older, non-current, non-rollback release be removed.

`scripts/test-nginx-release.mjs` rehearses release one → release two → release
one while Nginx is serving requests. It proves both release directories remain,
checks `nginx -t`, and repeats all route/header/MIME/404/gzip checks before and
after rollback.

## Current publication state

The NiceChunk static-site build now publishes the verified `web/dist` output at
`/miner/` as part of the complete content-addressed website release. Missing
Miner assets return a strict 404 rather than the homepage. The standalone Nginx
snippet and rollback test remain useful for a future Miner-only release channel;
they are not a reason to bypass the complete-site release process.

The browser page and public source repository are available independently of a
CLI release. CLI controls remain unavailable until all platform archives,
checksums, and the reviewed static release manifest have actually been
published.
