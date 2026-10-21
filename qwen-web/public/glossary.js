// Glossary: one "term = translation" pair per line, e.g. "亦庄 = E-town". Either side may come first;
// direction is decided per target language by which side is Chinese.
const ZH = /[\u3400-\u9fff]/;

export function parseGlossary(text) {
  const pairs = [];
  for (const line of String(text || '').split('\n')) {
    const m = line.match(/^\s*(.+?)\s*(?:=|＝|→|->|\t)\s*(.+?)\s*$/);
    if (m && m[1] && m[2]) pairs.push([m[1], m[2]]);
  }
  return pairs.slice(0, 1000);
}

// Returns [source, target] for translating into `target` ('zh' or 'en'); other targets keep the written order.
function orient([a, b], target) {
  if (target === 'zh' && ZH.test(a) && !ZH.test(b)) return [b, a];
  if (target === 'en' && ZH.test(b) && !ZH.test(a)) return [b, a];
  return [a, b];
}

// Qwen session.translation.corpus.phrases: { sourceTerm: targetTranslation }.
export function qwenPhrases(pairs, target) {
  const phrases = {};
  for (const pair of pairs) { const [s, t] = orient(pair, target); phrases[s] = t; }
  return Object.keys(phrases).length ? phrases : null;
}

// Soniox context: both sides help recognition; translation terms follow the translation direction(s).
export function sonioxContext(pairs, target) {
  if (!pairs.length) return null;
  const terms = [...new Set(pairs.flat())];
  const targets = target === 'two_way' ? ['zh', 'en'] : [target];
  const seen = new Set(), translation_terms = [];
  for (const pair of pairs) for (const lang of targets) {
    const [source, t] = orient(pair, lang);
    if (!seen.has(source)) { seen.add(source); translation_terms.push({ source, target: t }); }
  }
  return { terms, translation_terms };
}
