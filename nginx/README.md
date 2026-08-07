# Nginx integration

`miner-location.conf` is a server-block snippet for the existing
`nicechunk.com` virtual host. It serves only `/miner/` from the independently
managed site directory:

```text
/web/nicechunk/miner/
  index.html
  asset-manifest.json
  release-manifest.json
  assets/
```

The snippet does not alter `/`, `/play/`, `/forging/`, Docs, TLS, or any other
site. `/miner` redirects with 308. HTML, `asset-manifest.json`, and
`release-manifest.json` are no-cache; content-hashed assets are immutable for
one year. Missing executable or JSON files return 404 and cannot reach the
homepage SPA fallback. WASM is explicitly `application/wasm` and every response
uses `nosniff`.

The parent server must already include `/etc/nginx/mime.types` and gzip for JS,
JSON, CSS, WASM, SVG, and text, as the audited NiceChunk server does. Include
`brotli-optional.conf` only if `nginx -V` confirms Brotli support and it is not
already configured.

## Required administrator review

NiceChunk policy permits production mutation only through the private fixed
site-directory deployment client. This public miner tree intentionally contains
no SSH key, host, installer, or server-deployment script. An administrator must:

1. review and install the location snippet in the existing server block;
2. keep the `/miner/` URL bound to `/web/nicechunk/miner/`, which is the fixed
   destination of a metadata-bound `site-directory` release;
3. preserve the installer's managed-file manifest and rollback snapshot;
4. run `nginx -t` and reload (never restart) only after it succeeds;
5. smoke-test `/miner/`, every manifest asset, a missing JS/WASM 404, `/`, and
   `/play/` through the fixed deploy client.

The fixed installer atomically replaces each managed file, verifies every file
through local HTTPS, and restores the complete preceding managed set if any
check fails. `node scripts/test-nginx-release.mjs` performs an offline publish,
rollback, config test, static HTTP smoke, MIME/cache/header check, and existing
route isolation test. It never connects to or mutates production.
