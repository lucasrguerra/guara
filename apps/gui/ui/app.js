// Guará — interface. Fala com o núcleo por `rpc(método, parâmetros)`:
//  • app Tauri: invoke('rpc')   • modo `guara ui`: POST /rpc/<método> com token.

import { renderPreview, kindOf, KIND_LABEL } from './preview.js';

const tauri = window.__TAURI__;
const params = new URLSearchParams(location.search);
let token = params.get('token');
try {
  if (token) sessionStorage.setItem('guara-token', token);
  else token = sessionStorage.getItem('guara-token');
} catch { /* armazenamento indisponível */ }

async function rpc(method, p = {}) {
  if (tauri?.core?.invoke) return tauri.core.invoke('rpc', { method, params: p });
  const r = await fetch(`/rpc/${method}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-Guara-Token': token || '' },
    body: JSON.stringify(p),
  });
  const body = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(body.error || `erro ${r.status}`);
  return body;
}

const isWin = /Windows/i.test(navigator.userAgent);
const fileBase = tauri ? (isWin ? 'http://guara.localhost' : 'guara://localhost') : '';
const fileUrl = (job, id) => `${fileBase}/file/${job}/${id}` + (tauri ? '' : `?t=${encodeURIComponent(token || '')}`);

// Atualiza o HTML só quando mudou (evita piscar e perder o foco/hover).
function setHTML(el, html) {
  if (!el || el._h === html) return false;
  el._h = html;
  el.innerHTML = html;
  return true;
}

// ------------------------------------------------------------------ tema
function applyTheme(t) {
  const root = document.documentElement;
  if (t === 'light' || t === 'dark') root.dataset.theme = t; else delete root.dataset.theme;
  document.querySelectorAll('[data-theme-pick]').forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.themePick === (t || 'auto'))));
}
try { applyTheme(localStorage.getItem('guara-theme') || 'auto'); } catch { applyTheme('auto'); }
document.querySelectorAll('[data-theme-pick]').forEach((b) => (b.onclick = async () => {
  const t = b.dataset.themePick;
  applyTheme(t);
  try { localStorage.setItem('guara-theme', t); } catch { /* sem armazenamento */ }
  try { await rpc('settings_set', { theme: t }); } catch { /* segue só com o armazenamento local */ }
}));

// ------------------------------------------------------------------ utilidades
const $ = (s, el = document) => el.querySelector(s);
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const human = (n) => {
  if (!n) return '0 B';
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0, v = n;
  while (v >= 1024 && i < 4) { v /= 1024; i++; }
  return i === 0 ? `${n} B` : `${v.toFixed(v >= 100 ? 0 : 1)} ${u[i]}`;
};
const dur = (s) => {
  s = Math.round(s);
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s / 60)}min ${s % 60}s` : `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}min`;
};
const date = (t) => (t ? new Date(t * 1000).toLocaleString('pt-BR', { dateStyle: 'medium', timeStyle: 'short' }) : '—');
const CATS = [
  ['image', 'Fotos'], ['document', 'Documentos'], ['video', 'Vídeos'], ['audio', 'Áudio'],
  ['archive', 'Compactados'], ['database', 'Bancos de dados'], ['other', 'Outros'],
];
const CONF = { high: ['alta', 'ok'], medium: ['média', 'warn'], low: ['baixa', 'bad'] };
const KIND_ICON = {
  USB: '<path d="M12 3v12m0 0a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM12 8l-4 3m4-1l4 2M8 11v3m8-2v3" />',
  default: '<rect x="3" y="6" width="18" height="12" rx="3"/><circle cx="17" cy="12" r="1.2"/><path d="M7 12h6"/>',
};
const icon = (kind) => `<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">${KIND_ICON[kind] || KIND_ICON.default}</svg>`;
const warnSvg = '<svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3l10 18H2z"/><path d="M12 10v5M12 18v.01"/></svg>';

function toast(msg) {
  const t = $('#toast');
  t.textContent = msg;
  t.classList.add('show');
  clearTimeout(toast.t);
  toast.t = setTimeout(() => t.classList.remove('show'), 3600);
}

function ask({ title, text, value, placeholder, ok = 'OK', pick }) {
  return new Promise((resolve) => {
    const d = $('#dlg');
    d.innerHTML = `<form method="dialog"><h2>${esc(title)}</h2><p>${text}</p>
      <div class="row" style="flex-wrap:nowrap"><input class="input" name="v" value="${esc(value || '')}" placeholder="${esc(placeholder || '')}" autocomplete="off" spellcheck="false">
      ${pick && tauri?.dialog ? '<button type="button" class="btn" id="pickBtn">Escolher…</button>' : ''}</div>
      <menu><button class="btn" value="cancel">Cancelar</button><button class="btn primary" value="ok">${esc(ok)}</button></menu></form>`;
    const input = $('input', d);
    $('#pickBtn', d)?.addEventListener('click', async () => {
      const r = await tauri.dialog.open(pick);
      if (r) input.value = Array.isArray(r) ? r[0] : r;
    });
    d.onclose = () => resolve(d.returnValue === 'ok' ? input.value.trim() : null);
    d.showModal();
    input.focus();
    input.select();
  });
}

function info({ title, html }) {
  const d = $('#dlg');
  d.innerHTML = `<form method="dialog"><h2>${esc(title)}</h2>${html}<menu><button class="btn primary" value="ok">Entendi</button></menu></form>`;
  d.onclose = null;
  d.showModal();
}

// ------------------------------------------------------------------ estado
const S = {
  devices: [],
  defaults: {},
  sessions: [], // {job, target, label}
  device: null, // {path,name,...}
  layout: null,
  job: null, // id da varredura atual
  status: null,
  filters: { category: null, minConfidence: null, query: '', sort: 'confidence' },
  items: [],
  total: 0,
  selected: new Set(),
  allFiltered: false,
  current: null, // arquivo em foco (pré-visualização)
  poll: null,
  recJob: null,
};

const main = $('#main');
const setMenu = (open) => {
  $('#app').classList.toggle('menu-open', open);
  $('#menuBtn').setAttribute('aria-expanded', String(open));
};
$('#menuBtn').onclick = () => setMenu(!$('#app').classList.contains('menu-open'));
$('#scrim').onclick = () => setMenu(false);
$('#refreshBtn').onclick = () => loadDevices(true);

// ------------------------------------------------------------------ dispositivos
async function loadDevices(manual) {
  try {
    S.devices = await rpc('devices');
  } catch (e) { toast(e.message); }
  renderSide();
  if (manual) toast('Lista atualizada');
}

function renderSide() {
  const dl = $('#deviceList');
  dl.dataset.empty = 'Nenhum disco encontrado';
  dl.innerHTML = S.devices.map((d, i) => `
    <li><button class="dev" data-i="${i}" aria-current="${S.device?.path === d.path}">
      <span class="ico">${icon(d.kind)}</span>
      <span><span class="t">${esc(d.name)}</span><span class="s">${esc(d.kind)} · ${d.size ? human(d.size) : '—'}${d.readable ? '' : ' · requer admin'}</span></span>
    </button></li>`).join('');
  dl.querySelectorAll('.dev').forEach((b) => (b.onclick = () => { openDevice(S.devices[+b.dataset.i]); setMenu(false); }));
  const sl = $('#sessionList');
  sl.dataset.empty = 'Nenhuma ainda';
  sl.innerHTML = S.sessions.map((s, i) => `
    <li><button class="dev" data-j="${i}" aria-current="${S.job === s.job}"><span class="ico">${icon('default')}</span>
      <span><span class="t">${esc(s.label)}</span><span class="s">${esc(s.target)}</span></span></button></li>`).join('');
  sl.querySelectorAll('.dev').forEach((b) => (b.onclick = () => { openJob(S.sessions[+b.dataset.j]); setMenu(false); }));
}

$('#openImageBtn').onclick = async () => {
  setMenu(false);
  let path;
  if (tauri?.dialog) {
    path = await tauri.dialog.open({ multiple: false, directory: false });
    if (Array.isArray(path)) path = path[0];
  } else {
    path = await ask({ title: 'Abrir arquivo de imagem', text: 'Caminho completo de uma imagem de disco (.img, .dd, .iso…).', placeholder: '/caminho/para/disco.img', ok: 'Abrir' });
  }
  if (path) openDevice({ path, name: path.split(/[\\/]/).pop(), kind: 'Imagem', size: 0, readable: true, mounted_at: [], removable: false });
};

$('#loadSessionBtn').onclick = async () => {
  setMenu(false);
  let path;
  if (tauri?.dialog) {
    path = await tauri.dialog.open({ multiple: false, filters: [{ name: 'Sessão do Guará', extensions: ['json'] }] });
    if (Array.isArray(path)) path = path[0];
  } else {
    path = await ask({ title: 'Abrir sessão salva', text: 'Caminho do arquivo de sessão (.json) criado pelo Guará.', placeholder: '/caminho/sessao.json', ok: 'Abrir' });
  }
  if (!path) return;
  try {
    const { job } = await rpc('session_load', { path });
    const st = await rpc('status', { job });
    const s = { job, target: st.target, label: 'Sessão: ' + st.target.split(/[\\/]/).pop() };
    S.sessions.unshift(s);
    openJob(s);
  } catch (e) { toast(e.message); }
};

// ------------------------------------------------------------------ início
function renderHome() {
  stopPoll();
  S.device = null; S.job = null;
  renderSide();
  main.innerHTML = `<div class="page">
    <section class="hero"><img class="hero-logo" src="guara-logo.svg" alt="Guará"><h1>Encontre o que você achou que tinha perdido.</h1>
    <p>Escolha um disco, SSD ou pendrive ao lado. O Guará lê tudo — arquivos apagados, partições perdidas e espaço não particionado — sem escrever nada no disco de origem.</p></section>
    <div class="cards">
      <div class="card"><h3>Uma varredura, tudo junto</h3><p>Metadados do sistema de arquivos e busca por assinatura na mesma passada, sem duplicatas.</p></div>
      <div class="card"><h3>Confiança por arquivo</h3><p>Cada achado mostra a chance de abrir corretamente: alta, média ou baixa.</p></div>
      <div class="card"><h3>Origem protegida</h3><p>Somente leitura. A recuperação sempre vai para outra pasta, com SHA-256 de tudo.</p></div>
    </div>
    ${S.devices.length ? '' : `<div class="notice info" style="margin-top:22px">${warnSvg}<div><b>Nenhum disco apareceu.</b> Discos brutos exigem administrador. Rode <code>sudo guara ui</code> (Linux/macOS) ou abra o Guará como administrador (Windows). Você ainda pode abrir um arquivo de imagem pelo menu.</div></div>`}
  </div>`;
}

// ------------------------------------------------------------------ dispositivo
async function openDevice(d) {
  stopPoll();
  S.device = d; S.job = null; S.layout = null;
  renderSide();
  main.innerHTML = `<div class="page"><p class="sub">Lendo ${esc(d.name)}…</p></div>`;
  main.scrollTop = 0;
  let layout = null, err = null;
  try { layout = await rpc('inspect', { path: d.path }); } catch (e) { err = e.message; }
  S.layout = layout;
  renderDevice(err);
}

function fsClass(p) {
  if (p.encrypted) return 'fs-enc';
  const f = (p.fs || '').toLowerCase();
  if (f.startsWith('fat')) return 'fs-fat';
  return `fs-${f}` in { 'fs-ntfs': 1, 'fs-exfat': 1, 'fs-ext': 1, 'fs-apfs': 1, 'fs-hfsplus': 1 } ? `fs-${f}` : 'fs-unknown';
}
const FS_NAME = { ntfs: 'NTFS', fat12: 'FAT12', fat16: 'FAT16', fat32: 'FAT32', exfat: 'exFAT', ext: 'ext2/3/4', apfs: 'APFS', hfsplus: 'HFS+', bitlocker: 'BitLocker', luks: 'LUKS', unknown: 'Desconhecido' };

function diskMap(layout) {
  const total = layout.size || 1;
  const segs = [];
  layout.partitions.forEach((p) => segs.push({ start: p.start, len: p.len, p }));
  layout.gaps.forEach((g) => segs.push({ start: g.start, len: g.len, gap: true }));
  segs.sort((a, b) => a.start - b.start);
  return `<div class="diskmap" role="img" aria-label="Mapa do disco">${segs.map((s) => {
    const w = Math.max(s.len / total * 100, 1.4);
    if (s.gap) return `<div class="seg gap" style="flex:${w} 1 0" title="Espaço não particionado: ${human(s.len)}"><b>Não particionado</b>${human(s.len)}</div>`;
    const p = s.p;
    return `<div class="seg ${fsClass(p)} ${p.lost ? 'lost' : ''}" style="flex:${w} 1 0" title="${esc(FS_NAME[p.fs] || p.fs)} · ${human(p.len)}"><b>${p.encrypted ? '🔒 ' : ''}${esc(p.label || FS_NAME[p.fs] || p.fs)}</b>${human(p.len)}</div>`;
  }).join('')}</div>`;
}

function renderDevice(err) {
  const d = S.device, l = S.layout;
  const notes = [];
  if (!d.readable) notes.push(['bad', 'Este disco exige permissão de administrador. ' + (tauri ? '<button class="btn sm" id="elevate" style="margin-left:6px">Reiniciar como administrador</button>' : 'Feche e rode de novo com <code>sudo guara ui</code> (Linux/macOS) ou como administrador (Windows).')]);
  if (err) notes.push(['bad', esc(err)]);
  if (d.mounted_at?.length) notes.push(['warn', `Este disco está em uso (montado em <code>${esc(d.mounted_at.join(', '))}</code>). O Guará só lê, mas evite gravar nele enquanto recupera — qualquer escrita pode sobrescrever o que você quer salvar.`]);
  if (d.trim) notes.push(['warn', 'Este SSD usa TRIM: o sistema avisa o disco quais blocos foram apagados e o próprio SSD pode zerá-los. Arquivos apagados há bastante tempo tendem a não ser recuperáveis.']);
  (l?.warnings || []).forEach((w) => notes.push(['warn', esc(w)]));

  main.innerHTML = `<div class="page">
    <div class="dev-head"><div><h1>${esc(d.name)}</h1><div class="sub">${esc(d.kind)}${d.size ? ' · ' + human(d.size) : l ? ' · ' + human(l.size) : ''} · <span class="mono">${esc(d.path)}</span></div></div></div>
    ${notes.map(([k, t]) => `<div class="notice ${k === 'bad' ? 'bad' : ''}">${warnSvg}<div>${t}</div></div>`).join('')}
    ${l ? `<div class="section"><h2>Mapa do disco</h2>${diskMap(l)}
      <div class="legend">${l.partitions.map((p) => `<div><span class="dot ${fsClass(p)}"></span><b>Partição ${p.index + 1}</b> ${esc(FS_NAME[p.fs] || p.fs)} · ${human(p.len)}${p.label ? ' · ' + esc(p.label) : ''}
        ${p.lost ? '<span class="pill accent">encontrada por varredura</span>' : ''}${p.encrypted ? `<span class="pill warn">cifrada (${esc(p.encrypted)}) · só detecção</span>` : ''}</div>`).join('')}
        ${l.gaps.map((g) => `<div><span class="dot" style="background:var(--text-3)"></span><b>Espaço não particionado</b> ${human(g.len)}</div>`).join('')}</div></div>` : ''}
    <div class="section"><h2>Como procurar</h2>
      <div class="modes">
        <div class="mode rec"><span class="pill accent" style="align-self:flex-start">Recomendado</span><h3>Varredura profunda</h3><p>Lê os metadados e examina todo o espaço livre, partições danificadas e áreas não particionadas por assinatura. Encontra mais, leva mais tempo.</p>
          <button class="btn primary lg" id="goDeep" ${d.readable ? '' : 'disabled'}>Procurar tudo</button></div>
        <div class="mode"><h3>Varredura rápida</h3><p>Só os registros do sistema de arquivos. Em segundos mostra arquivos apagados recentemente, com nome e pasta.</p>
          <button class="btn lg" id="goQuick" ${d.readable ? '' : 'disabled'}>Varredura rápida</button></div>
        <div class="mode"><h3>Criar imagem do disco</h3><p>Copia o disco inteiro para um arquivo, tolerando setores ruins. Ideal para discos com defeito: você trabalha na cópia.</p>
          <button class="btn lg" id="goImage" ${d.readable ? '' : 'disabled'}>Criar imagem…</button></div>
      </div>
      <details class="adv"><summary>Opções avançadas</summary>
        <label class="check"><input type="checkbox" id="optAlloc"><span><b>Incluir áreas ainda em uso</b><br><span class="sub">Também procura cópias antigas dentro de arquivos existentes. Bem mais lento.</span></span></label>
        <div class="row" id="optCats">${CATS.map(([k, n]) => `<label class="check"><input type="checkbox" value="${k}"> ${n}</label>`).join('')}</div>
        <div class="sub">Sem nenhuma categoria marcada, todos os tipos são procurados.</div>
      </details>
    </div></div>`;
  $('#elevate')?.addEventListener('click', () => tauri.core.invoke('relaunch_elevated').catch((e) => toast(String(e))));
  const opts = (mode) => ({
    mode,
    include_allocated: $('#optAlloc')?.checked || false,
    categories: [...document.querySelectorAll('#optCats input:checked')].map((i) => i.value),
  });
  $('#goDeep')?.addEventListener('click', () => startScan(opts('deep')));
  $('#goQuick')?.addEventListener('click', () => startScan(opts('quick')));
  $('#goImage')?.addEventListener('click', startImage);
}

async function startScan(options) {
  try {
    const { job } = await rpc('scan_start', { path: S.device.path, options });
    const s = { job, target: S.device.path, label: S.device.name };
    S.sessions.unshift(s);
    openJob(s);
  } catch (e) { toast(e.message); info({ title: 'Não foi possível iniciar', html: `<p>${esc(e.message)}</p>` }); }
}

async function startImage() {
  const base = (S.defaults.home || '') + (S.defaults.sep || '/');
  const dest = await ask({ title: 'Criar imagem do disco', text: 'Onde salvar o arquivo de imagem? Use <b>outro disco</b> com espaço suficiente e um nome que ainda não exista.', value: base + 'disco-guara.img', ok: 'Criar imagem' });
  if (!dest) return;
  try {
    const { job } = await rpc('image_start', { path: S.device.path, dest });
    watchAux(job, 'Criando imagem do disco', (st) => {
      const r = st.report || {};
      const bad = (r.bad_ranges || []).length;
      return `<p>Imagem salva em <code>${esc(dest)}</code> (${human(r.bytes)}).</p><p>SHA-256: <code>${esc(r.sha256)}</code></p>${bad ? `<div class="notice">${warnSvg}<div>${bad} trecho(s) ilegível(is) foram preenchidos com zeros. Veja o mapa de erros ao lado da imagem.</div></div>` : '<p>Nenhum setor ruim encontrado.</p>'}`;
    });
  } catch (e) { toast(e.message); }
}

// ------------------------------------------------------------------ varredura
function openJob(s) {
  stopPoll();
  S.job = s.job;
  S.filters = { category: null, minConfidence: null, query: '', sort: 'confidence' };
  S.items = []; S.total = 0; S.selected = new Set(); S.allFiltered = false; S.current = null; S.status = null;
  renderSide();
  renderScan();
  tick();
  S.poll = setInterval(tick, 500);
  main.scrollTop = 0;
}

function stopPoll() { clearInterval(S.poll); S.poll = null; }

let lastFound = -1, lastFilterKey = '';
async function tick() {
  if (!S.job) return;
  let st;
  try { st = await rpc('status', { job: S.job }); } catch (e) { stopPoll(); toast(e.message); return; }
  S.status = st;
  const key = JSON.stringify(S.filters);
  if (st.found !== lastFound || key !== lastFilterKey) {
    lastFound = st.found; lastFilterKey = key;
    await loadResults(true);
  }
  paintStatus();
  if (st.status !== 'running') {
    stopPoll();
    if (st.status === 'error') toast(st.error);
    await loadResults(true);
    paintStatus();
  }
}

async function loadResults(reset) {
  const f = S.filters;
  const query = { sort: f.sort, offset: reset ? 0 : S.items.length, limit: 200 };
  if (f.category) query.category = f.category;
  if (f.minConfidence) query.min_confidence = f.minConfidence;
  if (f.query) query.query = f.query;
  const r = await rpc('results', { job: S.job, query });
  S.items = reset ? r.items : S.items.concat(r.items);
  S.total = r.total;
  paintList();
}

function renderScan() {
  main.innerHTML = `<div class="page">
    <button class="back" id="backBtn">‹ Dispositivos</button>
    <div class="dev-head"><div><h1 id="scanTitle">Procurando…</h1><div class="sub" id="scanSub"></div></div></div>
    <div id="progressArea"></div>
    <div id="warnArea"></div>
    <div class="toolbar">
      <div class="chips" id="chips" role="group" aria-label="Filtrar por tipo"></div>
    </div>
    <div class="toolbar" style="margin-top:0">
      <input class="input" id="q" type="search" placeholder="Buscar por nome ou pasta" value="${esc(S.filters.query)}" aria-label="Buscar">
      <select class="select" id="conf" aria-label="Confiança mínima"><option value="">Qualquer confiança</option><option value="medium">Média ou alta</option><option value="high">Só alta</option></select>
      <select class="select" id="sort" aria-label="Ordenar"><option value="confidence">Mais confiáveis</option><option value="size">Maiores</option><option value="name">Nome</option></select>
    </div>
    <div class="split"><div class="table" id="table"></div><aside class="preview" id="preview" aria-live="polite"></aside></div>
  </div><div id="barHost"></div>`;
  $('#backBtn').onclick = () => (S.device ? openDevice(S.device) : renderHome());
  $('#progressArea').onclick = onProgressClick;
  $('#chips').onclick = (e) => { const c = e.target.closest('.chip'); if (c) { S.filters.category = c.dataset.c || null; resetSel(); tick(); } };
  let t;
  $('#q').oninput = (e) => { clearTimeout(t); t = setTimeout(() => { S.filters.query = e.target.value; resetSel(); tick(); }, 250); };
  $('#conf').onchange = (e) => { S.filters.minConfidence = e.target.value || null; resetSel(); tick(); };
  $('#sort').onchange = (e) => { S.filters.sort = e.target.value; tick(); };
  wireTable();
  paintPreview();
}
const resetSel = () => { S.selected = new Set(); S.allFiltered = false; };

function paintStatus() {
  const st = S.status;
  if (!st || !$('#progressArea')) return;
  const running = st.status === 'running';
  const p = st.progress || {};
  const pct = p.total ? Math.min(100, Math.floor((p.done / p.total) * 100)) : null;
  $('#scanTitle').textContent = running ? 'Procurando…' : st.status === 'paused' ? 'Varredura pausada' : st.status === 'error' ? 'Algo deu errado' : st.status === 'stopped' ? 'Varredura interrompida' : 'Varredura concluída';
  $('#scanSub').innerHTML = `<span class="mono">${esc(st.target)}</span>`;
  const rate = p.total && st.elapsed > 1 && p.done ? p.done / st.elapsed : 0;
  const eta = rate && running ? dur((p.total - p.done) / rate) : null;
  setHTML($('#progressArea'), `<div class="progress-card">
    <div class="row spread"><div><b>${esc(p.phase || (running ? 'Iniciando' : ''))}</b>${pct != null ? ` <span class="sub">· ${pct}%</span>` : ''}</div>
      <div class="row">${running ? '<button class="btn sm" data-act="pause">Pausar</button><button class="btn sm danger" data-act="stop">Parar</button>' : st.status === 'paused' ? '<button class="btn sm primary" data-act="resume">Continuar</button>' : ''}
      ${!running && st.found ? '<button class="btn sm" data-act="save">Salvar sessão…</button>' : ''}</div></div>
    <div class="bar ${running && pct == null ? 'indet' : ''}"><i style="width:${pct ?? (running ? 35 : 100)}%"></i></div>
    <div class="stats">
      <div class="stat"><b>${st.found.toLocaleString('pt-BR')}</b><span>arquivos encontrados</span></div>
      <div class="stat"><b>${dur(st.elapsed)}</b><span>decorrido${eta ? ' · faltam ~' + eta : ''}</span></div>
      <div class="stat"><b style="color:var(--ok)">${st.confidence.high}</b><span>confiança alta</span></div>
      <div class="stat"><b style="color:var(--warn)">${st.confidence.medium}</b><span>média</span></div>
      <div class="stat"><b style="color:var(--bad)">${st.confidence.low}</b><span>baixa</span></div>
    </div></div>`);
  setHTML($('#warnArea'), (st.warnings || []).map((w) => `<div class="notice">${warnSvg}<div>${esc(w)}</div></div>`).join('')
    + (st.error ? `<div class="notice bad">${warnSvg}<div>${esc(st.error)}</div></div>` : ''));
  // chips
  const total = st.found;
  setHTML($('#chips'), [`<button class="chip" data-c="" aria-pressed="${!S.filters.category}">Tudo<small>${total}</small></button>`]
    .concat(CATS.filter(([k]) => st.categories[k]).map(([k, n]) => `<button class="chip" data-c="${k}" aria-pressed="${S.filters.category === k}">${n}<small>${st.categories[k]}</small></button>`)).join(''));
  const sel = $('#conf'); if (sel && sel.value !== (S.filters.minConfidence || '')) sel.value = S.filters.minConfidence || '';
  paintBar();
}

async function onProgressClick(e) {
  const act = e.target.closest('[data-act]')?.dataset.act;
  if (!act) return;
  if (act === 'pause') { await rpc('scan_pause', { job: S.job }); toast('Pausando…'); }
  else if (act === 'stop') await rpc('scan_stop', { job: S.job });
  else if (act === 'resume') { await rpc('scan_resume', { job: S.job }); stopPoll(); S.poll = setInterval(tick, 500); tick(); }
  else if (act === 'save') {
    const path = await ask({ title: 'Salvar sessão', text: 'Guarda os resultados para recuperar depois, sem varrer de novo.', value: (S.defaults.home || '') + (S.defaults.sep || '/') + 'guara-sessao.json', ok: 'Salvar' });
    if (path) { try { await rpc('session_save', { job: S.job, path }); toast('Sessão salva'); } catch (err) { toast(err.message); } }
  }
}

function rowHtml(i) {
  const [cl, cc] = CONF[i.confidence];
  const on = S.allFiltered || S.selected.has(i.id);
  return `<div class="tr" role="row" data-id="${i.id}" aria-selected="${on || S.current?.id === i.id}" tabindex="0">
    <input type="checkbox" ${on ? 'checked' : ''} aria-label="Selecionar ${esc(i.name)}">
    <div class="nm"><b>${esc(i.name)}</b><small>${esc(i.dir || (i.origin === 'carved' ? 'Encontrado por assinatura' : '—'))}</small></div>
    <div class="num">${human(i.size)}</div><div class="num" style="text-align:left">${i.origin === 'carved' ? 'assinatura' : esc(i.origin)}</div>
    <div class="num"><span class="pill ${cc}">${cl}</span></div></div>`;
}

function paintList() {
  const el = $('#table');
  if (!el) return;
  const st = S.status;
  if (!S.items.length) {
    setHTML(el, `<div class="empty"><h3>${st && st.status === 'running' ? 'Procurando…' : 'Nada por aqui'}</h3><p>${st && st.status === 'running' ? 'Os arquivos aparecem aqui assim que são encontrados.' : (S.filters.category || S.filters.query || S.filters.minConfidence) ? 'Nenhum arquivo combina com os filtros.' : 'Não encontramos arquivos recuperáveis neste disco.'}</p></div>`);
    paintBar();
    return;
  }
  const allSel = S.items.every((i) => S.selected.has(i.id)) || S.allFiltered;
  setHTML(el, `<div class="thead"><input type="checkbox" id="selAll" aria-label="Selecionar todos" ${allSel ? 'checked' : ''}><span>Nome</span><span style="text-align:right">Tamanho</span><span>Origem</span><span style="text-align:right">Confiança</span></div>`
    + S.items.map(rowHtml).join('')
    + (S.items.length < S.total ? `<div class="more"><button class="btn" id="moreBtn">Mostrar mais (${(S.total - S.items.length).toLocaleString('pt-BR')})</button></div>` : ''));
  paintBar();
}

// Eventos da tabela por delegação (o HTML é recriado, o listener não).
function wireTable() {
  const el = $('#table');
  el.addEventListener('click', (e) => {
    if (e.target.id === 'moreBtn') return loadResults(false);
    const row = e.target.closest('.tr');
    if (e.target.id === 'selAll') {
      if (e.target.checked) S.items.forEach((i) => S.selected.add(i.id)); else resetSel();
      el._h = null; paintList();
      return;
    }
    if (!row) return;
    const id = +row.dataset.id;
    if (e.target.matches('input[type=checkbox]')) {
      if (S.allFiltered) { S.allFiltered = false; S.items.forEach((i) => S.selected.add(i.id)); }
      e.target.checked ? S.selected.add(id) : S.selected.delete(id);
      row.setAttribute('aria-selected', e.target.checked);
      el._h = null;
      paintBar();
      return;
    }
    focusItem(S.items.find((i) => i.id === id));
  });
  el.addEventListener('dblclick', (e) => { if (e.target.closest('.tr')) openLightbox(); });
}

let stopPv = null;
function focusItem(item, { scroll = false } = {}) {
  if (!item) return;
  S.current = item;
  document.querySelectorAll('.tr').forEach((r) => {
    const on = +r.dataset.id === item.id || S.selected.has(+r.dataset.id) || S.allFiltered;
    r.setAttribute('aria-selected', on);
    if (scroll && +r.dataset.id === item.id) r.scrollIntoView({ block: 'nearest' });
  });
  paintPreview();
}

function detailsHtml(i) {
  const [cl, cc] = CONF[i.confidence];
  return `<dl class="kv"><dt>Nome</dt><dd>${esc(i.name)}</dd><dt>Pasta</dt><dd>${esc(i.dir || '—')}</dd><dt>Tamanho</dt><dd>${human(i.size)}</dd>
    <dt>Modificado</dt><dd>${date(i.modified)}</dd><dt>Origem</dt><dd>${i.origin === 'carved' ? 'Assinatura de formato' : esc(i.origin.toUpperCase())}</dd>
    <dt>Confiança</dt><dd><span class="pill ${cc}">${cl}</span></dd>${i.note ? `<dt>Aviso</dt><dd>${esc(i.note)}</dd>` : ''}</dl>`;
}

function paintPreview() {
  const el = $('#preview');
  if (!el) return;
  stopPv?.(); stopPv = null;
  const i = S.current;
  if (!i) { el.innerHTML = '<div class="viewer"><div class="pv-note">Selecione um arquivo para pré-visualizar</div></div>'; return; }
  el.innerHTML = `<div class="viewer" id="pvHost"></div><div class="sub" style="font-size:12.5px">${esc(KIND_LABEL[kindOf(i)])}</div>${detailsHtml(i)}
    <div class="row"><button class="btn sm" id="pvFull">Tela cheia</button><button class="btn primary sm" id="recOne">Recuperar este arquivo</button></div>`;
  stopPv = renderPreview($('#pvHost'), i, { url: fileUrl(S.job, i.id) });
  $('#pvFull').onclick = openLightbox;
  $('#recOne').onclick = () => recover({ ids: [i.id] });
}

// ------------------------------------------------------------------ tela cheia
let stopLb = null;
function openLightbox() {
  const i = S.current;
  if (!i) return;
  const d = $('#lightbox');
  const idx = S.items.findIndex((x) => x.id === i.id);
  d.innerHTML = `<div class="lb-head"><button class="icon-btn" id="lbPrev" aria-label="Anterior" ${idx <= 0 ? 'disabled' : ''}>‹</button><button class="icon-btn" id="lbNext" aria-label="Próximo" ${idx >= S.items.length - 1 ? 'disabled' : ''}>›</button>
      <div class="ttl"><b>${esc(i.name)}</b><small>${esc(KIND_LABEL[kindOf(i)])} · ${human(i.size)} · confiança ${CONF[i.confidence][0]}</small></div>
      <button class="icon-btn" id="lbClose" aria-label="Fechar"><svg viewBox="0 0 24 24" width="20" height="20"><path d="M6 6l12 12M18 6L6 18" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg></button></div>
    <div class="lb-body"><div class="viewer large" id="lbHost"></div></div>
    <div class="lb-foot"><span class="sub">${i.note ? esc(i.note) : '← → para navegar · Esc para fechar'}</span><button class="btn primary sm" id="lbRec">Recuperar este arquivo</button></div>`;
  stopLb?.();
  stopLb = renderPreview($('#lbHost', d), i, { url: fileUrl(S.job, i.id), large: true });
  const go = (n) => { const t = S.items[idx + n]; if (t) { focusItem(t, { scroll: true }); openLightbox(); } };
  $('#lbPrev', d).onclick = () => go(-1);
  $('#lbNext', d).onclick = () => go(1);
  $('#lbClose', d).onclick = () => d.close();
  $('#lbRec', d).onclick = () => { d.close(); recover({ ids: [i.id] }); };
  d.onclose = () => { stopLb?.(); stopLb = null; };
  d.onkeydown = (e) => { if (e.key === 'ArrowLeft') go(-1); else if (e.key === 'ArrowRight') go(1); };
  if (!d.open) d.showModal();
}

// Teclado: ↑/↓ percorre a lista; Espaço/Enter abre a tela cheia.
document.addEventListener('keydown', (e) => {
  if (!S.job || document.querySelector('dialog[open]') || /^(INPUT|SELECT|TEXTAREA)$/.test(document.activeElement?.tagName) && document.activeElement.type !== 'checkbox') return;
  const idx = S.items.findIndex((x) => x.id === S.current?.id);
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    e.preventDefault();
    const t = S.items[Math.min(S.items.length - 1, Math.max(0, idx + (e.key === 'ArrowDown' ? 1 : -1)))];
    focusItem(t, { scroll: true });
  } else if ((e.key === ' ' || e.key === 'Enter') && S.current && !e.target.closest?.('button')) {
    e.preventDefault();
    openLightbox();
  }
});

