import { LOC } from './i18n.js';
// Pré-visualização de arquivos recuperados.
// O conteúdo é lido em streaming do núcleo (com Range) pela URL `url`; nada é
// gravado em disco. Tipos: imagem, vídeo, áudio, PDF (pdf.js), texto, documentos
// Office/OpenDocument (texto extraído), listagem de ZIP e visão hexadecimal.

const IMG = new Set(['jpg', 'jpeg', 'png', 'gif', 'webp', 'bmp', 'avif', 'ico']);
const VID = new Set(['mp4', 'm4v', 'mov', 'webm', 'mkv', 'ogv']);
const AUD = new Set(['mp3', 'wav', 'ogg', 'opus', 'm4a', 'aac', 'flac']);
const TXT = new Set(['txt', 'md', 'csv', 'tsv', 'json', 'xml', 'html', 'htm', 'log', 'ini', 'yml', 'yaml', 'js', 'css', 'c', 'h', 'cpp', 'py', 'rs', 'sh', 'bat', 'sql', 'toml', 'cfg', 'conf', 'srt', 'tex', 'svg']);
const OFFICE = new Set(['docx', 'xlsx', 'pptx', 'odt', 'ods', 'odp']);
const ZIPS = new Set(['zip', 'jar', 'apk', 'epub']);
const DBS = new Set(['db', 'sqlite', 'sqlite3', 'db3', 'sdb', 'sqlitedb', 'storedata']);

export function kindOf(item) {
  const e = (item.ext || '').toLowerCase();
  if (IMG.has(e)) return 'image';
  if (VID.has(e)) return 'video';
  if (AUD.has(e)) return 'audio';
  if (e === 'pdf') return 'pdf';
  if (DBS.has(e)) return 'database';
  if (OFFICE.has(e)) return 'office';
  if (ZIPS.has(e)) return 'zip';
  if (TXT.has(e)) return 'text';
  return 'hex';
}

export const KIND_LABEL = {
  image: 'Imagem', video: 'Vídeo', audio: 'Áudio', pdf: 'PDF', office: 'Documento (texto extraído)',
  zip: 'Conteúdo do arquivo compactado', database: 'Banco de dados SQLite', text: 'Texto', hex: 'Dados brutos (hexadecimal)',
};

const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const human = (n) => {
  const u = ['B', 'KB', 'MB', 'GB'];
  let i = 0, v = n;
  while (v >= 1024 && i < 3) { v /= 1024; i++; }
  return i ? `${v.toFixed(1)} ${u[i]}` : `${n} B`;
};

async function fetchRange(url, a, b) {
  const r = await fetch(url, { headers: { Range: `bytes=${a}-${b}` } });
  if (!r.ok) throw new Error(`leitura falhou (${r.status})`);
  return new Uint8Array(await r.arrayBuffer());
}

// ------------------------------------------------------------------ ZIP mínimo
async function zipDir(url, size) {
  const tailLen = Math.min(size, 66000);
  const tail = await fetchRange(url, size - tailLen, size - 1);
  const dv = new DataView(tail.buffer);
  let e = -1;
  for (let i = tail.length - 22; i >= 0; i--) if (dv.getUint32(i, true) === 0x06054b50) { e = i; break; }
  if (e < 0) throw new Error('diretório do ZIP não encontrado (arquivo possivelmente incompleto)');
  const n = dv.getUint16(e + 10, true);
  const cdSize = dv.getUint32(e + 12, true);
  const cdOff = dv.getUint32(e + 16, true);
  if (cdOff + cdSize > size) throw new Error('diretório do ZIP inconsistente');
  const cd = await fetchRange(url, cdOff, cdOff + cdSize - 1);
  const cv = new DataView(cd.buffer);
  const out = [];
  let p = 0;
  for (let i = 0; i < n && p + 46 <= cd.length; i++) {
    if (cv.getUint32(p, true) !== 0x02014b50) break;
    const nl = cv.getUint16(p + 28, true), xl = cv.getUint16(p + 30, true), cl = cv.getUint16(p + 32, true);
    out.push({
      method: cv.getUint16(p + 10, true), csize: cv.getUint32(p + 20, true), usize: cv.getUint32(p + 24, true),
      off: cv.getUint32(p + 42, true), name: new TextDecoder().decode(cd.subarray(p + 46, p + 46 + nl)),
    });
    p += 46 + nl + xl + cl;
  }
  return out;
}

