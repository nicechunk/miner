import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { access, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { dirname, extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build as esbuildBuild } from "esbuild";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const web = resolve(root, "web");
const dist = resolve(web, "dist");
const assets = resolve(dist, "assets");
const build = resolve(web, ".build");
const chunkJsRoot = resolve(process.env.NICECHUNK_CHUNK_JS_ROOT || resolve(root, "..", "chunk.js"));
const cargoHome = resolve(root, ".toolchains", "cargo");
const rustupHome = resolve(root, ".toolchains", "rustup");
const localCargo = resolve(cargoHome, "bin", process.platform === "win32" ? "cargo.exe" : "cargo");
const localWasmBindgen = resolve(root, ".toolchains", "wasm-bindgen", "bin", process.platform === "win32" ? "wasm-bindgen.exe" : "wasm-bindgen");
const useLocalRust = await exists(localCargo);
const cargo = useLocalRust ? localCargo : (process.env.CARGO || "cargo");
const wasmBindgen = await exists(localWasmBindgen) ? localWasmBindgen : (process.env.WASM_BINDGEN || "wasm-bindgen");
const commandEnv = useLocalRust
  ? { ...process.env, CARGO_HOME: cargoHome, RUSTUP_HOME: rustupHome, PATH: `${resolve(cargoHome, "bin")}:${process.env.PATH || ""}` }
  : process.env;

execFileSync(process.execPath, [resolve(root, "scripts", "update-locales.mjs")], { cwd: root, env: commandEnv, stdio: "inherit" });
if (await exists(resolve(chunkJsRoot, "ncm", "blueprint-codec.js"))) {
  execFileSync(process.execPath, [resolve(root, "scripts", "generate-vectors.mjs")], { cwd: root, env: commandEnv, stdio: "inherit" });
} else {
  console.warn("Source JavaScript codecs are unavailable; using the committed audited test vectors.");
}
execFileSync(cargo, ["build", "-p", "pouw-wasm", "--target", "wasm32-unknown-unknown", "--release"], { cwd: root, env: commandEnv, stdio: "inherit" });

await rm(build, { recursive: true, force: true });
await rm(dist, { recursive: true, force: true });
await mkdir(resolve(build, "wasm"), { recursive: true });
await mkdir(assets, { recursive: true });

execFileSync(wasmBindgen, [
  resolve(root, "target", "wasm32-unknown-unknown", "release", "pouw_wasm.wasm"),
  "--target", "web",
  "--out-dir", resolve(build, "wasm"),
  "--out-name", "pouw_wasm",
  "--no-typescript",
], { cwd: root, env: commandEnv, stdio: "inherit" });

const wasmBytes = await readFile(resolve(build, "wasm", "pouw_wasm_bg.wasm"));
const wasmName = `pouw_wasm_bg.${hash(wasmBytes)}.wasm`;
await writeFile(resolve(assets, wasmName), wasmBytes);

let glue = await readFile(resolve(build, "wasm", "pouw_wasm.js"), "utf8");
glue = glue.replaceAll("pouw_wasm_bg.wasm", wasmName);
const glueName = `pouw_wasm.${hash(glue)}.js`;
await writeFile(resolve(assets, glueName), glue);

const sampleSources = {
  "terrain_delta:normal": "terrain_delta/normal-row.ncbk",
  "terrain_delta:complex": "terrain_delta/complex-cavity.ncbk",
  "terrain_delta:boundary": "terrain_delta/boundary.ncbk",
  "building:normal": "building/normal-box.ncm3",
  "building:complex": "building/complex-cottage.ncm3",
  "building:boundary": "building/boundary.ncm3",
  "forged_item:normal": "forged_item/normal-full.ncf1",
  "forged_item:complex": "forged_item/complex-painted-cavity.ncf1",
  "forged_item:boundary": "forged_item/boundary.ncf1",
};
const sampleUrls = {};
await mkdir(resolve(assets, "samples"), { recursive: true });
for (const [key, relative] of Object.entries(sampleSources)) {
  const bytes = await readFile(resolve(root, "test-vectors", relative));
  const extension = extname(relative);
  const base = relative.replaceAll("/", "-").slice(0, -extension.length);
  const name = `${base}.${hash(bytes)}${extension}`;
  await writeFile(resolve(assets, "samples", name), bytes);
  sampleUrls[key] = `./samples/${name}`;
}

const localeUrls = {};
await mkdir(resolve(assets, "locales"), { recursive: true });
for (const file of (await readdir(resolve(web, "locales"))).sort()) {
  const bytes = await readFile(resolve(web, "locales", file));
  const locale = file.slice(0, -5);
  const name = `${locale}.${hash(bytes)}.json`;
  await writeFile(resolve(assets, "locales", name), bytes);
  localeUrls[locale] = `./locales/${name}`;
}

const configBytes = await readFile(resolve(web, "site-config.json"));
const configName = `site-config.${hash(configBytes)}.json`;
await writeFile(resolve(assets, configName), configBytes);

const logoBytes = await readFile(resolve(web, "assets", "nicechunk-logo.png"));
const logoName = `nicechunk-logo.${hash(logoBytes)}.png`;
await writeFile(resolve(assets, logoName), logoBytes);

const workloadImageSources = {
  terrain: "terrain-workload.webp",
  building: "building-workload.webp",
  forged: "forged-workload.webp",
};
const workloadImageNames = {};
for (const [profile, sourceName] of Object.entries(workloadImageSources)) {
  const bytes = await readFile(resolve(web, "assets", sourceName));
  const outputName = `${sourceName.slice(0, -5)}.${hash(bytes)}.webp`;
  await writeFile(resolve(assets, outputName), bytes);
  workloadImageNames[profile] = outputName;
}

let worker = await readFile(resolve(web, "worker.js"), "utf8");
worker = worker.replace("__WASM_GLUE_URL__", `./${glueName}`);
const workerName = `worker.${hash(worker)}.js`;
await writeFile(resolve(assets, workerName), worker);

const sceneBundle = resolve(build, "miner-world-scene.js");
await access(resolve(chunkJsRoot, "renderer", "webgl2-renderer.js"));
await esbuildBuild({
  entryPoints: [resolve(web, "miner-world-scene.js")],
  outfile: sceneBundle,
  bundle: true,
  minify: true,
  treeShaking: true,
  legalComments: "none",
  charset: "ascii",
  format: "esm",
  platform: "browser",
  target: ["es2022"],
  alias: { "nicechunk-chunk-runtime": chunkJsRoot },
  loader: { ".ncm3": "text" },
  logLevel: "warning",
});
const scene = await readFile(sceneBundle, "utf8");
const sceneName = `miner-world-scene.${hash(scene)}.js`;
await writeFile(resolve(assets, sceneName), scene);

let i18n = await readFile(resolve(web, "i18n.js"), "utf8");
i18n = i18n.replace("__LOCALE_URLS__", JSON.stringify(localeUrls));
const i18nName = `i18n.${hash(i18n)}.js`;
await writeFile(resolve(assets, i18nName), i18n);

let app = await readFile(resolve(web, "app.js"), "utf8");
app = app
  .replace("__I18N_URL__", `./${i18nName}`)
  .replace("__WORKER_URL__", `./${workerName}`)
  .replace("__SCENE_URL__", `./${sceneName}`)
  .replace("__SITE_CONFIG_URL__", `./${configName}`)
  .replace("__SAMPLE_URLS__", JSON.stringify(sampleUrls));
const appName = `app.${hash(app)}.js`;
await writeFile(resolve(assets, appName), app);

const css = await readFile(resolve(web, "styles.css"));
const cssName = `styles.${hash(css)}.css`;
await writeFile(resolve(assets, cssName), css);

let html = await readFile(resolve(web, "index.html"), "utf8");
html = html
  .replace("__CSS_URL__", `./assets/${cssName}`)
  .replaceAll("__LOGO_URL__", `./assets/${logoName}`)
  .replaceAll("__TERRAIN_IMAGE_URL__", `./assets/${workloadImageNames.terrain}`)
  .replaceAll("__BUILDING_IMAGE_URL__", `./assets/${workloadImageNames.building}`)
  .replaceAll("__FORGED_IMAGE_URL__", `./assets/${workloadImageNames.forged}`)
  .replace("__APP_URL__", `./assets/${appName}`);
await writeFile(resolve(dist, "index.html"), html);

const release = JSON.parse(await readFile(resolve(web, "release-manifest.json"), "utf8"));
if (!release.available) {
  release.artifacts = [];
  release.releaseUrl = null;
  release.repository = null;
}
await writeFile(resolve(dist, "release-manifest.json"), `${JSON.stringify(release, null, 2)}\n`);

const manifest = {
  basePath: "/miner/",
  assets: {
    app: `assets/${appName}`,
    i18n: `assets/${i18nName}`,
    scene: `assets/${sceneName}`,
    worker: `assets/${workerName}`,
    wasmGlue: `assets/${glueName}`,
    wasm: `assets/${wasmName}`,
    css: `assets/${cssName}`,
    logo: `assets/${logoName}`,
    terrainImage: `assets/${workloadImageNames.terrain}`,
    buildingImage: `assets/${workloadImageNames.building}`,
    forgedImage: `assets/${workloadImageNames.forged}`,
    siteConfig: `assets/${configName}`,
  },
  samples: sampleUrls,
  locales: localeUrls,
};
await writeFile(resolve(dist, "asset-manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);

await rm(build, { recursive: true, force: true });
console.log(`Built NiceChunk miner web app in ${dist}`);

function hash(value) {
  return createHash("sha256").update(value).digest("hex").slice(0, 12);
}

async function exists(path) {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}
