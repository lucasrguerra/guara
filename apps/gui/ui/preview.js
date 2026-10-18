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

export function kindOf(item) {
  const e = (item.ext || '').toLowerCase();
  if (IMG.has(e)) return 'image';
  if (VID.has(e)) return 'video';
  if (AUD.has(e)) return 'audio';
  if (e === 'pdf') return 'pdf';
  if (OFFICE.has(e)) return 'office';
  if (ZIPS.has(e)) return 'zip';
  if (TXT.has(e)) return 'text';
  return 'hex';
}

export const KIND_LABEL = {
  image: 'Imagem', video: 'Vídeo', audio: 'Áudio', pdf: 'PDF', office: 'Documento (texto extraído)',
  zip: 'Conteúdo do arquivo compactado', text: 'Texto', hex: 'Dados brutos (hexadecimal)',
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
      host.classList.add('zoomable');
      img.onclick = () => host.classList.toggle('actual');
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

// ------------------------------------------------------------------ API
/** Desenha a pré-visualização de `item` dentro de `host`. Devolve uma função de limpeza. */
export function renderPreview(host, item, { url, large = false, thumb = false }) {
  const state = { dead: false, cleanup: [] };
  host.dataset.url = url;
  host.classList.remove('zoomable', 'actual');
  host.innerHTML = '<div class="pv-loading"><i></i></div>';
  const kind = kindOf(item);
  (async () => {
    try {
      if (!item.size) { host.innerHTML = ''; note(host, 'Arquivo vazio.'); return; }
      if (kind === 'image') await image(host, item, url, state, thumb);
      else if (kind === 'video') media(host, item, url, 'video', state);
      else if (kind === 'audio') media(host, item, url, 'audio', state);
      else if (kind === 'pdf') await pdf(host, item, url, state, large);
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
