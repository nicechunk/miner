import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const html = await readFile(resolve(root, "web", "index.html"), "utf8");
const localeDirectory = resolve(root, "web", "locales");
const localeOrder = ["en", "es", "fr", "de", "ja", "ru", "ko", "zh-Hant", "zh-Hans"];

const english = extractStaticMessages(html);
mergeInto(english, {
  actions: {
    copied: "Copied",
  },
  errors: {
    engine: "WASM engine error",
    generic: "The local mining engine stopped. Check the browser console for technical details and try again.",
    manifestHttp: "The release manifest returned HTTP {status}.",
    sampleHttp: "The built-in sample returned HTTP {status}.",
    sampleMissing: "No built-in vector is available for {key}.",
    stopped: "Stopped after an engine error.",
    title: "Error",
    workerFailed: "Worker {index} failed.",
    workerTimeout: "The WASM worker timed out.",
  },
  metrics: {
    exactMatch: "Exact Match",
    failed: "Failed",
  },
  release: {
    loadError: "The static release manifest could not be loaded: {error}",
    notPublished: "CLI release not published yet",
    notPublishedDetail: "No download button is shown until real GitHub Release assets and SHA-256 checksums exist.",
    unavailableTitle: "Release information unavailable",
    version: "NiceChunk Miner {version}",
    versions: "Protocol v{protocol} · VM v{vm}",
  },
  runtime: {
    engineReady: "Rust/WASM {software} ready · protocol {protocol} · VM {vm}",
    perSecond: "{rate}/s",
    seconds: "{seconds}s",
    workerMany: "{count} workers",
    workerOne: "{count} worker",
  },
  status: {
    exactNoImprovement: "Exact Match · no storage improvement",
    exactSmaller: "Exact Match · strictly smaller",
    hiddenPaused: "The page became hidden, so local CPU work was paused.",
    inputChanged: "Input changed.",
    inspecting: "Inspecting locally",
    inspectingDetail: "The Rust/WASM core is canonicalizing this asset.",
    loadFirst: "Load a sample or local file first.",
    mismatchDetail: "The verifier found {count} mismatches.",
    noImprovementDetail: "The verified candidate is equal to or larger than the incumbent. {suffix}",
    pageClosed: "The page was closed.",
    paused: "Paused",
    pausedDetail: "Workers are paused and can resume from their in-memory checkpoints.",
    profileChanged: "Profile changed.",
    readyLoaded: "Loaded and canonicalized {count} semantic entries.",
    reset: "Reset.",
    restarting: "Restarting search.",
    resumedDetail: "Workers resumed from their local checkpoints.",
    savedDetail: "Verified exact and saved {saved}. {suffix}",
    searching: "Searching locally",
    searchingDetail: "Typed worker islands are evaluating exact VM programs and canonical residuals.",
    stopped: "Stopped",
    stoppedByUser: "Stopped by user.",
    timeCompleted: "The configured time budget completed.",
    verificationFailed: "Verification failed",
  },
});

const languageNames = {
  de: "Deutsch",
  en: "English",
  es: "Español",
  fr: "Français",
  ja: "日本語",
  ko: "한국어",
  ru: "Русский",
  zhHans: "简体中文",
  zhHant: "繁體中文",
};