async function zipRead(url, e, cap = 8 << 20) {
  if (e.usize > cap) throw new Error('parte grande demais');
  const lh = await fetchRange(url, e.off, e.off + 29);
  const dv = new DataView(lh.buffer);
  const start = e.off + 30 + dv.getUint16(26, true) + dv.getUint16(28, true);
  const data = e.csize ? await fetchRange(url, start, start + e.csize - 1) : new Uint8Array(0);
  if (e.method === 0) return data;
  if (e.method !== 8) throw new Error('compressão não suportada');
  const stream = new Blob([data]).stream().pipeThrough(new DecompressionStream('deflate-raw'));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

const xml = (bytes) => new DOMParser().parseFromString(new TextDecoder().decode(bytes), 'application/xml');

// ------------------------------------------------------------------ renderizadores
function note(host, html, cls = '') {
  host.insertAdjacentHTML('beforeend', `<div class="pv-note ${cls}">${html}</div>`);
}

function failure(host, item, err, withHex) {
  host.innerHTML = '';
  note(host, `<b>Não foi possível pré-visualizar.</b><br>${esc(err?.message || err || '')} O arquivo pode estar danificado, incompleto ou parcialmente sobrescrito, o que também indica que a chance de recuperação é menor.`, 'bad');
  return withHex ? hex(host, item, host.dataset.url, false) : null;
}

async function hex(host, item, url, large) {
  const n = Math.min(item.size, large ? 4096 : 1024);
  if (!n) { note(host, 'Arquivo vazio.'); return; }
  const b = await fetchRange(url, 0, n - 1);
  const lines = [];
  for (let i = 0; i < b.length; i += 16) {
    const row = [...b.subarray(i, i + 16)];
    const h = row.map((x) => x.toString(16).padStart(2, '0')).join(' ').padEnd(47);
    const a = row.map((x) => (x >= 32 && x < 127 ? String.fromCharCode(x) : '·')).join('');
    lines.push(`<span class="o">${i.toString(16).padStart(6, '0')}</span>  ${h}  <span class="a">${esc(a)}</span>`);
  }
  host.insertAdjacentHTML('beforeend', `<pre class="pv-hex">${lines.join('\n')}</pre>`);
  if (item.size > n) note(host, `Mostrando os primeiros ${human(n)} de ${human(item.size)}.`);
}

async function text(host, item, url, large) {
  const cap = large ? 262144 : 24576;
  const b = await fetchRange(url, 0, Math.min(item.size, cap) - 1);
  const sample = b.subarray(0, 2048);
  const bin = sample.filter((x) => x === 0 || (x < 9) ).length;
  if (bin > sample.length * 0.02) { note(host, 'Este arquivo não parece ser texto.'); return hex(host, item, url, large); }
  let s = new TextDecoder('utf-8', { fatal: false }).decode(b);
  if ((s.match(/�/g) || []).length > s.length * 0.02) s = new TextDecoder('windows-1252').decode(b);
  host.insertAdjacentHTML('beforeend', `<pre class="pv-text">${esc(s)}</pre>`);
  if (item.size > b.length) note(host, `Mostrando o início (${human(b.length)} de ${human(item.size)}).`);
}

async function image(host, item, url, state, thumb) {
  await new Promise((resolve) => {
    const img = new Image();
    img.alt = `Pré-visualização de ${item.name}`;
    img.className = 'pv-img';
    img.decoding = 'async';
    img.onload = () => {
      if (state.dead) return resolve();
      host.innerHTML = '';
      host.appendChild(img);
      if (!thumb) {
        host.classList.add('zoomable');
        img.onclick = () => host.classList.toggle('actual');
      }
      if (!thumb) note(host, `${img.naturalWidth} × ${img.naturalHeight} px. Clique para alternar entre ajustar e tamanho real.`);
      resolve();
    };
    img.onerror = () => { if (!state.dead) failure(host, item, 'A imagem não pôde ser decodificada.', true); resolve(); };
    img.src = url;
  });
}

function media(host, item, url, tag, state) {
  const el = document.createElement(tag);
  el.controls = true;
  el.preload = 'metadata';
  el.className = `pv-${tag}`;
  if (tag === 'video') el.playsInline = true;
  el.onerror = () => { if (!state.dead) failure(host, item, `Não foi possível reproduzir este ${tag === 'video' ? 'vídeo' : 'áudio'}.`, true); };
  el.src = url;
  host.innerHTML = '';
  host.appendChild(el);
  state.cleanup.push(() => { el.pause?.(); el.removeAttribute('src'); el.load?.(); });
}

let pdfjsPromise;
function loadPdfjs() {
  pdfjsPromise ??= import('./vendor/pdf.min.mjs').then((m) => {
    m.GlobalWorkerOptions.workerSrc = new URL('./vendor/pdf.worker.min.mjs', import.meta.url).href;
    return m;
  });
  return pdfjsPromise;
}

async function pdf(host, item, url, state, large) {
  const pdfjs = await loadPdfjs();
  const task = pdfjs.getDocument({ url, disableAutoFetch: true, isEvalSupported: false });
  state.cleanup.push(() => task.destroy());
  const doc = await task.promise;
  if (state.dead) return;
  host.innerHTML = '<div class="pv-pdfbar"><button class="btn sm" data-d="-1" aria-label="Página anterior">‹</button><span class="pg"></span><button class="btn sm" data-d="1" aria-label="Próxima página">›</button></div><div class="pv-pdfwrap"><canvas class="pv-canvas"></canvas></div>';
  const canvas = host.querySelector('canvas');
  const label = host.querySelector('.pg');
  let n = 1, busy = false;
  async function draw() {
    if (busy || state.dead) return;
    busy = true;
    try {
      const page = await doc.getPage(n);
      const base = page.getViewport({ scale: 1 });
      const width = Math.max(240, (host.clientWidth || 320) - 16);
      const scale = (width / base.width) * (large ? 1 : 1);
      const dpr = window.devicePixelRatio || 1;
      const vp = page.getViewport({ scale: scale * dpr });
      canvas.width = vp.width; canvas.height = vp.height;
      canvas.style.width = `${vp.width / dpr}px`;
      await page.render({ canvasContext: canvas.getContext('2d'), viewport: vp }).promise;
      label.textContent = `Página ${n} de ${doc.numPages}`;
    } catch (e) {
      if (!state.dead) { host.innerHTML = ''; failure(host, item, e, true); }
    }
    busy = false;
  }
  host.querySelectorAll('[data-d]').forEach((b) => (b.onclick = () => { n = Math.min(doc.numPages, Math.max(1, n + +b.dataset.d)); draw(); }));
  await draw();
}

const colIndex = (ref) => [...ref.replace(/\d+/g, '')].reduce((a, c) => a * 26 + c.charCodeAt(0) - 64, 0) - 1;
const colName = (i) => { let s = ''; i++; while (i > 0) { s = String.fromCharCode(65 + ((i - 1) % 26)) + s; i = Math.floor((i - 1) / 26); } return s; };

async function office(host, item, url, state) {
  const ents = await zipDir(url, item.size);
  const find = (n) => ents.find((e) => e.name === n);
  const ext = item.ext.toLowerCase();
  let html = '';
  if (ext === 'docx') {
    const d = xml(await zipRead(url, find('word/document.xml')));
    const paras = [...d.getElementsByTagName('w:p')].slice(0, 400).map((p) => {
      let t = '';
      p.querySelectorAll('t, tab, br').forEach((n) => { t += n.localName === 't' ? n.textContent : n.localName === 'tab' ? '\t' : '\n'; });
      const h = p.querySelector('pStyle')?.getAttribute('w:val') || '';
      const tag = /^(Heading|Ttulo|Title)/i.test(h) ? 'h4' : 'p';
      return t.trim() ? `<${tag}>${esc(t)}</${tag}>` : '';
    });
    html = `<div class="pv-page">${paras.join('') || '<p class="sub">Sem texto.</p>'}</div>`;
  } else if (ext === 'odt') {
    const d = xml(await zipRead(url, find('content.xml')));
    const els = [...d.getElementsByTagName('text:p'), ...[]].slice(0, 400);
    html = `<div class="pv-page">${els.map((p) => (p.textContent.trim() ? `<p>${esc(p.textContent)}</p>` : '')).join('') || '<p class="sub">Sem texto.</p>'}</div>`;
  } else if (ext === 'xlsx') {
    const ss = find('xl/sharedStrings.xml');
    const strings = ss ? [...xml(await zipRead(url, ss)).getElementsByTagName('si')].map((si) => si.textContent) : [];
    const sheet = ents.filter((e) => /^xl\/worksheets\/sheet\d+\.xml$/.test(e.name)).sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }))[0];
    const d = xml(await zipRead(url, sheet));
    const rows = [...d.getElementsByTagName('row')].slice(0, 80);
    let maxc = 0;
    const grid = rows.map((r) => {
      const cells = {};
      [...r.getElementsByTagName('c')].forEach((c) => {
        const i = colIndex(c.getAttribute('r') || 'A1');
        const v = c.getElementsByTagName('v')[0]?.textContent ?? c.getElementsByTagName('t')[0]?.textContent ?? '';
        cells[i] = c.getAttribute('t') === 's' ? strings[+v] ?? '' : v;
        maxc = Math.max(maxc, i + 1);
      });
      return [r.getAttribute('r'), cells];
    });
    maxc = Math.min(maxc, 16);
    html = `<div class="pv-sheet"><table><thead><tr><th></th>${Array.from({ length: maxc }, (_, i) => `<th>${colName(i)}</th>`).join('')}</tr></thead><tbody>${grid
      .map(([n, cells]) => `<tr><th>${esc(n)}</th>${Array.from({ length: maxc }, (_, i) => `<td>${esc(cells[i] ?? '')}</td>`).join('')}</tr>`).join('')}</tbody></table></div>`;
  } else if (ext === 'ods') {
    const d = xml(await zipRead(url, find('content.xml')));
    const rows = [...d.getElementsByTagName('table:table-row')].slice(0, 80);
    html = `<div class="pv-sheet"><table><tbody>${rows.map((r) => `<tr>${[...r.getElementsByTagName('table:table-cell')].slice(0, 16).map((c) => `<td>${esc(c.textContent)}</td>`).join('')}</tr>`).join('')}</tbody></table></div>`;
  } else {
    // pptx / odp: texto por slide
    let slides;
    if (ext === 'pptx') {
      const files = ents.filter((e) => /^ppt\/slides\/slide\d+\.xml$/.test(e.name)).sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true })).slice(0, 60);
      slides = [];
      for (const f of files) {
        const d = xml(await zipRead(url, f));
        slides.push([...d.getElementsByTagName('a:p')].map((p) => p.textContent).filter((t) => t.trim()));
      }
    } else {
      const d = xml(await zipRead(url, find('content.xml')));
      slides = [...d.getElementsByTagName('draw:page')].slice(0, 60).map((pg) => [...pg.getElementsByTagName('text:p')].map((p) => p.textContent).filter((t) => t.trim()));
    }
    html = slides.map((s, i) => `<div class="pv-slide"><small>Slide ${i + 1}</small>${s.map((t, j) => (j === 0 ? `<h4>${esc(t)}</h4>` : `<p>${esc(t)}</p>`)).join('') || '<p class="sub">Sem texto.</p>'}</div>`).join('');
  }
  if (state.dead) return;
  host.innerHTML = html;
  note(host, 'Texto extraído do documento. A formatação, as imagens e os gráficos originais não são exibidos.');
}

