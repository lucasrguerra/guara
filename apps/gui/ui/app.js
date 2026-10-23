// Guará: interface. Fala com o núcleo por `rpc(método, parâmetros)`:
//  • app Tauri: invoke('rpc')   • modo `guara ui`: POST /rpc/<método> com token.

import { createExplorer } from './explorer.js';
import { setLang, LOC, currentLang } from './i18n.js';

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
const thumbUrl = (job, id, s = 256) => `${fileBase}/thumb/${job}/${id}?s=${s}` + (tauri ? '' : `&t=${encodeURIComponent(token || '')}`);
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
  saveSettings({ theme: t });
}));

// ------------------------------------------------------------------ configurações
// (declarado antes do uso: `S` é definido mais abaixo, por isso o objeto fica aqui)
const SETTINGS = { theme: 'auto', lang: 'auto', ui_lang: null, notify: true, maps: false, updates: true };
async function saveSettings(patch) {
  Object.assign(SETTINGS, patch);
  try { await rpc('settings_set', SETTINGS); } catch { /* segue só com o armazenamento local */ }
}

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
const date = (t) => (t ? new Date(t * 1000).toLocaleString(LOC(), { dateStyle: 'medium', timeStyle: 'short' }) : 'sem data');
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

function askForm({ title, text, fields, ok = 'OK' }) {
  return new Promise((resolve) => {
    const d = $('#dlg');
    d.innerHTML = `<form method="dialog"><h2>${esc(title)}</h2><p>${text || ''}</p>
      ${fields.map((f) => `<label class="fld"><span>${esc(f.label)}</span>${
        f.options ? `<select class="select" name="${f.name}">${f.options.map(([v, l]) => `<option value="${esc(v)}" ${v === f.value ? 'selected' : ''}>${esc(l)}</option>`).join('')}</select>`
        : f.multiline ? `<textarea class="input" name="${f.name}" rows="3" placeholder="${esc(f.placeholder || '')}">${esc(f.value || '')}</textarea>`
        : `<span class="row" style="flex-wrap:nowrap"><input class="input" name="${f.name}" value="${esc(f.value || '')}" placeholder="${esc(f.placeholder || '')}" autocomplete="off">${f.pick && tauri?.dialog ? `<button type="button" class="btn" data-pick="${f.name}">Escolher…</button>` : ''}</span>`}</label>`).join('')}
      <menu><button class="btn" value="cancel">Cancelar</button><button class="btn primary" value="ok">${esc(ok)}</button></menu></form>`;
    d.querySelectorAll('[data-pick]').forEach((b) => (b.onclick = async () => {
      const f = fields.find((x) => x.name === b.dataset.pick);
      const r = await tauri.dialog.open(f.pick);
      if (r) d.querySelector(`[name="${f.name}"]`).value = Array.isArray(r) ? r[0] : r;
    }));
    d.onclose = () => {
      if (d.returnValue !== 'ok') return resolve(null);
      const out = {};
      d.querySelectorAll('[name]').forEach((el) => (out[el.name] = el.value.trim()));
      resolve(out);
    };
    d.showModal();
    d.querySelector('[name]')?.focus();
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
      <span><span class="t">${esc(d.name)}</span><span class="s">${esc(d.kind)} · ${d.size ? human(d.size) : '?'}${d.readable ? '' : ' · requer admin'}</span></span>
    </button></li>`).join('');
  dl.querySelectorAll('.dev').forEach((b) => (b.onclick = () => { openDevice(S.devices[+b.dataset.i]); setMenu(false); }));
  const sl = $('#sessionList');
  sl.dataset.empty = 'Nenhuma ainda';
  sl.innerHTML = S.sessions.map((s, i) => `
    <li><button class="dev" data-j="${i}" aria-current="${S.job === s.job}"><span class="ico">${icon('default')}</span>
      <span><span class="t">${esc(s.label)}</span><span class="s">${esc(s.target)}</span></span></button></li>`).join('');
  sl.querySelectorAll('.dev').forEach((b) => (b.onclick = () => { openJob(S.sessions[+b.dataset.j]); setMenu(false); }));
  const rl = $('#recentList');
  rl.dataset.empty = 'Nada salvo ainda';
  rl.innerHTML = (S.recent || []).map((r, i) => `
    <li><button class="dev" data-r="${i}" title="${esc(r.path)}"><span class="ico">${icon('default')}</span>
      <span><span class="t">${esc((r.target || '').split(/[\\/]/).pop() || 'Sessão')}</span><span class="s">${r.found.toLocaleString(LOC())} arquivos · ${esc(r.status)} · ${new Date(r.saved_at * 1000).toLocaleString(LOC(), { dateStyle: 'short', timeStyle: 'short' })}</span></span></button></li>`).join('');
  rl.querySelectorAll('.dev').forEach((b) => (b.onclick = () => openRecent(S.recent[+b.dataset.r])));
}

async function loadRecent() {
  try { S.recent = await rpc('sessions_list'); } catch { S.recent = []; }
  renderSide();
}

async function openRecent(r) {
  setMenu(false);
  try {
    const { job } = await rpc('session_load', { path: r.path });
    const st = await rpc('status', { job });
    let s = S.sessions.find((x) => x.job === job);
    if (!s) {
      s = { job, target: st.target, label: 'Sessão: ' + st.target.split(/[\\/]/).pop() };
      S.sessions.unshift(s);
    }
    openJob(s);
  } catch (e) { info({ title: 'Não foi possível abrir a sessão', html: `<p>${esc(e.message)}</p>` }); }
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
    let s = S.sessions.find((x) => x.job === job);
    if (!s) {
      s = { job, target: st.target, label: 'Sessão: ' + st.target.split(/[\\/]/).pop() };
      S.sessions.unshift(s);
    }
    openJob(s);
  } catch (e) { info({ title: 'Não foi possível abrir a sessão', html: `<p>${esc(e.message)}</p>` }); }
};

// ------------------------------------------------------------------ início
function renderHome() {
  main.classList.remove('fixed');
  stopPoll();
  S.device = null; S.job = null;
  renderSide();
  main.innerHTML = `<div class="page">
    <section class="hero"><img class="logo-img hero-logo" src="guara.png" alt="Guará"><h1>Encontre o que você achou que tinha perdido.</h1>
    <p>Escolha um disco, SSD ou pendrive ao lado. O Guará lê tudo: arquivos apagados, partições perdidas e espaço não particionado, sem escrever nada no disco de origem.</p></section>
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
  main.classList.remove('fixed');
  stopPoll();
  S.device = d; S.job = null; S.layout = null;
  renderSide();
  main.innerHTML = `<div class="page"><p class="sub">Lendo ${esc(d.name)}…</p></div>`;
  main.scrollTop = 0;
  let layout = null, err = null;
  S.health = null;
  try { layout = await rpc('inspect', { path: d.path }); } catch (e) { err = e.message; }
  S.layout = layout;
  renderDevice(err);
  // saúde (SMART) em segundo plano: não atrasa a tela
  rpc('health', { path: d.path }).then((h) => { if (S.device === d) { S.health = h; paintHealth(); } }).catch(() => {});
}

const HEALTH_WORD = { ok: 'Bom', warning: 'Atenção', failing: 'Ruim' };

function paintHealth() {
  const el = $('#healthArea');
  const h = S.health;
  if (!el || !h) { if (el) el.innerHTML = ''; return; }
  if (!h.available) {
    el.innerHTML = `<div class="section"><h2>Saúde do disco</h2><p class="sub">${esc(h.summary)}</p></div>`;
    return;
  }
  const bad = h.status === 'failing' || h.status === 'warning';
  const fmtN = (n) => Number(n).toLocaleString(LOC());
  const tile = (label, value, cls = '') => value == null ? '' : `<div class="h-tile ${cls}"><small>${label}</small><b>${value}</b></div>`;
  const temp = h.temperature_c;
  const tiles = [
    `<div class="h-tile h-status ${esc(h.status)}"><small>Estado</small><b>${HEALTH_WORD[h.status] || '?'}${h.health_pct != null ? ` <span>${h.health_pct}%</span>` : ''}</b></div>`,
    tile('Temperatura', temp == null ? null : `${temp} °C`, temp >= 60 ? 'failing' : temp >= 50 ? 'warning' : ''),
    tile('Horas ligado', h.power_on_hours == null ? null : `${fmtN(h.power_on_hours)} h`),
    tile('Vezes ligado', h.power_cycles == null ? null : fmtN(h.power_cycles)),
    tile('Total lido', h.host_reads == null ? null : human(h.host_reads)),
    tile('Total gravado', h.host_writes == null ? null : human(h.host_writes)),
  ].join('');
  const info = (h.info || []).map(([k, v]) => `<dt>${esc(k)}</dt><dd>${esc(v)}</dd>`).join('');
  const dash = (x) => x == null ? '<span class="dim">-</span>' : x;
  const attrs = h.attributes || [];
  const table = attrs.length ? `<details class="adv h-attrs"${bad ? ' open' : ''}><summary>Atributos SMART (${attrs.length})</summary>
    <div class="h-scroll"><table class="h-table"><thead><tr><th></th><th>ID</th><th>Atributo</th><th>Atual</th><th>Pior</th><th>Limite</th><th>Bruto</th></tr></thead><tbody>
    ${attrs.map((a) => `<tr><td><span class="h-dot ${esc(a.status)}" title="${HEALTH_WORD[a.status] || ''}"></span></td><td class="mono">${esc(a.id)}</td><td class="mono">${esc(a.name)}</td><td>${dash(a.value)}</td><td>${dash(a.worst)}</td><td>${dash(a.thresh)}</td><td class="mono">${esc(a.raw)}</td></tr>`).join('')}
    </tbody></table></div></details>` : '';
  const scope = h.partition ? `<p class="sub">O SMART é do disco inteiro: os dados abaixo são de <span class="mono">${esc(h.disk)}</span>, que contém <span class="mono">${esc(h.partition)}</span>.</p>` : '';
  // o que motivou o alerta (os campos da ficha já aparecem abaixo)
  const known = new Set(['Modelo', 'Nº de série', 'Horas ligado', 'Temperatura', 'Autoavaliação SMART', 'Vida útil usada']);
  const why = (h.details || []).filter(([k]) => !known.has(k)).map(([k, v]) => `<dt>${esc(k)}</dt><dd>${esc(v)}</dd>`).join('');
  el.innerHTML = `${bad ? `<div class="notice ${h.status === 'failing' ? 'bad' : ''}">${warnSvg}<div><b>${esc(h.summary)}</b>
    ${why ? `<dl class="kv" style="margin-top:6px">${why}</dl>` : ''}
    ${(h.advice || []).map((a) => `<div style="margin-top:6px">${esc(a)}</div>`).join('')}
    <div class="row" style="margin-top:8px"><button class="btn sm primary" id="healthImg">Criar a imagem primeiro</button></div></div></div>` : ''}
    <div class="section"><h2>Saúde do disco</h2>${scope}
    <div class="h-tiles">${tiles}</div>
    ${info ? `<dl class="kv h-info">${info}</dl>` : ''}${table}</div>`;
  $('#healthImg')?.addEventListener('click', startImage);
}

/** Disco com sinais de defeito: oferece criar a imagem antes de varrer. */
function confirmRisky() {
  return new Promise((resolve) => {
    const h = S.health;
    const d = $('#dlg');
    d.innerHTML = `<form method="dialog"><h2>Este disco parece estar com defeito</h2><p>${esc(h.summary)}</p>${(h.advice || []).map((a) => `<p class="sub">${esc(a)}</p>`).join('')}
      <menu><button class="btn" value="cancel">Cancelar</button><button class="btn" value="scan">Varrer mesmo assim</button><button class="btn primary" value="image">Criar a imagem primeiro</button></menu></form>`;
    d.onclose = () => resolve(d.returnValue === 'image' ? 'image' : d.returnValue === 'scan' ? 'scan' : null);
    d.showModal();
  });
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
  if (d.mounted_at?.length) notes.push(['warn', `Este disco está em uso (montado em <code>${esc(d.mounted_at.join(', '))}</code>). O Guará só lê, mas evite gravar nele enquanto recupera, porque qualquer escrita pode sobrescrever o que você quer salvar.`]);
  if (d.trim) notes.push(['warn', 'Este SSD usa TRIM: o sistema avisa o disco quais blocos foram apagados e o próprio SSD pode zerá-los. Arquivos apagados há bastante tempo tendem a não ser recuperáveis.']);
  (l?.warnings || []).forEach((w) => notes.push(['warn', esc(w)]));

  main.innerHTML = `<div class="page">
    <div class="dev-head"><div><h1>${esc(d.name)}</h1><div class="sub">${esc(d.kind)}${d.size ? ' · ' + human(d.size) : l ? ' · ' + human(l.size) : ''} · <span class="mono">${esc(d.path)}</span></div></div></div>
    ${notes.map(([k, t]) => `<div class="notice ${k === 'bad' ? 'bad' : ''}">${warnSvg}<div>${t}</div></div>`).join('')}
    <div id="healthArea"></div>
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
      <h2 style="margin-top:26px">Ferramentas de perícia</h2>
      <div class="modes">
        <div class="mode"><h3>Pesquisar texto no disco</h3><p>Procura palavras em todo o disco, inclusive em espaço livre e fora de partições (UTF-8 e UTF-16). Útil quando não há estrutura de arquivo.</p>
          <input class="input" id="srchTerms" placeholder="termos separados por vírgula" aria-label="Termos a pesquisar" style="margin-bottom:8px">
          <button class="btn lg" id="goSearch" ${d.readable ? '' : 'disabled'}>Pesquisar</button></div>
        <div class="mode"><h3>Hashes da origem</h3><p>Calcula MD5, SHA-1 e SHA-256 do disco inteiro para registrar a integridade e a cadeia de custódia no relatório.</p>
          <button class="btn lg" id="goHash" ${d.readable ? '' : 'disabled'}>Calcular hashes</button></div>
      </div>
      <details class="adv"><summary>Opções avançadas</summary>
        <label class="check"><input type="checkbox" id="optAlloc"><span><b>Incluir áreas ainda em uso</b><br><span class="sub">Também procura cópias antigas dentro de arquivos existentes. Bem mais lento.</span></span></label>
        <div class="row" id="optCats">${CATS.map(([k, n]) => `<label class="check"><input type="checkbox" value="${k}"> ${n}</label>`).join('')}</div>
        <div class="sub">Sem nenhuma categoria marcada, todos os tipos são procurados.</div>
      </details>
      <details class="adv"><summary>Volumes cifrados, RAID e vídeos</summary>
        <label class="fld"><span>Senha do volume (BitLocker, LUKS, FileVault 2)</span><input class="input" id="optPw" type="password" autocomplete="off"></label>
        <label class="fld"><span>Chave de recuperação do BitLocker</span><input class="input" id="optRk" placeholder="000000-000000-... (8 grupos)" autocomplete="off"></label>
        <label class="fld"><span>Arquivo de chave (.BEK ou keyfile)</span><input class="input" id="optKf" placeholder="/caminho/chave.BEK"></label>
        <label class="fld"><span>Outros discos do mesmo conjunto (RAID/LVM), um caminho por linha</span><textarea class="input" id="optMembers" rows="3"></textarea></label>
        <label class="check"><input type="checkbox" id="optRaid"><span><b>Estes discos formam um RAID sem metadados</b><br><span class="sub">O Guará descobre sozinho o nível, a faixa, o layout e a ordem dos discos.</span></span></label>
        <label class="fld"><span>Vídeo íntegro da mesma câmera/drone (refaz o índice de MP4 sem moov)</span><input class="input" id="optRef" placeholder="/caminho/video-bom.mp4"></label>
        <label class="check"><input type="checkbox" id="optRemember"><span><b>Lembrar as chaves dos volumes abertos na sessão</b><br><span class="sub">A senha nunca é gravada; só a chave derivada.</span></span></label>
      </details>
    </div></div>`;
  $('#elevate')?.addEventListener('click', () => tauri.core.invoke('relaunch_elevated').catch((e) => toast(String(e))));
  const opts = (mode) => ({
    mode,
    assembly: (() => {
      const v = (id) => ($(id)?.value || '').trim();
      const secrets = [];
      if (v('#optPw')) secrets.push({ kind: 'password', value: v('#optPw') });
      if (v('#optRk')) secrets.push({ kind: 'recovery', value: v('#optRk') });
      if (v('#optKf')) secrets.push({ kind: 'keyfile', path: v('#optKf') });
      return {
        secrets,
        members: v('#optMembers').split('\n').map((x) => x.trim()).filter(Boolean),
        raid: $('#optRaid')?.checked ? {} : null,
        remember_keys: $('#optRemember')?.checked || false,
      };
    })(),
    references: $('#optRef')?.value.trim() ? [$('#optRef').value.trim()] : [],
    include_allocated: $('#optAlloc')?.checked || false,
    categories: [...document.querySelectorAll('#optCats input:checked')].map((i) => i.value),
  });
  $('#goDeep')?.addEventListener('click', () => startScan(opts('deep')));
  $('#goQuick')?.addEventListener('click', () => startScan(opts('quick')));
  $('#goImage')?.addEventListener('click', startImage);
  paintHealth();
  $('#goSearch')?.addEventListener('click', startSearch);
  $('#srchTerms')?.addEventListener('keydown', (e) => { if (e.key === 'Enter') startSearch(); });
  $('#goHash')?.addEventListener('click', startHash);
}

async function startScan(options) {
  if (S.health && (S.health.status === 'failing' || S.health.status === 'warning') && options.mode === 'deep' && !S.healthAck?.[S.device.path]) {
    const c = await confirmRisky();
    if (c === 'image') return startImage();
    if (!c) return;
    (S.healthAck ??= {})[S.device.path] = true;
  }
  try {
    const { job } = await rpc('scan_start', { path: S.device.path, options });
    const s = { job, target: S.device.path, label: S.device.name };
    S.sessions.unshift(s);
    openJob(s);
  } catch (e) { toast(e.message); info({ title: 'Não foi possível iniciar', html: `<p>${esc(e.message)}</p>` }); }
}

async function startImage() {
  const base = (S.defaults.home || '') + (S.defaults.sep || '/');
  const f = await askForm({ title: 'Criar imagem do disco', text: 'Onde salvar o arquivo de imagem? Use <b>outro disco</b> com espaço suficiente e um nome que ainda não exista.', ok: 'Criar imagem', fields: [
    { name: 'dest', label: 'Arquivo de destino', value: base + 'disco-guara.img' },
    { name: 'format', label: 'Formato da imagem', value: 'raw', options: [['raw', 'Bruta (.img)'], ['e01', 'Expert Witness (.E01, com hash e metadados)'], ['aff4', 'AFF4 (.aff4)']] },
    { name: 'case', label: 'Número do caso (E01/AFF4)', value: '' },
    { name: 'examiner', label: 'Examinador (E01/AFF4)', value: '' },
  ] });
  if (!f || !f.dest) return;
  const dest = f.dest;
  const format = f.format === 'e01' ? { kind: 'e01', case_number: f.case, examiner: f.examiner }
    : f.format === 'aff4' ? { kind: 'aff4', case_number: f.case, examiner: f.examiner } : { kind: 'raw' };
  try {
    const { job } = await rpc('image_start', { path: S.device.path, dest, format });
    watchAux(job, 'Criando imagem do disco', (st) => {
      const r = st.report || {};
      const bad = (r.bad_ranges || []).length;
      return `<p>Imagem salva em <code>${esc(dest)}</code> (${human(r.bytes)}).</p><p>SHA-256: <code>${esc(r.sha256)}</code></p>${bad ? `<div class="notice">${warnSvg}<div>${bad} trecho(s) ilegível(is) foram preenchidos com zeros. Veja o mapa de erros ao lado da imagem.</div></div>` : '<p>Nenhum setor ruim encontrado.</p>'}`;
    });
  } catch (e) { toast(e.message); }
}

async function startHash() {
  try {
    const { job } = await rpc('hash_start', { path: S.device.path });
    watchAux(job, 'Calculando hashes da origem', (st) => {
      const r = st.report || {};
      S.sourceHashes = { ...r, path: S.device.path };
      return `<p>Hashes do conteúdo inteiro de <code>${esc(S.device.path)}</code>:</p><dl class="kv"><dt>MD5</dt><dd class="mono">${esc(r.md5)}</dd><dt>SHA-1</dt><dd class="mono">${esc(r.sha1)}</dd><dt>SHA-256</dt><dd class="mono">${esc(r.sha256)}</dd></dl><p class="sub">Eles serão incluídos nos relatórios gerados nesta sessão.</p>`;
    });
  } catch (e) { toast(e.message); }
}

async function startSearch() {
  const terms = ($('#srchTerms')?.value || '').split(',').map((t) => t.trim()).filter(Boolean);
  if (!terms.length) { toast('Digite pelo menos um termo.'); $('#srchTerms')?.focus(); return; }
  try {
    const { job } = await rpc('search_start', { path: S.device.path, terms, ignore_case: true, utf16: true });
    openSearch(job, terms);
  } catch (e) { toast(e.message); }
}

function openSearch(job, terms) {
  stopPoll();
  EX?.destroy(); EX = null;
  S.job = null;
  main.classList.remove('fixed');
  main.innerHTML = `<div class="page"><button class="back" id="backBtn">‹ ${esc(S.device?.name || 'Dispositivos')}</button>
    <h1 id="shTitle">Procurando…</h1><div class="sub">Termos: ${terms.map((t) => `<code>${esc(t)}</code>`).join(', ')}</div>
    <div id="shProg" style="margin:14px 0"></div><div class="hits" id="hits"></div></div>`;
  $('#backBtn').onclick = () => (S.device ? openDevice(S.device) : renderHome());
  const termRe = new RegExp(terms.map((t) => esc(t).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|'), 'gi');
  const tick2 = async () => {
    let st, res;
    try { st = await rpc('status', { job }); res = await rpc('search_results', { job, offset: 0, limit: 1000 }); } catch { return false; }
    const run = st.status === 'running';
    const p = st.progress || {};
    const pct = p.total ? Math.floor((p.done / p.total) * 100) : 0;
    $('#shTitle').textContent = run ? 'Procurando…' : `${res.total.toLocaleString(LOC())} ocorrência${res.total === 1 ? '' : 's'}`;
    setHTML($('#shProg'), run ? `<div class="bar"><i style="width:${pct}%"></i></div><div class="sub" style="margin-top:6px">${pct}% · ${res.total.toLocaleString(LOC())} até agora</div><button class="btn sm danger" data-stop="1" style="margin-top:8px">Parar</button>` : st.status === 'error' ? `<div class="notice bad">${warnSvg}<div>${esc(st.error)}</div></div>` : '');
    setHTML($('#hits'), res.items.length ? `<div class="hit head"><span>Posição</span><span>Partição</span><span>Codificação</span><span>Contexto</span></div>` + res.items.map((h) => `<div class="hit"><span class="mono" title="${h.offset} bytes (0x${h.offset.toString(16)})">${h.offset.toLocaleString(LOC())}</span><span>${h.partition ? '#' + h.partition : 'fora'}</span><span>${esc(h.encoding)}</span><span class="ctx">${esc(h.context).replace(termRe, (m) => `<mark>${m}</mark>`)}</span></div>`).join('') : (run ? '' : '<div class="empty"><h3>Nada encontrado</h3><p>Esses termos não aparecem em nenhum ponto do disco (nem em UTF-8 nem em UTF-16).</p></div>'));
    return run;
  };
  $('#shProg').onclick = (e) => { if (e.target.dataset.stop) rpc('job_stop', { job }); };
  (async () => { while (await tick2()) await new Promise((r) => setTimeout(r, 700)); })();
}

// ------------------------------------------------------------------ varredura
let EX = null; // explorador de arquivos (grade, navegação, detalhes)

function openJob(s) {
  stopPoll();
  EX?.destroy();
  EX = null;
  S.job = s.job;
  S.status = null;
  renderSide();
  renderScan();
  tick();
  S.poll = setInterval(tick, 500);
}

function stopPoll() { clearInterval(S.poll); S.poll = null; }

async function tick() {
  if (!S.job) return;
  let st;
  try { st = await rpc('status', { job: S.job }); } catch (e) { stopPoll(); toast(e.message); return; }
  S.status = st;
  paintStatus();
  EX?.onStatus(st);
  // continua consultando enquanto a varredura roda ou as datas/EXIF ainda estão sendo lidas
  if (st.status !== 'running' && !st.enriching) {
    stopPoll();
    loadRecent();
    if (st.status === 'error') toast(st.error);
  } else if (!S.poll) {
    S.poll = setInterval(tick, 700);
  }
}

function renderScan() {
  main.classList.add('fixed');
  main.innerHTML = `<div class="page wide fill">
    <div class="xp-top"><button class="back" id="backBtn">‹ Dispositivos</button><div><h1 id="scanTitle">Procurando…</h1><div class="sub" id="scanSub"></div></div></div>
    <div id="progressArea"></div>
    <div id="warnArea"></div>
    <div class="xp-host" id="xpHost"></div>
  </div>`;
  $('#backBtn').onclick = () => (S.device ? openDevice(S.device) : renderHome());
  $('#progressArea').onclick = onProgressClick;
  EX = createExplorer({ host: $('#xpHost'), job: S.job, rpc, esc, human, fileUrl, thumbUrl, onRecover: recover, toast, noInlineMedia: !!tauri && /Linux/i.test(navigator.userAgent), getSettings: () => SETTINGS });
}

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
  const enr = st.enriching ? `<span class="sub">Lendo datas e metadados das fotos… ${st.enriching.done.toLocaleString(LOC())}/${st.enriching.total.toLocaleString(LOC())}</span>` : '';
  const buttons = running ? '<button class="btn sm" data-act="pause">Pausar</button><button class="btn sm danger" data-act="stop">Parar</button>'
    : st.status === 'paused' ? `<button class="btn sm primary" data-act="${st.source === false ? 'reconnect' : 'resume'}">${st.source === false ? 'Reconectar o disco' : 'Continuar'}</button><button class="btn sm" data-act="relink">Escolher a origem…</button>` : '';
  const relink = st.source === false ? '<button class="btn sm primary" data-act="relink">Escolher a origem…</button>' : '';
  const tools = relink + (!running && st.found ? '<button class="btn sm" data-act="csv">Exportar CSV</button><button class="btn sm" data-act="report">Relatório</button><button class="btn sm" data-act="save">Salvar sessão…</button>' : '');
  if (running || st.status === 'paused') {
    setHTML($('#progressArea'), `<div class="progress-card slim">
      <div class="row spread"><div><b>${esc(p.phase || 'Iniciando')}</b>${pct != null ? ` <span class="sub">· ${pct}%</span>` : ''}</div><div class="row">${buttons}</div></div>
      <div class="bar ${running && pct == null ? 'indet' : ''}"><i style="width:${pct ?? (running ? 35 : 100)}%"></i></div>
      <div class="stats compact">
        <div class="stat"><b>${st.found.toLocaleString(LOC())}</b><span>arquivos</span></div>
        <div class="stat"><b>${dur(st.elapsed)}</b><span>decorrido${eta ? ' · faltam ~' + eta : ''}</span></div>
        <div class="stat"><b style="color:var(--ok)">${st.confidence.high}</b><span>alta</span></div>
        <div class="stat"><b style="color:var(--warn)">${st.confidence.medium}</b><span>média</span></div>
        <div class="stat"><b style="color:var(--bad)">${st.confidence.low}</b><span>baixa</span></div>
      </div></div>`);
  } else {
    setHTML($('#progressArea'), `<div class="done-bar"><span><b>${st.found.toLocaleString(LOC())}</b> arquivos em ${dur(st.elapsed)}</span>
      <span class="sub">confiança alta ${st.confidence.high} · média ${st.confidence.medium} · baixa ${st.confidence.low}</span>${enr}<span class="grow"></span>${tools}</div>`);
  }
  setHTML($('#warnArea'), (st.warnings || []).map((w) => `<div class="notice">${warnSvg}<div>${esc(w)}</div></div>`).join('')
    + (st.error ? `<div class="notice bad">${warnSvg}<div>${esc(st.error)}</div></div>` : ''));
}

async function chooseSource() {
  let cands = [];
  try { cands = await rpc('session_candidates', { job: S.job }); } catch { /* segue só com o campo manual */ }
  const st = S.status || {};
  const size = st.layout?.size;
  const d = $('#dlg');
  const badge = (c) => (c.same_content === true ? '<span class="pill ok">mesmo disco</span>'
    : c.same_content === false ? '<span class="pill warn">tamanho igual, conteúdo diferente</span>'
    : c.same_size ? '<span class="pill accent">mesmo tamanho (precisa de administrador para conferir)</span>' : '');
  d.innerHTML = `<form method="dialog"><h2>Escolher a origem</h2>
    <p>Esta sessão foi feita sobre <code>${esc(st.target || '')}</code>${size ? ` (${human(size)})` : ''}. Escolha o disco que voltou ou uma imagem dele.</p>
    <div class="cand">${cands.map((c, i) => `<button type="button" class="dev" data-c="${i}"><span class="ico">${icon(c.device.kind)}</span><span><span class="t">${esc(c.device.name)}</span><span class="s">${esc(c.device.path)} · ${human(c.device.size)} ${badge(c)}</span></span></button>`).join('') || '<p class="sub">Nenhum disco encontrado. Informe o caminho abaixo.</p>'}</div>
    <label class="fld"><span>Ou o caminho de um dispositivo ou imagem</span><input class="input" id="srcPath" name="path" placeholder="/dev/sdb ou /caminho/imagem.img" value="${esc(st.source_path || st.target || '')}"></label>
    <label class="check"><input type="checkbox" name="force"><span>Usar mesmo assim, mesmo que o conteúdo inicial seja diferente (por exemplo, uma cópia editada)</span></label>
    <menu><button class="btn" value="cancel">Cancelar</button><button class="btn primary" value="ok">Usar esta origem</button></menu></form>`;
  d.querySelectorAll('.cand .dev').forEach((b) => (b.onclick = () => {
    d.querySelectorAll('.cand .dev').forEach((x) => x.setAttribute('aria-current', 'false'));
    b.setAttribute('aria-current', 'true');
    $('#srcPath', d).value = cands[+b.dataset.c].device.path;
  }));
  d.onclose = async () => {
    if (d.returnValue !== 'ok') return;
    const path = $('#srcPath', d).value.trim();
    const force = d.querySelector('[name=force]').checked;
    if (!path) return;
    try {
      await rpc('session_relink', { job: S.job, path, force });
      toast('Origem reconectada');
      const cur = { job: S.job, target: st.target, label: S.sessions.find((x) => x.job === S.job)?.label || 'Sessão' };
      openJob(cur);
    } catch (err) { info({ title: 'Não foi possível usar essa origem', html: `<p>${esc(err.message)}</p>` }); }
  };
  d.showModal();
}

async function onProgressClick(e) {
  const act = e.target.closest('[data-act]')?.dataset.act;
  if (!act) return;
  if (act === 'pause') { await rpc('scan_pause', { job: S.job }); toast('Pausando…'); }
  else if (act === 'stop') await rpc('scan_stop', { job: S.job });
  else if (act === 'resume') {
    try { await rpc('scan_resume', { job: S.job }); stopPoll(); S.poll = setInterval(tick, 500); tick(); }
    catch (err) { toast(err.message); chooseSource(); }
  } else if (act === 'reconnect') {
    try {
      const r = await rpc('session_reconnect', { job: S.job });
      toast(r.renamed ? `Disco encontrado como ${r.renamed}` : 'Disco reconectado');
      const cur = { job: S.job, target: S.status.target, label: S.sessions.find((x) => x.job === S.job)?.label || 'Sessão' };
      openJob(cur);
    } catch { chooseSource(); }
  }
  else if (act === 'relink') { chooseSource();
  } else if (act === 'csv') {
    const path = await ask({ title: 'Exportar para CSV', text: 'Grava os arquivos da vista atual (com os filtros aplicados). O arquivo abre direto no Excel.', value: (S.defaults.home || '') + (S.defaults.sep || '/') + 'guara-achados.csv', ok: 'Exportar' });
    if (path) { try { const r = await rpc('export_csv', { job: S.job, path, query: EX?.query() }); toast(`${r.rows.toLocaleString(LOC())} linhas gravadas`); } catch (err) { toast(err.message); } }
  } else if (act === 'report') {
    const f = await askForm({ title: 'Relatório de recuperação', text: 'Gera um relatório pronto para imprimir, com a origem examinada, os achados e os hashes dos arquivos recuperados. Termine o nome em .pdf para gerar PDF; com .html sai uma página web.', ok: 'Gerar relatório', fields: [
      { name: 'path', label: 'Salvar em', value: (S.defaults.home || '') + (S.defaults.sep || '/') + 'guara-relatorio.html' },
      { name: 'case_id', label: 'Número do caso' }, { name: 'examiner', label: 'Responsável pelo exame' }, { name: 'organization', label: 'Organização' },
      { name: 'notes', label: 'Observações', multiline: true } ] });
    if (f) {
      try {
        const hashes = S.sourceHashes && S.sourceHashes.path === (S.status?.target) ? { md5: S.sourceHashes.md5, sha1: S.sourceHashes.sha1, sha256: S.sourceHashes.sha256 } : null;
        await rpc('export_report', { job: S.job, path: f.path, case: { case_id: f.case_id, examiner: f.examiner, organization: f.organization, notes: f.notes }, hashes });
        toast('Relatório gravado');
      } catch (err) { toast(err.message); }
    }
  } else if (act === 'save') {
    const path = await ask({ title: 'Salvar sessão', text: 'Guarda os resultados para recuperar depois, sem varrer de novo.', value: (S.defaults.home || '') + (S.defaults.sep || '/') + 'guara-sessao.json', ok: 'Salvar' });
    if (path) { try { await rpc('session_save', { job: S.job, path }); toast('Sessão salva'); } catch (err) { toast(err.message); } }
  }
}

async function recover(sel) {
  const f = await askForm({
    title: 'Recuperar para…',
    text: 'Escolha uma pasta em <b>outro disco</b>. O Guará nunca grava no disco de origem, para não sobrescrever o que ainda dá para salvar. <span class="sub">Se a recuperação for interrompida, escolha a mesma pasta de novo: o Guará pula o que já foi conferido.</span>',
    ok: 'Recuperar',
    fields: [
      { name: 'dest', label: 'Pasta de destino', value: (S.defaults.home || '') + (S.defaults.sep || '/') + 'Guara-Recuperados', pick: { directory: true, multiple: false } },
      { name: 'layout', label: 'Organizar os arquivos', value: 'folders', options: [['folders', 'Pastas originais (e por tipo quando não há pasta)'], ['type', 'Por tipo (Fotos, Documentos, Vídeos...)'], ['date', 'Por data (ano e mês)'], ['flat', 'Tudo na mesma pasta']] },
    ],
  });
  if (!f || !f.dest) return;
  const dest = f.dest;
  try {
    const { job } = await rpc('recover_start', { job: S.job, dest, layout: f.layout, ...sel });
    watchAux(job, 'Recuperando arquivos', (st) => {
      const r = st.report || {};
      const ok = (r.recovered || []).length, bad = (r.failed || []).length;
      return `<p><b>${ok}</b> arquivo(s) salvos em <code>${esc(dest)}</code> (${human(r.total_bytes)}).</p>
        ${r.skipped ? `<p class="sub">${r.skipped} já estavam recuperados e conferidos (pulados).</p>` : ''}
        ${bad ? `<div class="notice">${warnSvg}<div>${bad} arquivo(s) não puderam ser gravados.</div></div>` : ''}
        <p class="sub">O arquivo <code>guara-manifesto.json</code> lista MD5, SHA-1 e SHA-256 de cada arquivo para você conferir a integridade.</p>`;
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

// ------------------------------------------------------------------ configurações (janela)
$('#settingsBtn')?.addEventListener('click', () => {
  const d = $('#dlg');
  d.innerHTML = `<form method="dialog"><h2>Configurações</h2>
    <label class="fld"><span>Idioma</span><select class="select" name="lang"><option value="auto">Auto</option><option value="pt">Português</option><option value="en">English</option></select></label>
    <label class="check"><input type="checkbox" name="notify" ${SETTINGS.notify ? 'checked' : ''}><span><b>Notificações</b><br><span class="sub">Avisar quando uma tarefa longa terminar</span></span></label>
    <label class="check"><input type="checkbox" name="maps" ${SETTINGS.maps ? 'checked' : ''}><span><b>Mapas online</b><br><span class="sub">Mostrar mapas das fotos com GPS (usa a internet: baixa blocos do OpenStreetMap)</span></span></label>
    ${tauri?.core?.invoke ? `<label class="check"><input type="checkbox" name="updates" ${SETTINGS.updates ? 'checked' : ''}><span><b>Atualizações automáticas</b><br><span class="sub">Ao abrir, consulta o GitHub para ver se há versão nova (uma consulta, sem enviar dados seus)</span><br><button type="button" class="btn sm" id="updNow">Verificar agora</button></span></label>` : ''}
    <menu><button class="btn" value="cancel">Cancelar</button><button class="btn primary" value="ok">Salvar</button></menu></form>`;
  d.querySelector('[name=lang]').value = SETTINGS.lang;
  d.querySelector('#updNow')?.addEventListener('click', (e) => { e.preventDefault(); checkUpdate(true); });
  d.onclose = () => {
    if (d.returnValue !== 'ok') return;
    const patch = { lang: d.querySelector('[name=lang]').value, notify: d.querySelector('[name=notify]').checked, maps: d.querySelector('[name=maps]').checked };
    if (d.querySelector('[name=updates]')) patch.updates = d.querySelector('[name=updates]').checked;
    setLang(patch.lang);
    patch.ui_lang = currentLang();
    saveSettings(patch);
    EX?.relang?.();
    toast('Salvo');
  };
  d.showModal();
});

// ------------------------------------------------------------------ atualização do app
// O núcleo do app (Tauri) consulta o `latest.json` da release no GitHub, confere a assinatura do
// pacote com a chave pública embutida, instala e reabre. No navegador (`guara ui`) não há nada a fazer.
let UPD = null;

async function checkUpdate(manual) {
  if (!tauri?.core?.invoke) return;
  try {
    UPD = await tauri.core.invoke('update_check');
    renderUpdateCard();
    if (manual) toast(UPD ? `Nova versão ${UPD.version} disponível` : 'Você já está na versão mais recente');
  } catch (e) {
    if (manual) toast(`Não foi possível verificar atualizações: ${e}`);
  }
}

function renderUpdateCard(state) {
  let el = $('#updateCard');
  if (!UPD) { el?.remove(); return; }
  if (!el) { el = document.createElement('div'); el.id = 'updateCard'; el.className = 'update-card'; el.setAttribute('role', 'status'); document.body.appendChild(el); }
  if (state === 'busy') return;
  el.innerHTML = `<div class="upd-head"><b>Nova versão do Guará disponível</b><span class="mono upd-ver">v${esc(UPD.current)} → v${esc(UPD.version)}</span></div>
    ${UPD.auto ? '' : '<p class="sub">Esta instalação vem de um pacote do sistema e não se atualiza sozinha. Baixe a versão nova na página da release:</p><p class="mono upd-url">' + esc(UPD.url) + '</p>'}
    <div class="upd-actions">
      ${UPD.auto ? '<button class="btn primary sm" id="updGo">Atualizar agora</button>' : ''}
      ${UPD.notes ? '<button class="btn sm" id="updNotes">Novidades</button>' : ''}
      <button class="btn sm" id="updLater">Depois</button>
    </div>`;
  $('#updGo')?.addEventListener('click', installUpdate);
  $('#updLater')?.addEventListener('click', () => { UPD = null; el.remove(); });
  $('#updNotes')?.addEventListener('click', () => info({ title: `Guará ${UPD.version}`, html: `<pre class="upd-notes">${esc(UPD.notes)}</pre>` }));
}

async function installUpdate() {
  const el = $('#updateCard');
  el.innerHTML = `<div class="upd-head"><b>Atualizando o Guará…</b></div>
    <div class="bar indet" id="updBar"><i style="width:0"></i></div><p class="sub" id="updMsg">Baixando a atualização…</p>`;
  renderUpdateCard('busy');
  try {
    await tauri.core.invoke('update_install');
  } catch (e) {
    toast(String(e));
    renderUpdateCard();
  }
}

function updateProgress(p) {
  const bar = $('#updBar'); const msg = $('#updMsg');
  if (!bar || !msg) return;
  if (p.installing) { bar.classList.add('indet'); msg.textContent = 'Instalando… o Guará vai reabrir sozinho.'; return; }
  if (p.total) {
    const pct = Math.min(100, Math.round((p.downloaded / p.total) * 100));
    bar.classList.remove('indet');
    bar.firstElementChild.style.width = `${pct}%`;
    msg.textContent = `Baixando… ${pct}%`;
  }
}

// ------------------------------------------------------------------ fechar com varredura em andamento
function askClose() {
  return new Promise((resolve) => {
    const d = $('#dlg');
    d.innerHTML = `<form method="dialog"><h2>Há uma varredura em andamento</h2><p>A sessão é salva automaticamente. Pausando agora, você retoma exatamente do ponto em que parou.</p>
      <menu><button class="btn" value="stay">Continuar varrendo</button><button class="btn" value="quit">Sair sem pausar</button><button class="btn primary" value="pause">Pausar e sair</button></menu></form>`;
    d.onclose = () => resolve(d.returnValue);
    d.showModal();
  });
}

function watchClose() {
  window.addEventListener('beforeunload', (e) => {
    if (S.status?.status === 'running' && !tauri) { e.preventDefault(); e.returnValue = ''; }
  });
  const win = tauri?.window?.getCurrentWindow?.();
  if (!win?.onCloseRequested) return;
  win.onCloseRequested(async (ev) => {
    if (S.status?.status !== 'running') return;
    ev.preventDefault();
    const c = await askClose();
    if (c === 'stay' || !c) return;
    if (c === 'pause') {
      try {
        await rpc('scan_pause', { job: S.job });
        for (let i = 0; i < 40; i++) { const st = await rpc('status', { job: S.job }); if (st.status !== 'running') break; await new Promise((r) => setTimeout(r, 250)); }
        await new Promise((r) => setTimeout(r, 600)); // dá tempo do salvamento automático
      } catch { /* sai mesmo assim */ }
    }
    win.destroy();
  });
}

// ------------------------------------------------------------------ início do app
(async function init() {
  try {
    const v = await rpc('version');
    $('#versionTag').textContent = `v${v.version}`;
    S.defaults = await rpc('defaults');
    try {
      const st = await rpc('settings_get');
      Object.assign(SETTINGS, st);
      applyTheme(st.theme);
      try { localStorage.setItem('guara-theme', st.theme); } catch { /* */ }
    } catch { /* usa o local */ }
    setLang(SETTINGS.lang);
    if (SETTINGS.ui_lang !== currentLang()) saveSettings({ ui_lang: currentLang() });
    watchClose();
    tauri?.event?.listen?.('update-progress', (e) => updateProgress(e.payload));
    if (SETTINGS.updates !== false) checkUpdate(false);
    tauri?.event?.listen?.('elevate-failed', (e) => { toast(String(e.payload || 'Não foi possível reiniciar como administrador')); });
  } catch (e) {
    main.innerHTML = `<div class="page"><div class="notice bad">${warnSvg}<div>Não foi possível falar com o núcleo do Guará: ${esc(e.message)}. Se abriu esta página manualmente, use o endereço completo exibido pelo comando <code>guara ui</code> (ele inclui o token).</div></div></div>`;
    return;
  }
  await loadDevices();
  await loadRecent();
  renderHome();
})();
