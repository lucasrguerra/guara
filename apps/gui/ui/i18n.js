// Idioma da interface. O texto-fonte é em português; no modo inglês um tradutor
// troca os textos exibidos (nós de texto e atributos) usando um dicionário, e
// continua observando a página para traduzir o que for desenhado depois.
// Textos com valores variáveis usam {n} no dicionário (ex.: "{n} arquivos" -> "{n} files").

import { EN } from './i18n-en.js';

let lang = 'pt';
export const isEn = () => lang === 'en';
export const currentLang = () => lang;
export const LOC = () => (lang === 'en' ? 'en-US' : 'pt-BR');

export function resolveLang(pref) {
  if (pref === 'pt' || pref === 'en') return pref;
  return (navigator.language || 'pt').toLowerCase().startsWith('pt') ? 'pt' : 'en';
}

const ATTRS = ['placeholder', 'title', 'aria-label', 'alt', 'data-empty'];
// Dados do usuário (nomes, caminhos, conteúdo) nunca são traduzidos.
const SKIP = '.xp-cap, .xp-lnm, .db-table, .pv-zip, .hit, pre, code, .mono, .xp-ititle b, dd, textarea, input, script, style';

const exact = new Map(Object.entries(EN).filter(([k]) => !k.includes('{n}')));
const patterns = Object.entries(EN)
  .filter(([k]) => k.includes('{n}'))
  .map(([k, v]) => {
    const parts = k.split('{n}').map((p) => p.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
    return { re: new RegExp('^' + parts.join('(.+?)') + '$', 's'), to: v };
  })
  // padrões mais específicos (mais texto fixo) primeiro
  .sort((a, b) => b.re.source.length - a.re.source.length);

const cache = new Map();
export function tr(text) {
  if (!isEn()) return text;
  const lead = text.match(/^\s*/)[0];
  const trail = text.match(/\s*$/)[0];
  const core = text.trim();
  if (!core) return text;
  if (cache.has(core)) return lead + cache.get(core) + trail;
  let out = exact.get(core);
  if (out === undefined) {
    for (const p of patterns) {
      const m = core.match(p.re);
      if (m) {
        let i = 0;
        // os valores capturados também podem ter partes traduzíveis (ex.: nomes de tipos)
        out = p.to.replace(/\{n\}/g, () => tr(m[++i] ?? ''));
        break;
      }
    }
  }
  if (out === undefined) out = core;
  if (cache.size > 5000) cache.clear();
  cache.set(core, out);
  return lead + out + trail;
}

const orig = new WeakMap(); // nó -> texto original em português
let observer = null;

function translateNode(n) {
  if (n.nodeType === 3) {
    if (n.parentElement?.closest(SKIP)) return;
    const o = orig.get(n) ?? n.nodeValue;
    const t = tr(o);
    if (t !== n.nodeValue) {
      if (!orig.has(n)) orig.set(n, o);
      n.nodeValue = t;
    }
  } else if (n.nodeType === 1) {
    if (n.matches?.(SKIP) && n.tagName !== 'INPUT') return;
    for (const a of ATTRS) {
      if (n.hasAttribute?.(a)) {
        const key = `i18n-${a}`;
        const o = n[key] ?? n.getAttribute(a);
        const t = tr(o);
        if (t !== n.getAttribute(a)) { n[key] = o; n.setAttribute(a, t); }
      }
    }
    for (const c of n.childNodes) translateNode(c);
  }
}

function restoreAll(root) {
  const w = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  let n;
  while ((n = w.nextNode())) { if (orig.has(n)) { n.nodeValue = orig.get(n); orig.delete(n); } }
  root.querySelectorAll('*').forEach((el) => {
    for (const a of ATTRS) { const key = `i18n-${a}`; if (el[key] !== undefined) { el.setAttribute(a, el[key]); delete el[key]; } }
  });
}

function start() {
  translateNode(document.body);
  document.title = tr(document.title);
  if (observer) return;
  observer = new MutationObserver((muts) => {
    observer.disconnect();
    for (const m of muts) {
      if (m.type === 'childList') m.addedNodes.forEach(translateNode);
      else if (m.type === 'characterData') { orig.delete(m.target); translateNode(m.target); }
      else if (m.type === 'attributes') { delete m.target[`i18n-${m.attributeName}`]; translateNode(m.target); }
    }
    observer.observe(document.body, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ATTRS });
  });
  observer.observe(document.body, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ATTRS });
}

function stop() {
  observer?.disconnect();
  observer = null;
  restoreAll(document.body);
}

export function setLang(pref) {
  const next = resolveLang(pref);
  const changed = next !== lang;
  lang = next;
  document.documentElement.lang = lang === 'en' ? 'en' : 'pt-BR';
  if (lang === 'en') start();
  else if (changed || observer) stop();
}