async function zip(host, item, url, state) {
  const ents = await zipDir(url, item.size);
  if (state.dead) return;
  host.innerHTML = `<ul class="pv-zip">${ents.slice(0, 400).map((e) => `<li><span>${esc(e.name)}</span><small>${e.name.endsWith('/') ? 'pasta' : human(e.usize)}</small></li>`).join('')}</ul>`;
  note(host, `${ents.length} item(ns) no arquivo compactado.`);
}

// ------------------------------------------------------------------ bancos de dados (SQLite)

/** Valores inteiros que parecem datas (Unix, Chrome/WebKit, macOS) viram uma dica legível. */
function tsHint(v) {
  if (!Number.isInteger(v)) return null;
  let ms = null, kind = '';
  if (v > 9.4e8 && v < 4.2e9) { ms = v * 1000; kind = 'Unix (s)'; }
  else if (v > 9.4e11 && v < 4.2e12) { ms = v; kind = 'Unix (ms)'; }
  else if (v > 9.4e14 && v < 4.2e15) { ms = v / 1000; kind = 'Unix (µs)'; }
  else if (v > 1.2e16 && v < 1.4e16) { ms = v / 1000 - 11644473600000; kind = 'Chrome/WebKit'; }
  else if (v > 3e8 && v < 9.4e8) { ms = (v + 978307200) * 1000; kind = 'macOS/CoreData'; }
  if (ms == null) return null;
  const d = new Date(ms);
  return Number.isNaN(d.getTime()) ? null : { kind, text: d.toISOString().replace('T', ' ').slice(0, 19) + ' UTC' };
}