const overrides = {
  de: {
    nav: {
      home: "Home", roadbook: "Roadbook", worldRules: "Weltregeln", resources: "Ressourcen",
      ncm: "NCM", ncfm: "NCFM", elements: "Elemente", fairness: "Fairness",
      proofOfFrontier: "Beweis", seed: "Seed", guardians: "Wächter", contracts: "Verträge",
      civilization: "Zivilisation", trust: "Vertrauen", docs: "Docs", miner: "Miner",
      whitepaper: "Whitepaper", enterWorld: "Welt betreten", play: "Spielen",
    },
    hero: {
      lede: "Dieser Miner berechnet keine bedeutungslosen Hashes. Er sucht in einer festen, begrenzten Voxelsprache nach einer kürzeren, exakt gleichwertigen Darstellung.",
      try: "Browser-Demo testen",
      download: "CLI herunterladen",
    },
  },
  es: {
    nav: {
      home: "Inicio", roadbook: "Ruta", worldRules: "Reglas", resources: "Recursos",
      ncm: "NCM", ncfm: "NCFM", elements: "Elementos", fairness: "Equidad",
      proofOfFrontier: "Prueba", seed: "Semilla", guardians: "Guardianes", contracts: "Contratos",
      civilization: "Civilización", trust: "Confianza", docs: "Docs", miner: "Miner",
      whitepaper: "Whitepaper", enterWorld: "Entrar al mundo", play: "Jugar",
    },
    hero: {
      lede: "Este minero no calcula hashes sin utilidad. Busca en un lenguaje de vóxeles acotado una expresión más corta que reconstruya exactamente el mismo activo.",
      try: "Probar demo web",
      download: "Descargar CLI",
    },
  },
  fr: {
    nav: {
      home: "Accueil", roadbook: "Route", worldRules: "Règles", resources: "Ressources",
      ncm: "NCM", ncfm: "NCFM", elements: "Éléments", fairness: "Équité",
      proofOfFrontier: "Preuve", seed: "Graine", guardians: "Gardiens", contracts: "Contrats",
      civilization: "Civilisation", trust: "Confiance", docs: "Docs", miner: "Mineur",
      whitepaper: "Whitepaper", enterWorld: "Entrer dans le monde", play: "Jouer",
    },
    hero: {
      lede: "Ce mineur ne calcule pas de hachages inutiles. Il cherche, dans un langage voxel borné, une expression plus courte qui reconstruit exactement le même objet.",
      try: "Essayer la démo",
      download: "Télécharger le CLI",
    },
  },
  ja: {
    nav: {
      home: "ホーム", roadbook: "ロードブック", worldRules: "世界ルール", resources: "資源",
      ncm: "NCM", ncfm: "NCFM", elements: "元素", fairness: "公平性",
      proofOfFrontier: "証明", seed: "シード", guardians: "ガーディアン", contracts: "コントラクト",
      civilization: "文明", trust: "信頼", docs: "ドキュメント", miner: "マイナー",
      whitepaper: "Whitepaper", enterWorld: "ワールドへ入る", play: "プレイ",
    },
    hero: {
      lede: "無意味なハッシュを計算するのではなく、固定された有限のボクセル言語から、同じ資産を正確に復元する短い表現を探索します。",
      try: "ブラウザデモを試す",
      download: "CLI をダウンロード",
    },
  },
  ko: {
    nav: {
      home: "홈", roadbook: "로드북", worldRules: "월드 규칙", resources: "자원",
      ncm: "NCM", ncfm: "NCFM", elements: "원소", fairness: "공정성",
      proofOfFrontier: "증명", seed: "시드", guardians: "가디언", contracts: "컨트랙트",
      civilization: "문명", trust: "신뢰", docs: "문서", miner: "마이너",
      whitepaper: "Whitepaper", enterWorld: "월드 입장", play: "플레이",
    },
    hero: {
      lede: "무의미한 해시를 계산하지 않고, 제한된 복셀 언어에서 같은 자산을 정확히 복원하는 더 짧은 표현을 찾습니다.",
      try: "브라우저 데모",
      download: "CLI 다운로드",
    },
  },
  ru: {
    nav: {
      home: "Главная", roadbook: "План", worldRules: "Правила", resources: "Ресурсы",
      ncm: "NCM", ncfm: "NCFM", elements: "Элементы", fairness: "Честность",
      proofOfFrontier: "Доказательство", seed: "Сид", guardians: "Стражи", contracts: "Контракты",
      civilization: "Цивилизация", trust: "Доверие", docs: "Документы", miner: "Майнер",
      whitepaper: "Whitepaper", enterWorld: "Войти в мир", play: "Играть",
    },
    hero: {
      lede: "Майнер не перебирает бессмысленные хэши. Он ищет в ограниченном воксельном языке более короткое выражение, точно восстанавливающее тот же объект.",
      try: "Открыть демо",
      download: "Скачать CLI",
    },
  },
  "zh-Hans": {
    nav: {
      home: "首页", roadbook: "路书", worldRules: "世界规则", resources: "资源",
      ncm: "NCM", ncfm: "NCFM", elements: "元素", fairness: "公平性",
      proofOfFrontier: "证明", seed: "种子", guardians: "守护者", contracts: "合约",
      civilization: "文明", trust: "信任", docs: "文档", miner: "矿工",
      whitepaper: "白皮书", enterWorld: "进入世界", play: "开始游戏",
    },
    hero: {
      lede: "这个矿工不计算无意义哈希，而是在固定、有界的体素语言中搜索更短、且能精确还原同一资产的表达。",
      try: "体验浏览器演示",
      download: "下载 CLI",
    },
  },
  "zh-Hant": {
    nav: {
      home: "首頁", roadbook: "路書", worldRules: "世界規則", resources: "資源",
      ncm: "NCM", ncfm: "NCFM", elements: "元素", fairness: "公平性",
      proofOfFrontier: "證明", seed: "種子", guardians: "守護者", contracts: "合約",
      civilization: "文明", trust: "信任", docs: "文檔", miner: "礦工",
      whitepaper: "白皮書", enterWorld: "進入世界", play: "開始遊戲",
    },
    hero: {
      lede: "這個礦工不計算無意義雜湊，而是在固定、有界的體素語言中搜尋更短、且能精確還原同一資產的表達。",
      try: "體驗瀏覽器示範",
      download: "下載 CLI",
    },
  },
};

