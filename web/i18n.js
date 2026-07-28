const LOCALE_URLS = __LOCALE_URLS__;
const STORAGE_KEY = "nicechunk.language";
const DEFAULT_LOCALE = "en";
const TRANSLATED_ATTRIBUTES = ["aria-label", "alt", "content", "placeholder", "title"];

let activeLocale = DEFAULT_LOCALE;
let messages = Object.freeze({});

export async function initI18n() {
  return setLocale(resolveInitialLocale(), { persist: false });
}

export async function setLocale(requestedLocale, { persist = true } = {}) {
  const locale = normalizeLocale(requestedLocale);
  let catalog;
  let resolvedLocale = locale;

  try {
    catalog = await fetchCatalog(locale);
  } catch (error) {
    if (locale === DEFAULT_LOCALE) throw error;
    console.warn(`NiceChunk Miner locale ${locale} is unavailable; using English.`, error);
    catalog = await fetchCatalog(DEFAULT_LOCALE);
    resolvedLocale = DEFAULT_LOCALE;
  }

  messages = Object.freeze(catalog);
  activeLocale = resolvedLocale;
  document.documentElement.lang = resolvedLocale;
  applyTranslations(document);
  if (persist) localStorage.setItem(STORAGE_KEY, resolvedLocale);
  window.dispatchEvent(new CustomEvent("nicechunk:minerlanguagechange", {
    detail: Object.freeze({ language: resolvedLocale }),
  }));
  return resolvedLocale;
}

export function getLocale() {
  return activeLocale;
}

export function t(path, parameters = {}) {
  const value = readNested(messages, path);
  if (typeof value !== "string") return path;
  return value.replace(/\{([A-Za-z0-9_]+)\}/gu, (match, key) => (
    Object.hasOwn(parameters, key) ? String(parameters[key]) : match
  ));
}

export function applyTranslations(root = document) {
  root.querySelectorAll("[data-i18n]").forEach((element) => {
    const value = readNested(messages, element.dataset.i18n);
    if (typeof value === "string") element.textContent = value;
  });

  for (const attribute of TRANSLATED_ATTRIBUTES) {
    const dataName = `i18n${attribute.split("-").map(capitalize).join("")}`;
    root.querySelectorAll(`[data-i18n-${attribute}]`).forEach((element) => {
      const value = readNested(messages, element.dataset[dataName]);
      if (typeof value === "string") element.setAttribute(attribute, value);
    });
  }
}

function resolveInitialLocale() {
  const saved = localStorage.getItem(STORAGE_KEY);
  if (saved) return normalizeLocale(saved);
  return normalizeLocale(navigator.languages?.[0] || navigator.language || DEFAULT_LOCALE);
}

function normalizeLocale(value) {
  const requested = String(value || "").trim();
  if (LOCALE_URLS[requested]) return requested;

  const canonical = requested.replaceAll("_", "-");
  if (/^zh(?:-|$)/iu.test(canonical)) {
    return /(?:Hant|TW|HK|MO)(?:-|$)/iu.test(canonical) ? "zh-Hant" : "zh-Hans";
  }

  const base = canonical.split("-")[0].toLowerCase();
  return Object.keys(LOCALE_URLS).find((locale) => locale.toLowerCase() === base) || DEFAULT_LOCALE;
}

async function fetchCatalog(locale) {
  const url = LOCALE_URLS[locale];
  if (!url) throw new Error(`Unsupported Miner locale: ${locale}`);
  const response = await fetch(new URL(url, import.meta.url), { cache: "force-cache" });
  if (!response.ok) throw new Error(`Miner locale ${locale} returned HTTP ${response.status}`);
  const catalog = await response.json();
  if (!catalog || typeof catalog !== "object" || Array.isArray(catalog)) {
    throw new Error(`Miner locale ${locale} is not an object`);
  }
  return catalog;
}

function readNested(value, path) {
  return path.split(".").reduce((current, key) => current?.[key], value);
}

function capitalize(value) {
  return value[0].toUpperCase() + value.slice(1);
}