function selCount() { return S.allFiltered ? S.total : S.selected.size; }

function paintBar() {
  const host = $('#barHost');
  if (!host) return;
  const st = S.status;
  const n = selCount();
  const canAll = st && st.found > 0 && st.status !== 'error';
  if (!n && !canAll) { host.innerHTML = ''; host._h = null; return; }
  const moreOffer = !S.allFiltered && S.selected.size > 0 && S.selected.size < S.total && S.items.length === S.selected.size
    ? '<button class="btn sm" data-bar="all">Selecionar todos os ' + S.total.toLocaleString('pt-BR') + '</button>' : '';
  const inner = `<span class="info">${n ? `${n.toLocaleString('pt-BR')} selecionado(s)` : `${S.total.toLocaleString('pt-BR')} arquivo(s) na lista`}</span>${moreOffer}
    ${n ? '<button class="btn primary" data-bar="sel">Recuperar selecionados</button>' : '<button class="btn primary" data-bar="rec">Recuperar todos da lista</button>'}`;
  let bar = host.firstElementChild;
  if (!bar) {
    // A barra é criada uma vez (com animação de entrada) e depois só tem o texto atualizado.
    host.innerHTML = '<div class="actionbar enter"></div>';
    bar = host.firstElementChild;
    bar.onclick = (e) => {
      const act = e.target.closest('[data-bar]')?.dataset.bar;
      if (act === 'all') { S.allFiltered = true; $('#table')._h = null; paintList(); }
      else if (act === 'sel') recover(S.allFiltered ? { filter: currentFilter() } : { ids: [...S.selected] });
      else if (act === 'rec') recover({ filter: currentFilter() });
    };
  }
  setHTML(bar, inner);
}