function cellHtml(c) {
  switch (c.t) {
    case 'null': return '<span class="db-null">NULL</span>';
    case 'int': { const h = tsHint(c.v); return h ? `<span title="${esc(h.kind)}: ${esc(h.text)}">${c.v}<i class="db-ts">${esc(h.text.slice(0, 10))}</i></span>` : String(c.v); }
    case 'real': return String(c.v);
    case 'text': return `<span title="${esc(c.v.slice(0, 600))}">${esc(c.v.length > 240 ? c.v.slice(0, 240) + '…' : c.v)}</span>`;
    default: return `<span class="db-blob" title="${esc(c.v.head)}">BLOB ${c.v.mime ? esc(c.v.mime) + ' ' : ''}${human(c.v.len)}</span>`;
  }
}

async function database(host, item, state, large, db, onOpen) {
  if (!db) throw new Error('visualizador indisponível');
  const info = await db.info();
  if (state.dead) return;
  const tables = info.tables;
  const warn = (info.warnings || []).map((w) => `<div class="pv-note bad">${esc(w)}</div>`).join('');
  if (!large) {
    const total = tables.reduce((a, t) => a + (t.rows || 0), 0);
    host.innerHTML = `<div class="db-sum"><b>Banco SQLite</b><span class="sub">${tables.length} tabela(s) · ${total.toLocaleString(LOC())} linhas · páginas de ${human(info.page_size)}</span>
      <ul>${tables.slice(0, 10).map((t) => `<li><span>${esc(t.name)}</span><small>${(t.rows || 0).toLocaleString(LOC())}</small></li>`).join('')}</ul>
      ${tables.length > 10 ? `<span class="sub">e mais ${tables.length - 10}…</span>` : ''}${warn}
      <button class="btn primary sm" data-pv="open">Abrir o visualizador</button></div>`;
    host.onclick = (e) => { if (e.target.closest('[data-pv]')) onOpen?.(); };
    return;
  }
  const S = { table: tables.find((t) => t.rows > 0)?.name || tables[0]?.name, offset: 0, limit: 100, deleted: false, q: '', schema: false };
  host.classList.add('db-large');
  host.innerHTML = `<div class="db-wrap"><nav class="db-tables" aria-label="Tabelas"></nav><section class="db-main"><div class="db-bar"></div><div class="db-grid"></div><div class="db-foot"></div></section></div>`;
  const $ = (sel) => host.querySelector(sel);
  const nav = $('.db-tables'), bar = $('.db-bar'), grid = $('.db-grid'), foot = $('.db-foot');
  nav.innerHTML = tables.map((t) => `<button class="db-t" data-t="${esc(t.name)}"><span>${esc(t.name)}</span><small>${(t.rows || 0).toLocaleString(LOC())}</small></button>`).join('')
    + (info.indexes.length || info.views.length ? `<div class="db-meta">${info.views.length ? `<b>Views</b>${info.views.map((v) => `<span>${esc(v)}</span>`).join('')}` : ''}${info.indexes.length ? `<b>Índices</b>${info.indexes.map((v) => `<span>${esc(v)}</span>`).join('')}` : ''}</div>` : '')
    + warn;

  async function load() {
    if (state.dead) return;
    nav.querySelectorAll('.db-t').forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.t === S.table)));
    const t = tables.find((x) => x.name === S.table);
    if (!t) { grid.innerHTML = '<div class="pv-note">Nenhuma tabela.</div>'; return; }
    bar.innerHTML = `<b>${esc(t.name)}</b><span class="sub">${(t.rows || 0).toLocaleString(LOC())} linhas · ${t.columns.length} colunas</span>
      <input class="input" id="dbq" type="search" placeholder="Filtrar esta página" value="${esc(S.q)}" aria-label="Filtrar">
      <label class="check" style="padding:0"><input type="checkbox" id="dbdel" ${S.deleted ? 'checked' : ''}><span>Registros apagados (prováveis)</span></label>
      <button class="btn sm" id="dbsch">${S.schema ? 'Ver dados' : 'Ver esquema'}</button>`;
    if (S.schema) { grid.innerHTML = `<pre class="pv-text">${esc(t.sql)}</pre>`; foot.innerHTML = ''; wire(); return; }
    grid.innerHTML = '<div class="pv-loading"><i></i></div>';
    let page;
    try { page = await db.rows(S.table, S.offset, S.limit, S.deleted); } catch (e) { grid.innerHTML = `<div class="pv-note bad">${esc(e.message || e)}</div>`; wire(); return; }
    if (state.dead) return;
    const q = S.q.toLowerCase();
    const rows = q ? page.rows.filter((r) => r.cells.some((c) => String(c.t === 'blob' ? '' : c.v).toLowerCase().includes(q))) : page.rows;
    const notes = (page.warnings || []).map((w) => `<div class="pv-note bad">${esc(w)}</div>`).join('');
    grid.innerHTML = notes + (rows.length ? `<table class="db-table"><thead><tr><th>#</th>${page.columns.map((c) => `<th>${esc(c)}</th>`).join('')}</tr></thead><tbody>${rows.map((r, i) => `<tr class="${r.deleted ? 'del' : ''}"><th>${r.deleted ? 'apagado' : (r.rowid ?? S.offset + i + 1)}</th>${r.cells.map((c) => `<td>${cellHtml(c)}</td>`).join('')}</tr>`).join('')}</tbody></table>`
      : `<div class="pv-note">${S.deleted ? 'Nenhum registro apagado reconhecido nesta tabela. Isso não garante que não existam: o conteúdo pode ter sido sobrescrito.' : 'Nenhuma linha.'}</div>`);
    const total = S.deleted ? page.total : (t.rows || page.total);
    foot.innerHTML = `<span class="sub">${S.deleted ? `${total.toLocaleString(LOC())} registro(s) provável(is) apagado(s)` : `${total ? S.offset + 1 : 0} a ${Math.min(S.offset + S.limit, total)} de ${total.toLocaleString(LOC())}`}</span>
      <span class="row"><button class="btn sm" id="dbprev" ${S.offset <= 0 ? 'disabled' : ''}>‹ Anterior</button><button class="btn sm" id="dbnext" ${S.offset + S.limit >= total ? 'disabled' : ''}>Próximas ›</button></span>`;
    wire();
  }
  function wire() {
    $('#dbq')?.addEventListener('input', (e) => { S.q = e.target.value; clearTimeout(S.t); S.t = setTimeout(load, 200); });
    $('#dbdel')?.addEventListener('change', (e) => { S.deleted = e.target.checked; S.offset = 0; load(); });
    $('#dbsch')?.addEventListener('click', () => { S.schema = !S.schema; load(); });
    $('#dbprev')?.addEventListener('click', () => { S.offset = Math.max(0, S.offset - S.limit); load(); });
    $('#dbnext')?.addEventListener('click', () => { S.offset += S.limit; load(); });
  }
  nav.addEventListener('click', (e) => { const b = e.target.closest('.db-t'); if (b) { S.table = b.dataset.t; S.offset = 0; S.q = ''; S.deleted = false; S.schema = false; load(); } });
  load();
}

