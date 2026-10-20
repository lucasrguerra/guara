#!/usr/bin/env node
// Confere se o dicionário inglês cobre os textos da interface.
//
// A varredura é estática e sem dependências: lê os arquivos da interface
// (JS e HTML), os rótulos que o núcleo em Rust envia para a tela (metadados,
// tipos de arquivo e saúde do disco) e falha quando encontra uma string que
// parece português e não tem tradução. Também recusa chaves duplicadas no
// dicionário (num objeto JS a última vence em silêncio).
//
// Uso: node scripts/check-ui-i18n.mjs

import { readFileSync, readdirSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(fileURLToPath(new URL('.', import.meta.url)), '..');
const rel = (p) => relative(root, p).replaceAll('\\', '/');

// ---------------------------------------------------------------- dicionário

const dictPath = join(root, 'apps/gui/ui/i18n-en.js');
const dictSource = readFileSync(dictPath, 'utf8');

const keyRe = /^\s*(['"])((?:(?!\1).)*)\1\s*:/gm;
const seen = new Map();
const duplicates = [];
let m;
while ((m = keyRe.exec(dictSource))) {
  const key = m[2];
  const line = dictSource.slice(0, m.index).split('\n').length;
  if (seen.has(key)) duplicates.push({ key, first: seen.get(key), again: line });
  else seen.set(key, line);
}

if (duplicates.length) {
  console.error('Chaves duplicadas em apps/gui/ui/i18n-en.js:');
  for (const d of duplicates) console.error(`  ${JSON.stringify(d.key)} (linhas ${d.first} e ${d.again})`);
  process.exitCode = 1;
}

const { EN } = await import(new URL('../apps/gui/ui/i18n-en.js', import.meta.url).href);

const exact = new Set(Object.keys(EN).filter((k) => !k.includes('{n}')));
const patterns = Object.keys(EN)
  .filter((k) => k.includes('{n}'))
  .map((k) => {
    const parts = k.split('{n}').map((p) => p.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
    return new RegExp('^' + parts.join('(.+?)') + '$', 's');
  });

function translated(text) {
  if (exact.has(text)) return true;
  return patterns.some((re) => re.test(text));
}

// ---------------------------------------------------------------- heurística

const ACCENT = /[áàâãäéèêëíìîïóòôõöúùûüçÁÀÂÃÄÉÈÊËÍÌÎÏÓÒÔÕÖÚÙÛÜÇ]/;
const WORDS = [
  'arquivo', 'arquivos', 'pasta', 'pastas', 'selecionado', 'selecionados', 'selecionada',
  'selecionadas', 'item', 'itens', 'abrir', 'abre', 'salvar', 'salvo', 'salvos', 'cancelar',
  'fechar', 'buscar', 'procurar', 'procurando', 'disco', 'discos', 'senha', 'chave', 'origem',
  'destino', 'tamanho', 'nome', 'nenhum', 'nenhuma', 'todos', 'todas', 'cada', 'esta', 'este',
  'essa', 'esse', 'para', 'com', 'sem', 'dos', 'das', 'por', 'que', 'não', 'sim', 'mais',
  'menos', 'uma', 'uns', 'umas', 'aqui', 'onde', 'quando', 'como', 'qual', 'confiança',
  'recuperar', 'recupera', 'recuperação', 'varredura', 'imagem', 'sessão', 'mês', 'dia',
  'hora', 'hoje', 'ontem', 'modificado', 'criado', 'acessado', 'tirada', 'foto', 'fotos',
  'documentos', 'vídeos', 'vídeo', 'áudio', 'compactados', 'banco', 'bancos', 'dados',
];

const BRAND = /Guará|Português|English|OpenStreetMap|BitLocker|FileVault|LUKS|NO\u00c7\u00d5ES|S\u00c3O/gi;
const isPortuguese = (raw) => {
  const s = raw.replace(BRAND, ' ');
  return ACCENT.test(s) || WORDS.some((w) => new RegExp(`(^|[^a-zà-ú])${w}([^a-zà-ú]|$)`, 'i').test(s));
};

const ALLOW = new Set([
  'Guará', 'Português', 'English', 'Auto', 'SMART', 'NVMe', 'OpenStreetMap', 'SHA-256',
  'SHA-1', 'MD5', 'AES', 'LUKS', 'BitLocker', 'FileVault', 'APFS', 'HFS+', 'ext4', 'Btrfs',
  'XFS', 'NTFS', 'FAT32', 'exFAT', 'ReFS', 'RAID', 'LVM', 'SQLite', 'CSV', 'HTML', 'PDF',
  'JPEG', 'PNG', 'GIF', 'BMP', 'WebP', 'MP4', 'MOV', 'E01', 'Ex01', 'AFF4', 'UTF-8',
  'UTF-16', 'USB', 'SSD', 'ATA', 'CRC', 'GPS', 'EXIF', 'TRIM', 'ZIP', 'TAR', 'OK',
]);
const ALLOW_PREFIX = ['http://', 'https://', 'data:', 'file:', 'guara:', '#', '.', '/'];
const ALLOW_EXACT = new Set([
  'sem data', 'no date', 'n/a', '—', '-', '·',
]);

function suspicious(text) {
  const s = text.replace(/\s+/g, ' ').trim();
  if (!s || s.length < 2) return false;
  if (ALLOW.has(s) || ALLOW_EXACT.has(s)) return false;
  if (ALLOW_PREFIX.some((p) => s.startsWith(p))) return false;
  if (!isPortuguese(s)) return false;
  // técnico: seletores, classes, extensões, expressões
  if (/^[.#\w-]+(\s*[,>+~]\s*[.#\w-]+)+$/.test(s)) return false;
  if (/^[a-z0-9_.:/@%{}$-]+$/i.test(s) && !ACCENT.test(s)) return false;
  return true;
}

// ---------------------------------------------------------------- extratores

function jsStrings(source, file, baseLine = 1) {
  const out = [];
  let i = 0;
  let line = baseLine;
  let prev = '';
  const skipRegex = () => {
    i++;
    let inClass = false;
    while (i < source.length) {
      const ch = source[i];
      if (ch === '\\') { i += 2; continue; }
      if (ch === '[') inClass = true;
      else if (ch === ']') inClass = false;
      else if (ch === '/' && !inClass) { i++; break; }
      else if (ch === '\n') line++;
      i++;
    }
    while (i < source.length && /[a-z]/i.test(source[i])) i++;
  };
  while (i < source.length) {
    const c = source[i];
    if (c === '\n') { line++; i++; prev = '\n'; continue; }
    if (c === ' ' || c === '\t' || c === '\r') { i++; continue; }
    if (c === '/' && source[i + 1] === '/') {
      while (i < source.length && source[i] !== '\n') i++;
      continue;
    }
    if (c === '/' && source[i + 1] === '*') {
      i += 2;
      while (i < source.length && !(source[i] === '*' && source[i + 1] === '/')) { if (source[i] === '\n') line++; i++; }
      i += 2;
      continue;
    }
    if (c === '/' && (/[=(:,[!&|?{};+\-*%^~<>]/.test(prev) || prev === 'return')) {
      skipRegex();
      prev = ')';
      continue;
    }
    if (c === "'" || c === '"') {
      const quote = c;
      const startLine = line;
      let value = '';
      i++;
      while (i < source.length) {
        const ch = source[i];
        if (ch === '\\') {
          const n = source[i + 1];
          if (n === 'n') value += '\n';
          else if (n === 't') value += '\t';
          else if (n === 'r') value += '\r';
          else if (n === '\n') line++;
          else value += n ?? '';
          i += 2;
          continue;
        }
        if (ch === quote) { i++; break; }
        if (ch === '\n') line++;
        value += ch;
        i++;
      }
      out.push({ text: value, file, line: startLine });
      prev = ')';
      continue;
    }
    if (c === '`') {
      const startLine = line;
      let value = '';
      i++;
      while (i < source.length) {
        const ch = source[i];
        if (ch === '\\') {
          const n = source[i + 1];
          if (n === 'n') value += '\n';
          else if (n === 't') value += '\t';
          else if (n === '\n') line++;
          else value += n ?? '';
          i += 2;
          continue;
        }
        if (ch === '`') { i++; break; }
        if (ch === '$' && source[i + 1] === '{') {
          value += '{n}';
          i += 2;
          const exprStart = i;
          const exprLine = line;
          let depth = 1;
          while (i < source.length && depth > 0) {
            if (source[i] === '{') depth++;
            else if (source[i] === '}') depth--;
            else if (source[i] === '\n') line++;
            i++;
          }
          out.push(...jsStrings(source.slice(exprStart, i - 1), file, exprLine));
          continue;
        }
        if (ch === '\n') line++;
        value += ch;
        i++;
      }
      out.push({ text: value, file, line: startLine });
      prev = ')';
      continue;
    }
    prev = c;
    i++;
  }
  return out;
}

function htmlStrings(source, file, baseLine = 1) {
  const out = [];
  const lineAt = (idx) => baseLine + source.slice(0, idx).split('\n').length - 1;
  // todo texto entre tags (inclusive começo e fim) — nós de texto reais
  let last = 0;
  for (const m of source.matchAll(/<[^>]*>/g)) {
    out.push({ text: source.slice(last, m.index), file, line: lineAt(last) });
    last = m.index + m[0].length;
  }
  out.push({ text: source.slice(last), file, line: lineAt(last) });
  // atributos traduzíveis (com aspas simples ou duplas)
  for (const m of source.matchAll(/\b(placeholder|title|aria-label|alt|data-empty)\s*=\s*(?:"([^"]*)"|'([^']*)')/g)) {
    out.push({ text: m[2] ?? m[3], file, line: lineAt(m.index) });
  }
  return out;
}

function rustLabels(source, file) {
  const out = [];
  const lineAt = (idx) => source.slice(0, idx).split('\n').length;
  // m.add("Rótulo", ...) e m.add(\n "Rótulo", ...)
  for (const m of source.matchAll(/\.add\(\s*"([^"]+)"/g)) out.push({ text: m[1], file, line: lineAt(m.index) });
  // m.kind = "Tipo".into()
  for (const m of source.matchAll(/\.kind\s*=\s*"([^"]+)"/g)) out.push({ text: m[1], file, line: lineAt(m.index) });
  // rótulos de listas (saúde do disco, tipos de arquivo) dentro de arrays de tuplas
  for (const m of source.matchAll(/\(\s*"([^"]{2,60})"\s*,\s*"(?:[^"]*)"\s*\)/g)) out.push({ text: m[1], file, line: lineAt(m.index) });
  return out;
}

// ---------------------------------------------------------------- coleta

const uiFiles = readdirSync(join(root, 'apps/gui/ui'))
  .filter((f) => f.endsWith('.js') && f !== 'i18n-en.js')
  .map((f) => join(root, 'apps/gui/ui', f));
uiFiles.push(join(root, 'apps/gui/ui/index.html'));

const rustFiles = [
  'crates/guara-core/src/meta.rs',
  'crates/guara-core/src/explorer.rs',
  'crates/guara-core/src/types.rs',
  'crates/guara-core/src/health.rs',
].map((f) => join(root, f));

const found = [];
for (const file of uiFiles) {
  const source = readFileSync(file, 'utf8');
  if (file.endsWith('.html')) found.push(...htmlStrings(source, file));
  else {
    for (const str of jsStrings(source, file)) {
      if (/<[a-z!/][^>]*>/i.test(str.text)) found.push(...htmlStrings(str.text, file, str.line));
      else found.push(str);
    }
  }
}
for (const file of rustFiles) found.push(...rustLabels(readFileSync(file, 'utf8'), file));

const missing = [];
const seenMissing = new Set();
for (const { text, file, line } of found) {
  if (!suspicious(text)) continue;
  const key = text.replace(/\s+/g, ' ').trim();
  if (translated(key)) continue;
  const id = `${key}\u0000${rel(file)}\u0000${line}`;
  if (seenMissing.has(id)) continue;
  seenMissing.add(id);
  missing.push({ text: key, file: rel(file), line });
}

if (missing.length) {
  console.error('Textos em português sem tradução em apps/gui/ui/i18n-en.js:\n');
  for (const x of missing) console.error(`  ${x.file}:${x.line}  ${JSON.stringify(x.text)}`);
  console.error(`\n${missing.length} texto(s) sem tradução.`);
  process.exitCode = 1;
}

if (!process.exitCode) {
  console.log(`i18n ok: ${found.length} textos verificados, ${Object.keys(EN).length} chaves no dicionário.`);
}