function currentFilter() {
  const f = S.filters;
  const q = { sort: f.sort, limit: 1 };
  if (f.category) q.category = f.category;
  if (f.minConfidence) q.min_confidence = f.minConfidence;
  if (f.query) q.query = f.query;
  return q;
}

async function recover(sel) {
  const dest = await ask({
    title: 'Recuperar para…',
    text: 'Escolha uma pasta em <b>outro disco</b>. O Guará nunca grava no disco de origem, para não sobrescrever o que ainda dá para salvar.',
    value: (S.defaults.home || '') + (S.defaults.sep || '/') + 'Guara-Recuperados',
    ok: 'Recuperar',
    pick: { directory: true, multiple: false },
  });
  if (!dest) return;
  try {
    const { job } = await rpc('recover_start', { job: S.job, dest, ...sel });
    watchAux(job, 'Recuperando arquivos', (st) => {
      const r = st.report || {};
      const ok = (r.recovered || []).length, bad = (r.failed || []).length;
      return `<p><b>${ok}</b> arquivo(s) salvos em <code>${esc(dest)}</code> (${human(r.total_bytes)}).</p>
        ${bad ? `<div class="notice">${warnSvg}<div>${bad} arquivo(s) não puderam ser gravados.</div></div>` : ''}
        <p class="sub">O arquivo <code>guara-manifesto.json</code> lista o SHA-256 de cada arquivo para você conferir a integridade.</p>`;
    });
  } catch (e) { info({ title: 'Não foi possível recuperar', html: `<p>${esc(e.message)}</p>` }); }
}

