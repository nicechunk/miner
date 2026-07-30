import { execFileSync } from "node:child_process";
import { access, readFile, readdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const dist = resolve(root, "web", "dist");
const manifest = JSON.parse(await readFile(resolve(dist, "asset-manifest.json"), "utf8"));
const release = JSON.parse(await readFile(resolve(dist, "release-manifest.json"), "utf8"));
const html = await readFile(resolve(dist, "index.html"), "utf8");
const sourceApp = await readFile(resolve(root, "web", "app.js"), "utf8");
const sourceI18n = await readFile(resolve(root, "web", "i18n.js"), "utf8");
const sourceScene = await readFile(resolve(root, "web", "miner-world-scene.js"), "utf8");
const sourceStyles = await readFile(resolve(root, "web", "styles.css"), "utf8");
const sourceConfig = JSON.parse(await readFile(resolve(root, "web", "site-config.json"), "utf8"));
const expectedLocales = ["en", "es", "fr", "de", "ja", "ru", "ko", "zh-Hant", "zh-Hans"];

if (/__[A-Z0-9_]+__/u.test(html) || /(?:src|href)=["']\/assets\//u.test(html)) throw new Error("index.html contains an unresolved placeholder or root asset path");
if (/<(?:script|link)[^>]+https?:\/\//iu.test(html)) throw new Error("index.html includes a third-party script or stylesheet");
if (!/<button id="startButton"[^>]*\bdisabled\b/iu.test(html)) throw new Error("Miner Start must be disabled before inspection completes");
if (release.available === false && (release.artifacts.length || release.releaseUrl || release.repository)) {
  throw new Error("Unreleased manifest contains release links or artifacts");
}

const requiredNavPaths = [
  "/", "/roadmap/", "/world_rule/", "/resource_rule/", "/ncm/", "/ncfm/", "/elements/",
  "/fairness/", "/proof-of-frontier/", "/seed/", "/guardian/", "/contracts/", "/civilization/",
  "/trust/", "/docs/", "/miner/", "/whitepaper/", "/play/",
];
for (const path of requiredNavPaths) {
  if (!html.includes(`href="https://nicechunk.com${path}"`)) throw new Error(`Miner header is missing ${path}`);
}
if ((html.match(/<svg class="scene-icon /gu) || []).length !== 4) throw new Error("Scene dock must contain four inline SVG icons");
if (sourceStyles.includes(".scene-icon::before") || sourceStyles.includes(".scene-icon::after")) {
  throw new Error("Scene dock still uses generated CSS box icons");
}
if (!sourceScene.includes("NCM3:") || !sourceScene.includes("EQUIPMENT_MODEL_ID.forgedPickaxe")) {
  throw new Error("Chunk.js scene must embed the real cottage and game forged pickaxe");
}
if (!sourceScene.includes('"leftBlade"') || !sourceScene.includes('"rightBlade"')) {
  throw new Error("Forged item display must retain a recognizable two-sided pickaxe silhouette");
}
if (!html.includes('id="ncmPreviewCanvas"') || !sourceScene.includes("createNcmPreviewScene") || !sourceScene.includes("createCanonicalBuildingPlacement")) {
  throw new Error("Miner must render Rust/WASM canonical NCM semantics in the left 3D preview");
}
if (!sourceApp.includes("ncmPreviewScene.setInspection(state.inspect)")) {
  throw new Error("NCM preview must update from the inspected WASM result");
}
if (!sourceStyles.includes('.ncm-preview-frame[data-preview-state="ready"]')) {
  throw new Error("NCM preview is missing its rendered and fallback visual states");
}
if (/\bthree(?:\.module)?\b/iu.test(sourceScene)) throw new Error("Miner world scene must not depend on Three.js");
for (const [key, suffix] of Object.entries({ source: "", docs: "/tree/main/docs", issues: "/issues", releases: "/releases" })) {
  if (sourceConfig[key] !== `https://github.com/nicechunk/miner${suffix}`) throw new Error(`Miner ${key} URL is stale`);
}

for (const path of Object.values(manifest.assets)) await access(resolve(dist, path));
for (const path of Object.values(manifest.samples)) await access(resolve(dist, "assets", path.replace(/^\.\//u, "")));
for (const path of Object.values(manifest.locales)) await access(resolve(dist, "assets", path.replace(/^\.\//u, "")));

const localeNames = Object.keys(manifest.locales);
if (localeNames.length !== expectedLocales.length || expectedLocales.some((locale) => !localeNames.includes(locale))) {
  throw new Error(`Locale manifest must contain exactly: ${expectedLocales.join(", ")}`);
}
const optionOrder = [...html.matchAll(/<option\s+value="([^"]+)"[^>]*data-i18n="languages\./gu)].map((match) => match[1]);
if (JSON.stringify(optionOrder) !== JSON.stringify(expectedLocales)) {
  throw new Error(`Language selector order is invalid: ${optionOrder.join(", ")}`);
}

const catalogs = {};
for (const [locale, path] of Object.entries(manifest.locales)) {
  catalogs[locale] = JSON.parse(await readFile(resolve(dist, "assets", path.replace(/^\.\//u, "")), "utf8"));
}
const english = flattenStrings(catalogs.en);
for (const locale of expectedLocales) {
  const flattened = flattenStrings(catalogs[locale]);
  const missing = [...english.keys()].filter((key) => !flattened.has(key));
  const extra = [...flattened.keys()].filter((key) => !english.has(key));
  if (missing.length || extra.length) {
    throw new Error(`${locale} locale key mismatch; missing=${missing.join(",")} extra=${extra.join(",")}`);
  }
  for (const [key, englishValue] of english) {
    const value = flattened.get(key);
    if (!value.trim()) throw new Error(`${locale}.${key} is empty`);
    if (/NCK(?:TERM|VAR)|__NCK_/u.test(value)) throw new Error(`${locale}.${key} contains an unresolved translation token`);
    if (JSON.stringify(placeholders(value)) !== JSON.stringify(placeholders(englishValue))) {
      throw new Error(`${locale}.${key} has a placeholder mismatch`);
    }
  }
  if (locale !== "en") {
    const copied = [...english].filter(([key, value]) => !key.startsWith("languages.") && flattened.get(key) === value);
    if (copied.length > Math.ceil(english.size * 0.25)) {
      throw new Error(`${locale} still copies too much English text (${copied.length}/${english.size} keys)`);
    }
  }
}

const referencedKeys = new Set([
  ...[...html.matchAll(/data-i18n(?:-[a-z-]+)?="([^"]+)"/gu)].map((match) => match[1]),
  ...[...sourceApp.matchAll(/\bt\("([^"]+)"/gu)].map((match) => match[1]),
  "runtime.workerOne",
  "runtime.workerMany",
]);
const missingEnglishKeys = [...referencedKeys].filter((key) => !english.has(key));
if (missingEnglishKeys.length) throw new Error(`English locale is missing referenced keys: ${missingEnglishKeys.join(", ")}`);
if (/[^\x00-\x7f]*(?:[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}\p{Script=Cyrillic}])/u.test(await readFile(resolve(root, "web", "index.html"), "utf8"))) {
  throw new Error("Fallback HTML contains non-English script text outside locale catalogs");
}
if (!manifest.assets.i18n || !sourceApp.includes('from "__I18N_URL__"')) {
  throw new Error("Miner i18n must be emitted as an independent browser module");
}
if (sourceApp.includes("LOCALE_URLS") || sourceApp.includes('localStorage.setItem("nicechunk.language"')) {
  throw new Error("Miner app still owns locale loading or persistence instead of the i18n module");
}
if (!sourceI18n.includes('const STORAGE_KEY = "nicechunk.language"') || !sourceI18n.includes("nicechunk:minerlanguagechange")) {
  throw new Error("Miner i18n module is missing shared preference persistence or its locale event");
}

for (const name of (await readdir(resolve(dist, "assets"))).filter((value) => value.endsWith(".js"))) {
  execFileSync(process.execPath, ["--check", resolve(dist, "assets", name)], { stdio: "inherit" });
}

const allText = await Promise.all(Object.values(manifest.assets)
  .filter((path) => path.endsWith(".js") || path.endsWith(".json"))
  .map((path) => readFile(resolve(dist, path), "utf8")));
if (allText.some((value) => value.includes("api.github.com") || value.includes("google-analytics") || value.includes("googletagmanager"))) {
  throw new Error("Runtime assets contain a forbidden API or tracking endpoint");
}
if (allText.some((value) => /__[A-Z0-9_]+__/u.test(value))) throw new Error("Runtime assets contain unresolved placeholders");

console.log("NiceChunk miner static web checks passed");

function flattenStrings(value, prefix = "", output = new Map()) {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`Invalid locale object at ${prefix || "root"}`);
  for (const [key, child] of Object.entries(value)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof child === "string") output.set(path, child);
    else flattenStrings(child, path, output);
  }
  return output;
}

function placeholders(value) {
  return [...value.matchAll(/\{([A-Za-z0-9_]+)\}/gu)].map((match) => match[1]).sort();
}