// ------------------------------------------------------------------ API
/** Desenha a pré-visualização de `item` dentro de `host`. Devolve uma função de limpeza. */
function mediaCard(host, item, kind, { large, inlineMedia, onOpen, onExternal }) {
  const ico = kind === 'video' ? '🎞' : '🎵';
  const canInline = inlineMedia && large;
  host.innerHTML = `<div class="pv-card"><div class="pv-bigicon" aria-hidden="true">${ico}</div>
    <b>${esc(item.name)}</b><span class="sub">${esc(KIND_LABEL[kind])} · ${human(item.size)}</span>
    ${inlineMedia
      ? '<button class="btn primary" data-pv="open">Reproduzir em tela grande</button>'
      : '<button class="btn primary" data-pv="ext">Abrir no player do sistema</button><span class="pv-why">Neste app o ' + (kind === 'video' ? 'vídeo' : 'áudio') + ' abre no seu player: o navegador embutido do Linux não traz os codecs e travaria a janela.</span>'}
  </div>`;
  host.onclick = (e) => {
    const a = e.target.closest('[data-pv]')?.dataset.pv;
    if (a === 'open') onOpen?.();
    else if (a === 'ext') onExternal?.();
  };
  void canInline;
}

export function renderPreview(host, item, { url, large = false, thumb = false, inlineMedia = true, onOpen = null, onExternal = null, db = null }) {
  const state = { dead: false, cleanup: [] };
  host.dataset.url = url;
  host.classList.remove('zoomable', 'actual');
  host.innerHTML = '<div class="pv-loading"><i></i></div>';
  const kind = kindOf(item);
  // Vídeo e áudio: nunca criamos <video>/<audio> no painel pequeno, e no Linux do app desktop
  // (WebKitGTK sem plugins de mídia) nem na tela grande, pois o processo da página morre.
  if ((kind === 'video' || kind === 'audio') && (!large || !inlineMedia)) {
    mediaCard(host, item, kind, { large, inlineMedia, onOpen, onExternal });
    return () => { state.dead = true; };
  }
  (async () => {
    try {
      if (!item.size) { host.innerHTML = ''; note(host, 'Arquivo vazio.'); return; }
      if (kind === 'image') await image(host, item, url, state, thumb);
      else if (kind === 'video') media(host, item, url, 'video', state);
      else if (kind === 'audio') media(host, item, url, 'audio', state);
      else if (kind === 'pdf') await pdf(host, item, url, state, large);
      else if (kind === 'database') await database(host, item, state, large, db, onOpen);
      else if (kind === 'office') await office(host, item, url, state);
      else if (kind === 'zip') await zip(host, item, url, state);
      else if (kind === 'text') { host.innerHTML = ''; await text(host, item, url, large); }
      else { host.innerHTML = ''; await hex(host, item, url, large); }
    } catch (e) {
      if (!state.dead) await failure(host, item, e, kind !== 'hex' && kind !== 'text');
    }
  })();
  return () => { state.dead = true; state.cleanup.forEach((f) => { try { f(); } catch { /* ignora */ } }); };
}