// Modal de progresso de tarefas auxiliares (recuperação / imagem).
function watchAux(job, title, doneHtml) {
  const d = $('#dlg');
  d.onclose = null;
  const draw = (st) => {
    const p = st.progress || {};
    const pct = p.total ? Math.floor((p.done / p.total) * 100) : 0;
    if (st.status === 'running') {
      d.innerHTML = `<form method="dialog"><h2>${esc(title)}</h2><p>${esc(p.phase || '')}</p><div class="bar"><i style="width:${pct}%"></i></div><p class="sub">${pct}%</p><menu><button type="button" class="btn danger" id="auxStop">Cancelar</button></menu></form>`;
      $('#auxStop').onclick = () => rpc('job_stop', { job });
    } else if (st.status === 'error') {
      d.innerHTML = `<form method="dialog"><h2>Algo deu errado</h2><div class="notice bad">${warnSvg}<div>${esc(st.error)}</div></div><menu><button class="btn primary">Fechar</button></menu></form>`;
    } else {
      d.innerHTML = `<form method="dialog"><h2>${st.status === 'stopped' ? 'Interrompido' : 'Pronto'}</h2>${doneHtml(st)}<menu><button class="btn primary">Fechar</button></menu></form>`;
    }
  };
  if (!d.open) d.showModal();
  draw({ status: 'running', progress: {} });
  const iv = setInterval(async () => {
    try {
      const st = await rpc('status', { job });
      draw(st);
      if (st.status !== 'running') clearInterval(iv);
    } catch { clearInterval(iv); }
  }, 400);
}

async function openJobFromList(s) { openJob(s); }

// ------------------------------------------------------------------ início do app
(async function init() {
  try {
    const v = await rpc('version');
    $('#versionTag').textContent = `v${v.version}`;
    S.defaults = await rpc('defaults');
    try { const st = await rpc('settings_get'); if (st?.theme) { applyTheme(st.theme); try { localStorage.setItem('guara-theme', st.theme); } catch { /* */ } } } catch { /* usa o local */ }
    tauri?.event?.listen?.('elevate-failed', (e) => { toast(String(e.payload || 'Não foi possível reiniciar como administrador')); });
  } catch (e) {
    main.innerHTML = `<div class="page"><div class="notice bad">${warnSvg}<div>Não foi possível falar com o núcleo do Guará: ${esc(e.message)}. Se abriu esta página manualmente, use o endereço completo exibido pelo comando <code>guara ui</code> (ele inclui o token).</div></div></div>`;
    return;
  }
  await loadDevices();
  renderHome();
})();