await mkdir(localeDirectory, { recursive: true });
for (const locale of localeOrder) {
  const messages = structuredClone(english);
  messages.languages = { ...languageNames };
  mergeInto(messages, overrides[locale] || {});
  await writeFile(resolve(localeDirectory, `${locale}.json`), `${JSON.stringify(messages, null, 2)}\n`);
}

console.log(`Generated ${localeOrder.length} complete miner locale catalogs`);

function extractStaticMessages(source) {
  const messages = {};
  const tagPattern = /<([a-z][a-z0-9-]*)\b([^>]*)>/giu;
  for (const match of source.matchAll(tagPattern)) {
    const [tagSource, tag, rawAttributes] = match;
    const attributes = Object.fromEntries([...rawAttributes.matchAll(/([a-z][a-z0-9:-]*)="([^"]*)"/giu)]
      .map((attribute) => [attribute[1], decodeHtml(attribute[2])]));
    if (attributes["data-i18n"]) {
      const afterTag = match.index + tagSource.length;
      const closing = source.indexOf(`</${tag}>`, afterTag);
      if (closing < 0) throw new Error(`Missing closing tag for ${attributes["data-i18n"]}`);
      const text = decodeHtml(source.slice(afterTag, closing).replace(/<[^>]*>/gu, "").replace(/\s+/gu, " ").trim());
      setNested(messages, attributes["data-i18n"], text);
    }
    for (const attribute of ["aria-label", "alt", "content", "placeholder", "title"]) {
      const key = attributes[`data-i18n-${attribute}`];
      if (key) setNested(messages, key, attributes[attribute] || "");
    }
  }
  return messages;
}

function decodeHtml(value) {
  return value.replace(/&(#x[0-9a-f]+|#\d+|amp|quot|apos|lt|gt);/giu, (entity, token) => {
    if (token[0] === "#") {
      const radix = token[1].toLowerCase() === "x" ? 16 : 10;
      const digits = radix === 16 ? token.slice(2) : token.slice(1);
      return String.fromCodePoint(Number.parseInt(digits, radix));
    }
    return { amp: "&", quot: "\"", apos: "'", lt: "<", gt: ">" }[token.toLowerCase()];
  });
}

function mergeInto(target, source) {
  for (const [key, value] of Object.entries(source)) {
    if (value && typeof value === "object" && !Array.isArray(value)) {
      target[key] ||= {};
      mergeInto(target[key], value);
    } else {
      target[key] = value;
    }
  }
  return target;
}

function setNested(target, path, value) {
  const keys = path.split(".");
  const final = keys.pop();
  let cursor = target;
  for (const key of keys) cursor = cursor[key] ||= {};
  if (final in cursor && cursor[final] !== value) throw new Error(`Conflicting locale value for ${path}`);
  cursor[final] = value;
}
