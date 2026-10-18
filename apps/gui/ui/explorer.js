// Explorador de arquivos recuperados: grade de miniaturas (virtualizada, aguenta
// dezenas de milhares de itens), navegação por tipo/data/pasta/câmera, seleção
// estilo gerenciador de arquivos (Ctrl/Shift/teclado), painel de detalhes e
// visualização rápida em tela cheia.

import { renderPreview, kindOf, KIND_LABEL } from './preview.js';

const CAT = {
  image: ['Fotos', '🖼'], document: ['Documentos', '📄'], video: ['Vídeos', '🎞'], audio: ['Áudio', '🎵'],
  archive: ['Compactados', '🗜'], database: ['Bancos de dados', '🗄'], other: ['Outros', '📦'],
};
const CONF = { high: ['alta', 'ok'], medium: ['média', 'warn'], low: ['baixa', 'bad'] };
const MONTHS = ['janeiro', 'fevereiro', 'março', 'abril', 'maio', 'junho', 'julho', 'agosto', 'setembro', 'outubro', 'novembro', 'dezembro'];
const SORTS = [
  ['date', 'Data (mais recentes)'], ['date_asc', 'Data (mais antigas)'], ['name', 'Nome'], ['size', 'Tamanho'], ['confidence', 'Confiança'],
];
const GAP = 10;
const PAD = 12;

const svg = (d, w = 18) => `<svg viewBox="0 0 24 24" width="${w}" height="${w}" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">${d}</svg>`;
const I = {
  grid: svg('<rect x="4" y="4" width="7" height="7" rx="1.5"/><rect x="13" y="4" width="7" height="7" rx="1.5"/><rect x="4" y="13" width="7" height="7" rx="1.5"/><rect x="13" y="13" width="7" height="7" rx="1.5"/>'),
  list: svg('<path d="M8 6h12M8 12h12M8 18h12M4 6h.01M4 12h.01M4 18h.01"/>'),
  filter: svg('<path d="M4 5h16l-6 8v6l-4-2v-4z"/>'),
  chev: svg('<path d="M9 6l6 6-6 6"/>', 14),
  x: svg('<path d="M6 6l12 12M18 6L6 18"/>', 14),
  pin: svg('<path d="M12 21s-6-5.3-6-10a6 6 0 1 1 12 0c0 4.7-6 10-6 10z"/><circle cx="12" cy="11" r="2"/>', 14),
  warn: svg('<path d="M12 3l10 18H2z"/><path d="M12 10v5M12 18v.01"/>', 22),
  close: svg('<path d="M6 6l12 12M18 6L6 18"/>', 20),
};

export function createExplorer(deps) {
  const { host, job, rpc, esc, human, fileUrl, thumbUrl, onRecover, toast } = deps;
  const inlineMedia = !deps.noInlineMedia;
  const openExternal = (it) => {
    toast('Preparando para abrir no player do sistema…');
    rpc('open_external', { job, id: it.id }).then(() => toast('Aberto no player do sistema')).catch((err) => toast(err.message));
  };
  const E = {
    filters: { category: null, year: null, month: null, undated: false, folder: null, camera: null, origin: null, minConf: null, gps: false, dedupe: false, bigOnly: false, mp1: false, query: '' },
    sort: 'date',
    view: 'grid',
    tile: 150,
    items: [],
    total: 0,
    sel: new Set(),
    allFiltered: false,
    anchor: -1,
    curId: null,
    facets: null,
    expanded: new Set(),
    rows: [],
    height: 0,
    rendered: new Map(),
    reloadToken: 0,
    lastReload: 0,
    lastFound: -1,
    wasEnriching: false,
    status: null,
  };
  try {
    E.view = localStorage.getItem('guara-view') || 'grid';
    E.tile = +localStorage.getItem('guara-tile') || 150;
  } catch { /* sem armazenamento */ }

  host.innerHTML = `<div class="xp" data-view="${E.view}">
    <div class="xp-bar">
      <button class="icon-btn xp-navbtn" id="xpNavBtn" aria-label="Filtros">${I.filter}</button>
      <div class="xp-crumb" id="xpCrumb"></div>
      <input class="input xp-q" id="xpQ" type="search" placeholder="Buscar por nome, pasta ou câmera" aria-label="Buscar">
      <select class="select" id="xpSort" aria-label="Ordenar">${SORTS.map(([k, n]) => `<option value="${k}">${n}</option>`).join('')}</select>
      <div class="theme-switch" role="group" aria-label="Modo de exibição"><button data-view="grid" aria-label="Grade" title="Grade de miniaturas">${I.grid}</button><button data-view="list" aria-label="Lista" title="Lista">${I.list}</button></div>
      <input type="range" id="xpSize" min="100" max="300" step="4" aria-label="Tamanho das miniaturas">
    </div>
    <aside class="xp-nav" id="xpNav" aria-label="Navegação"></aside>
    <section class="xp-main">
      <div class="xp-chips" id="xpChips"></div>
      <div class="xp-scroll" id="xpScroll" tabindex="0" role="listbox" aria-multiselectable="true" aria-label="Arquivos encontrados"><div class="xp-sizer" id="xpSizer"></div></div>
      <div class="xp-actions" id="xpActions"></div>
    </section>
    <aside class="xp-insp" id="xpInsp" aria-label="Detalhes"></aside>
    <div class="xp-veil" id="xpVeil"></div>
  </div>`;
  const $ = (s) => host.querySelector(s);
  const root = $('.xp'), scroll = $('#xpScroll'), sizer = $('#xpSizer'), nav = $('#xpNav'), insp = $('#xpInsp'), actions = $('#xpActions');
  $('#xpSort').value = E.sort;
  $('#xpSize').value = E.tile;

  // ------------------------------------------------------------ dados
  const toQuery = () => {
    const f = E.filters;
    const q = {};
    if (f.category) q.category = f.category;
    if (f.year) q.year = f.year;
    if (f.month) q.month = f.month;
    if (f.undated) q.undated = true;
    if (f.folder) q.folder = f.folder;
    if (f.camera) q.camera = f.camera;
    if (f.origin) q.origin = f.origin;
    if (f.minConf) q.min_confidence = f.minConf;
    if (f.gps) q.has_gps = true;
    if (f.dedupe) q.dedupe = true;
    if (f.bigOnly) q.min_size = 30 * 1024;
    if (f.mp1) q.min_mp = 1;
    if (f.query) q.query = f.query;
    return q;
  };
  const anyFilter = () => Object.entries(E.filters).some(([k, v]) => (k === 'query' ? !!v : !!v));

  async function reload() {
    const token = ++E.reloadToken;
    E.lastReload = Date.now();
    const base = { ...toQuery(), sort: E.sort };
    let items = [], total = 0;
    for (;;) {
      let r;
      try { r = await rpc('results', { job, query: { ...base, offset: items.length, limit: 2000 } }); } catch (e) { toast(e.message); return; }
      if (token !== E.reloadToken) return;
      items = items.concat(r.items);
      total = r.total;
      if (!r.items.length || items.length >= total || items.length >= 30000) break;
      if (items.length === 2000) { E.items = items; E.total = total; relayout(); paintAll(); }
    }
    E.items = items;
    E.total = total;
    relayout();
    paintAll();
  }

  async function loadFacets() {
    try { E.facets = await rpc('facets', { job }); } catch { return; }
    paintNav();
  }

  const reloadAll = () => { reload(); loadFacets(); };

  // ------------------------------------------------------------ layout virtual
  // Agrupa por dia; se houver poucos itens por dia, agrupa por mês.
  let groupBy = 'day';
  const dayKey = (t) => (t == null ? 'Sem data' : new Date(t * 1000).toISOString().slice(0, groupBy === 'day' ? 10 : 7));
  const dayLabel = (k) => {
    if (k === 'Sem data') return k;
    return groupBy === 'day'
      ? new Date(k + 'T00:00:00Z').toLocaleDateString('pt-BR', { timeZone: 'UTC', weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' })
      : new Date(k + '-01T00:00:00Z').toLocaleDateString('pt-BR', { timeZone: 'UTC', month: 'long', year: 'numeric' });
  };
  const fmtDate = (t) => (t == null ? 'sem data' : new Date(t * 1000).toLocaleString('pt-BR', { timeZone: 'UTC', dateStyle: 'medium', timeStyle: 'short' }));
  const fmtDay = (t) => (t == null ? '' : new Date(t * 1000).toLocaleDateString('pt-BR', { timeZone: 'UTC', day: '2-digit', month: 'short', year: 'numeric' }));

  function geometry() {
    const W = Math.max(200, scroll.clientWidth - PAD * 2);
    if (E.view === 'list') return { W, cols: 1, tw: W, cap: 0, rowH: 54 };
    const T = E.tile;
    const cols = Math.max(1, Math.floor((W + GAP) / (T + GAP)));
    const tw = Math.floor((W - GAP * (cols - 1)) / cols);
    const cap = tw >= 128 ? 42 : 0;
    return { W, cols, tw, cap, rowH: tw + cap + GAP };
  }

  let G = geometry();
  function relayout() {
    G = geometry();
    const grouped = E.view === 'grid' && (E.sort === 'date' || E.sort === 'date_asc');
    if (grouped) {
      groupBy = 'day';
      const days = new Set(E.items.map((it) => dayKey(it.when)));
      if (E.items.length / Math.max(1, days.size) < 2.5) groupBy = 'month';
    }
    const rows = [];
    let y = PAD;
    let cur = null, rowItems = [];
    const flush = () => {
      if (rowItems.length) { rows.push({ t: 'row', y, h: G.rowH, idx: rowItems }); y += G.rowH; rowItems = []; }
    };
    E.items.forEach((it, i) => {
      if (grouped) {
        const k = dayKey(it.when);
        if (k !== cur) {
          flush();
          cur = k;
          rows.push({ t: 'head', y, h: 40, key: k, start: i });
          y += 40;
        }
      }
      rowItems.push(i);
      if (rowItems.length === G.cols) flush();
    });
    flush();
    if (grouped) {
      // contagem por grupo
      for (let r = 0; r < rows.length; r++) {
        if (rows[r].t !== 'head') continue;
        let n = 0;
        for (let i = rows[r].start; i < E.items.length && dayKey(E.items[i].when) === rows[r].key; i++) n++;
        rows[r].count = n;
      }
    }
    E.rows = rows;
    E.height = y + PAD;
    sizer.style.height = `${E.height}px`;
    for (const el of E.rendered.values()) el.remove();
    E.rendered.clear();
    renderWindow();
  }

  function rowAt(top) {
    let lo = 0, hi = E.rows.length - 1, ans = 0;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (E.rows[mid].y + E.rows[mid].h < top) lo = mid + 1; else { ans = mid; hi = mid - 1; }
    }
    return ans;
  }

  function renderWindow() {
    if (!E.rows.length) { for (const el of E.rendered.values()) el.remove(); E.rendered.clear(); return; }
    const top = Math.max(0, scroll.scrollTop - 400), bottom = scroll.scrollTop + scroll.clientHeight + 400;
    const a = rowAt(top);
    let b = a;
    while (b < E.rows.length && E.rows[b].y < bottom) b++;
    for (const [ri, el] of E.rendered) {
      if (ri < a || ri >= b) { el.remove(); E.rendered.delete(ri); }
    }
    const frag = document.createDocumentFragment();
    for (let ri = a; ri < b; ri++) {
      if (E.rendered.has(ri)) continue;
      const row = E.rows[ri];
      const el = document.createElement('div');
      el.className = `xp-row ${row.t}`;
      el.style.top = `${row.y}px`;
      el.style.height = `${row.h}px`;
      el.dataset.ri = ri;
      el.innerHTML = row.t === 'head' ? headHtml(row) : row.idx.map((i) => (E.view === 'list' ? listHtml(i) : tileHtml(i))).join('');
      E.rendered.set(ri, el);
      frag.appendChild(el);
    }
    sizer.appendChild(frag);
  }

  // ------------------------------------------------------------ itens
  const isSel = (id) => E.allFiltered || E.sel.has(id);

  function headHtml(row) {
    return `<div class="xp-head"><b>${esc(dayLabel(row.key))}</b><small>${row.count} ${row.count === 1 ? 'item' : 'itens'}</small><button class="xp-hsel" data-hsel="${row.start}" data-n="${row.count}" aria-label="Selecionar este grupo">Selecionar grupo</button></div>`;
  }

  function thumbSrc(it, px) {
    const s = px <= 200 ? 256 : px <= 330 ? 384 : 640;
    return thumbUrl(job, it.id, s);
  }

  function tileHtml(i) {
    const it = E.items[i];
    const [cl, cc] = CONF[it.confidence];
    const on = isSel(it.id);
    const [catName, catIco] = CAT[it.category] || CAT.other;
    const media = it.category === 'image'
      ? `<img loading="lazy" decoding="async" alt="" src="${thumbSrc(it, G.tw * (window.devicePixelRatio > 1 ? 1.5 : 1))}" data-th="1">`
      : `<div class="xp-ico c-${it.category}"><span>${catIco}</span><b>${esc((it.ext || '?').toUpperCase().slice(0, 5))}</b></div>`;
    return `<div class="xp-tile" role="option" data-i="${i}" data-id="${it.id}" aria-selected="${on}" ${E.curId === it.id ? 'data-cur="1"' : ''} style="width:${G.tw}px;height:${G.tw + G.cap}px" title="${esc(it.name)}">
      <div class="xp-thumb" style="height:${G.tw}px">${media}
        <span class="xp-chk" data-chk="1" aria-hidden="true"></span>
        <span class="xp-dot ${cc}" title="Confiança ${cl}"></span>
        ${it.gps ? `<span class="xp-gps" title="Tem localização GPS">${I.pin}</span>` : ''}
      </div>
      ${G.cap ? `<div class="xp-cap"><b>${esc(it.name)}</b><small>${human(it.size)}${it.when != null ? ' · ' + esc(fmtDay(it.when)) : ''}</small></div>` : ''}
    </div>`;
  }

  function listHtml(i) {
    const it = E.items[i];
    const [cl, cc] = CONF[it.confidence];
    const on = isSel(it.id);
    const [, catIco] = CAT[it.category] || CAT.other;
    const th = it.category === 'image' ? `<img loading="lazy" decoding="async" alt="" src="${thumbUrl(job, it.id, 128)}" data-th="1">` : `<span class="xp-lico c-${it.category}">${catIco}</span>`;
    return `<div class="xp-lrow" role="option" data-i="${i}" data-id="${it.id}" aria-selected="${on}" ${E.curId === it.id ? 'data-cur="1"' : ''}>
      <span class="xp-chk" data-chk="1" aria-hidden="true"></span><div class="xp-lth">${th}</div>
      <div class="xp-lnm"><b>${esc(it.name)}</b><small>${esc(it.dir || (it.origin === 'carved' ? 'Encontrado por assinatura' : '/'))}</small></div>
      <span class="xp-lcol">${esc(fmtDate(it.when))}</span><span class="xp-lcol num">${human(it.size)}</span><span class="pill ${cc}">${cl}</span>
    </div>`;
  }

  // imagens que falham viram um aviso (arquivo possivelmente danificado)
  scroll.addEventListener('error', (e) => {
    const img = e.target;
    if (img.tagName !== 'IMG' || !img.dataset.th) return;
    const wrap = img.closest('.xp-thumb, .xp-lth');
    img.remove();
    const offline = E.status && E.status.source === false;
    wrap?.insertAdjacentHTML('afterbegin', offline
      ? '<div class="xp-ico c-image" title="Origem desconectada: sem miniatura"><span>🖼</span><b>IMG</b></div>'
      : `<div class="xp-ico c-bad" title="Não foi possível gerar a miniatura (arquivo possivelmente danificado)"><span>${I.warn}</span><b>?</b></div>`);
  }, true);

  // ------------------------------------------------------------ seleção
  const selCount = () => (E.allFiltered ? E.total : E.sel.size);
  const idxOf = (id) => E.items.findIndex((x) => x.id === id);

  function paintSel() {
    const set = (el) => {
      const id = +el.dataset.id;
      el.setAttribute('aria-selected', String(isSel(id)));
      if (E.curId === id) el.dataset.cur = '1'; else delete el.dataset.cur;
    };
    scroll.querySelectorAll('.xp-tile, .xp-lrow').forEach(set);
    paintActions();
  }

  function selectRange(a, b) {
    const [lo, hi] = a < b ? [a, b] : [b, a];
    for (let i = lo; i <= hi; i++) E.sel.add(E.items[i].id);
  }

  function setCursor(i, { extend = false, toggle = false, reveal = false } = {}) {
    if (i < 0 || i >= E.items.length) return;
    const it = E.items[i];
    if (E.allFiltered) { E.allFiltered = false; E.sel = new Set(E.items.map((x) => x.id)); }
    if (extend && E.anchor >= 0) {
      E.sel = new Set();
      selectRange(E.anchor, i);
    } else if (toggle) {
      if (E.sel.has(it.id)) E.sel.delete(it.id); else E.sel.add(it.id);
      E.anchor = i;
    } else {
      E.sel = new Set([it.id]);
      E.anchor = i;
    }
    E.curId = it.id;
    if (reveal) revealIndex(i);
    paintSel();
    schedInspector();
    if (matchMedia('(max-width: 1100px)').matches && !extend && !toggle) root.classList.add('show-insp');
  }

  function revealIndex(i) {
    const ri = E.rows.findIndex((r) => r.t === 'row' && r.idx.includes(i));
    if (ri < 0) return;
    const r = E.rows[ri];
    if (r.y < scroll.scrollTop + 4) scroll.scrollTop = r.y - 8;
    else if (r.y + r.h > scroll.scrollTop + scroll.clientHeight) scroll.scrollTop = r.y + r.h - scroll.clientHeight + 8;
    renderWindow();
  }

  scroll.addEventListener('scroll', () => renderWindow(), { passive: true });
  new ResizeObserver(() => { if (Math.abs(geometry().W - G.W) > 1 || geometry().cols !== G.cols) relayout(); else renderWindow(); }).observe(scroll);

  scroll.addEventListener('click', (e) => {
    const hs = e.target.closest('[data-hsel]');
    if (hs) {
      const start = +hs.dataset.hsel, n = +hs.dataset.n;
      if (E.allFiltered) { E.allFiltered = false; E.sel = new Set(); }
      for (let i = start; i < start + n; i++) E.sel.add(E.items[i].id);
      paintSel();
      return;
    }
    const el = e.target.closest('.xp-tile, .xp-lrow');
    if (!el) return;
    const i = +el.dataset.i;
    if (e.target.closest('[data-chk]')) setCursor(i, { toggle: true });
    else setCursor(i, { extend: e.shiftKey, toggle: e.ctrlKey || e.metaKey });
  });
  scroll.addEventListener('dblclick', (e) => {
    const el = e.target.closest('.xp-tile, .xp-lrow');
    if (el) { setCursor(+el.dataset.i); openLightbox(+el.dataset.i); }
  });
  scroll.addEventListener('keydown', (e) => {
    if (!E.items.length) return;
    let i = idxOf(E.curId);
    if (i < 0) i = 0;
    const cols = G.cols;
    const move = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: cols, ArrowUp: -cols, PageDown: cols * 4, PageUp: -cols * 4 }[e.key];
    if (move != null) {
      e.preventDefault();
      setCursor(Math.max(0, Math.min(E.items.length - 1, idxOf(E.curId) < 0 ? 0 : i + move)), { extend: e.shiftKey, reveal: true });
    } else if (e.key === 'Home') { e.preventDefault(); setCursor(0, { extend: e.shiftKey, reveal: true }); }
    else if (e.key === 'End') { e.preventDefault(); setCursor(E.items.length - 1, { extend: e.shiftKey, reveal: true }); }
    else if (e.key === ' ' || e.key === 'Enter') { e.preventDefault(); if (E.curId == null) setCursor(0); openLightbox(idxOf(E.curId)); }
    else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'a') { e.preventDefault(); E.allFiltered = true; paintSel(); }
    else if (e.key === 'Escape') { E.sel = new Set(); E.allFiltered = false; paintSel(); }
  });

  // ------------------------------------------------------------ barra de ações
  function paintActions() {
    const n = selCount();
    const html = n
      ? `<span class="info"><b>${n.toLocaleString('pt-BR')}</b> selecionado${n === 1 ? '' : 's'}${E.allFiltered ? ' (todos desta vista)' : ''}</span>
         <button class="btn sm" data-a="clear">Limpar seleção</button>
         ${!E.allFiltered && E.total > n ? `<button class="btn sm" data-a="all">Selecionar todos os ${E.total.toLocaleString('pt-BR')}</button>` : ''}
         <button class="btn primary" data-a="rec">Recuperar ${n === 1 ? 'selecionado' : 'selecionados'}</button>`
      : `<span class="info">${E.total.toLocaleString('pt-BR')} ${E.total === 1 ? 'item' : 'itens'} nesta vista${E.total > E.items.length ? ` (mostrando ${E.items.length.toLocaleString('pt-BR')})` : ''}</span>
         ${E.total ? '<button class="btn sm" data-a="all">Selecionar tudo</button><button class="btn primary" data-a="recall">Recuperar tudo desta vista</button>' : ''}`;
    if (actions._h !== html) { actions._h = html; actions.innerHTML = html; }
  }
  actions.addEventListener('click', (e) => {
    const a = e.target.closest('[data-a]')?.dataset.a;
    if (a === 'clear') { E.sel = new Set(); E.allFiltered = false; paintSel(); }
    else if (a === 'all') { E.allFiltered = true; paintSel(); }
    else if (a === 'rec') onRecover(E.allFiltered ? { filter: toQuery() } : { ids: [...E.sel] });
    else if (a === 'recall') onRecover({ filter: toQuery() });
  });

  // ------------------------------------------------------------ navegação lateral
  function paintNav() {
    const f = E.facets;
    if (!f) return;
    const F = E.filters;
    const node = (label, count, attrs, active, depth = 0, extra = '') =>
      `<button class="xp-node${active ? ' on' : ''}" ${attrs} style="--d:${depth}" aria-pressed="${!!active}">${extra}<span class="l">${label}</span><small>${count.toLocaleString('pt-BR')}</small></button>`;
    const sec = (title, body) => (body ? `<div class="xp-sec"><h4>${title}</h4>${body}</div>` : '');

    const cats = Object.keys(CAT).filter((k) => f.categories[k]).map((k) => node(`${CAT[k][1]} ${CAT[k][0]}`, f.categories[k], `data-f="category" data-v="${k}"`, F.category === k)).join('');
    const years = f.years.map((y) => {
      const open = E.expanded.has('y' + y.year) || F.year === y.year;
      return node(String(y.year), y.count, `data-f="year" data-v="${y.year}"`, F.year === y.year && !F.month, 0,
        `<span class="tw" data-tw="y${y.year}" aria-label="Expandir" style="${open ? 'transform:rotate(90deg)' : ''}">${I.chev}</span>`)
        + (open ? y.months.map((m) => node(MONTHS[m.month - 1], m.count, `data-f="month" data-y="${y.year}" data-v="${m.month}"`, F.year === y.year && F.month === m.month, 1)).join('') : '');
    }).join('') + (f.undated ? node('Sem data', f.undated, 'data-f="undated" data-v="1"', F.undated) : '');
    const folders = f.folders.filter((d) => {
      // só mostra se todos os ancestrais estiverem expandidos
      const parts = d.path.split('/');
      for (let k = 1; k < parts.length; k++) if (!E.expanded.has('d' + parts.slice(0, k).join('/')) && !(F.folder || '').startsWith(parts.slice(0, k).join('/'))) return false;
      return true;
    }).map((d) => {
      const hasKids = f.folders.some((o) => o.path.startsWith(d.path + '/'));
      const open = E.expanded.has('d' + d.path) || (F.folder || '').startsWith(d.path + '/');
      return node(esc(d.name), d.count, `data-f="folder" data-v="${esc(d.path)}"`, F.folder === d.path, d.depth,
        hasKids ? `<span class="tw" data-tw="d${esc(d.path)}" style="${open ? 'transform:rotate(90deg)' : ''}">${I.chev}</span>` : '<span class="tw"></span>');
    }).join('')
      + (f.root ? node('/ (raiz)', f.root, 'data-f="folder" data-v="@raiz"', F.folder === '@raiz') : '')
      + (f.carved ? node('Por assinatura', f.carved, 'data-f="folder" data-v="@carved"', F.folder === '@carved') : '');
    const cams = f.cameras.map((c) => node(esc(c.name), c.count, `data-f="camera" data-v="${esc(c.name)}"`, F.camera === c.name)).join('');
    const conf = ['high', 'medium', 'low'].map((k) => node(`Confiança ${CONF[k][0]}`, f.confidence[k], `data-f="minConf" data-v="${k}"`, F.minConf === k)).join('');
    const extra = (f.gps ? node('Com localização (GPS)', f.gps, 'data-f="gps" data-v="1"', F.gps) : '')
      + (f.small && f.small < f.total ? node('Ocultar arquivos pequenos (menos de 30 KB)', f.total - f.small, 'data-f="bigOnly" data-v="1"', F.bigOnly) : '')
      + (f.mp1 ? node('Mais de 1 megapixel', f.mp1, 'data-f="mp1" data-v="1"', F.mp1) : '')
      + (f.duplicates ? node('Ocultar duplicadas', f.duplicates, 'data-f="dedupe" data-v="1"', F.dedupe) : '');

    const html = `<div class="xp-sec">${node('Tudo', f.total, 'data-f="all"', !anyFilter())}</div>`
      + sec('Tipos', cats) + sec('Datas', years) + sec('Pastas', folders) + sec('Câmeras', cams) + sec('Confiança', conf) + sec('Outros', extra);
    if (nav._h !== html) { nav._h = html; const st = nav.scrollTop; nav.innerHTML = html; nav.scrollTop = st; }
  }

  nav.addEventListener('click', (e) => {
    const tw = e.target.closest('[data-tw]');
    if (tw) {
      const k = tw.dataset.tw;
      if (E.expanded.has(k)) E.expanded.delete(k); else E.expanded.add(k);
      nav._h = null; paintNav();
      e.stopPropagation();
      return;
    }
    const b = e.target.closest('.xp-node');
    if (!b) return;
    const F = E.filters;
    const f = b.dataset.f, v = b.dataset.v;
    if (f === 'all') clearFilters(false);
    else if (f === 'year') { const same = F.year === +v && !F.month; F.year = same ? null : +v; F.month = null; F.undated = false; E.expanded.add('y' + v); }
    else if (f === 'month') { const same = F.year === +b.dataset.y && F.month === +v; F.year = same ? null : +b.dataset.y; F.month = same ? null : +v; F.undated = false; }
    else if (f === 'undated') { F.undated = !F.undated; F.year = F.month = null; }
    else if (f === 'gps') F.gps = !F.gps;
    else if (f === 'dedupe') F.dedupe = !F.dedupe;
    else if (f === 'bigOnly') F.bigOnly = !F.bigOnly;
    else if (f === 'mp1') F.mp1 = !F.mp1;
    else F[f] = F[f] === v ? null : v;
    if (f === 'folder' && F.folder && !F.folder.startsWith('@')) E.expanded.add('d' + F.folder);
    nav._h = null;
    root.classList.remove('show-nav');
    changed();
  });

  function clearFilters(refresh = true) {
    E.filters = { category: null, year: null, month: null, undated: false, folder: null, camera: null, origin: null, minConf: null, gps: false, dedupe: false, bigOnly: false, mp1: false, query: '' };
    $('#xpQ').value = '';
    if (refresh) changed(); else changed();
  }

  function changed() {
    E.sel = new Set(); E.allFiltered = false; E.curId = null; E.anchor = -1;
    scroll.scrollTop = 0;
    paintCrumb(); paintChips(); paintNav(); paintInspector();
    reload();
  }

  function paintCrumb() {
    const F = E.filters, parts = [];
    if (F.category) parts.push(CAT[F.category][0]);
    if (F.year) parts.push(String(F.year));
    if (F.month) parts.push(MONTHS[F.month - 1]);
    if (F.undated) parts.push('Sem data');
    if (F.folder) parts.push(F.folder === '@carved' ? 'Por assinatura' : F.folder === '@raiz' ? 'Raiz' : F.folder.split('/').join(' › '));
    if (F.camera) parts.push(F.camera);
    $('#xpCrumb').innerHTML = `<b>${parts.length ? esc(parts.join(' › ')) : 'Todos os arquivos'}</b><small>${E.total.toLocaleString('pt-BR')}</small>`;
  }

  function paintChips() {
    const F = E.filters, chips = [];
    const add = (label, key) => chips.push(`<button class="chip" data-x="${key}">${label} ${I.x}</button>`);
    if (F.category) add(CAT[F.category][0], 'category');
    if (F.year) add(String(F.year), 'year');
    if (F.month) add(MONTHS[F.month - 1], 'month');
    if (F.undated) add('Sem data', 'undated');
    if (F.folder) add(F.folder === '@carved' ? 'Por assinatura' : F.folder === '@raiz' ? 'Raiz' : esc(F.folder), 'folder');
    if (F.camera) add(esc(F.camera), 'camera');
    if (F.minConf) add(`Confiança ${CONF[F.minConf][0]}`, 'minConf');
    if (F.gps) add('Com GPS', 'gps');
    if (F.dedupe) add('Sem duplicadas', 'dedupe');
    if (F.bigOnly) add('Sem arquivos pequenos', 'bigOnly');
    if (F.mp1) add('Mais de 1 MP', 'mp1');
    if (F.query) add(`“${esc(F.query)}”`, 'query');
    $('#xpChips').innerHTML = chips.length ? chips.join('') + '<button class="btn sm" data-x="all">Limpar filtros</button>' : '';
  }
  $('#xpChips').addEventListener('click', (e) => {
    const k = e.target.closest('[data-x]')?.dataset.x;
    if (!k) return;
    if (k === 'all') return clearFilters();
    E.filters[k] = k === 'query' ? '' : k === 'undated' || k === 'gps' || k === 'dedupe' || k === 'bigOnly' || k === 'mp1' ? false : null;
    if (k === 'year') E.filters.month = null;
    if (k === 'query') $('#xpQ').value = '';
    nav._h = null;
    changed();
  });

  // ------------------------------------------------------------ barra superior
  let qt;
  $('#xpQ').addEventListener('input', (e) => { clearTimeout(qt); qt = setTimeout(() => { E.filters.query = e.target.value.trim(); changed(); }, 250); });
  $('#xpSort').addEventListener('change', (e) => { E.sort = e.target.value; reload(); });
  const paintView = () => {
    root.dataset.view = E.view;
    host.querySelectorAll('[data-view]').forEach((b) => { if (b.tagName === 'BUTTON') b.setAttribute('aria-pressed', String(b.dataset.view === E.view)); });
    $('#xpSize').style.display = E.view === 'grid' ? '' : 'none';
  };
  host.querySelectorAll('button[data-view]').forEach((b) => (b.onclick = () => {
    E.view = b.dataset.view;
    try { localStorage.setItem('guara-view', E.view); } catch { /* */ }
    paintView(); relayout();
  }));
  $('#xpSize').addEventListener('input', (e) => {
    E.tile = +e.target.value;
    try { localStorage.setItem('guara-tile', String(E.tile)); } catch { /* */ }
    relayout();
  });
  $('#xpNavBtn').onclick = () => root.classList.toggle('show-nav');
  $('#xpVeil').onclick = () => root.classList.remove('show-nav', 'show-insp');

  // ------------------------------------------------------------ painel de detalhes
  let inspT, stopPv = null, metaToken = 0;
  const schedInspector = () => { clearTimeout(inspT); inspT = setTimeout(paintInspector, 90); };

  function timesHtml(it) {
    const rows = [['Tirada em', it.taken], ['Modificado', it.modified], ['Criado', it.created], ['Acessado', it.accessed]].filter(([, v]) => v != null);
    return rows.map(([k, v]) => `<dt>${k}</dt><dd>${esc(fmtDate(v))}</dd>`).join('');
  }

  function paintInspector() {
    stopPv?.(); stopPv = null;
    const n = selCount();
    const it = E.items[idxOf(E.curId)];
    const close = `<button class="icon-btn xp-insp-x" data-i="close" aria-label="Fechar">${I.close}</button>`;
    if (n > 1) {
      const sizeSum = E.allFiltered ? null : [...E.sel].reduce((a, id) => a + (E.items[idxOf(id)]?.size || 0), 0);
      insp.innerHTML = `${close}<div class="xp-multi"><b>${n.toLocaleString('pt-BR')} itens selecionados</b>${sizeSum != null ? `<span class="sub">${human(sizeSum)} no total</span>` : ''}
        <div class="row"><button class="btn primary" data-i="rec">Recuperar selecionados</button><button class="btn" data-i="clear">Limpar seleção</button></div></div>`;
      return;
    }
    if (!it) {
      insp.innerHTML = `${close}<div class="xp-empty-insp"><div class="viewer"><div class="pv-note">Selecione um arquivo para pré-visualizar.<br><span class="sub">Setas navegam, Espaço abre em tela cheia.</span></div></div></div>`;
      return;
    }
    const [cl, cc] = CONF[it.confidence];
    const url = kindOf(it) === 'image' ? thumbUrl(job, it.id, 640) : fileUrl(job, it.id);
    insp.innerHTML = `${close}<div class="viewer" id="xpPv"></div>
      <div class="xp-ititle"><b>${esc(it.name)}</b><span class="pill ${cc}">${cl}</span></div>
      <div class="sub" style="font-size:12.5px">${esc(KIND_LABEL[kindOf(it)])} · ${human(it.size)}</div>
      ${it.note ? `<div class="pv-note bad" style="text-align:left">${esc(it.note)}</div>` : ''}
      <dl class="kv"><dt>Pasta</dt><dd>${esc(it.dir || (it.origin === 'carved' ? 'Encontrado por assinatura' : '/'))}</dd>${timesHtml(it)}${it.camera ? `<dt>Câmera</dt><dd>${esc(it.camera)}</dd>` : ''}${it.px ? `<dt>Resolução</dt><dd>${it.px[0]} × ${it.px[1]} px</dd>` : ''}</dl>
      <div class="xp-meta" id="xpMeta"></div>
      <div class="row"><button class="btn sm" data-i="full">Tela cheia</button><button class="btn primary sm" data-i="rec1">Recuperar este arquivo</button></div>`;
    stopPv = renderPreview($('#xpPv'), it, { url, thumb: kindOf(it) === 'image', inlineMedia, onOpen: () => openLightbox(idxOf(it.id)), onExternal: () => openExternal(it) });
    // clicar na miniatura abre o modal grande
    $('#xpPv').addEventListener('click', (ev) => {
      if (ev.target.closest('button, a, .pv-pdfbar')) return;
      openLightbox(idxOf(it.id));
    });
    const my = ++metaToken;
    rpc('metadata', { job, id: it.id }).then((m) => {
      if (my !== metaToken || !$('#xpMeta')) return;
      const rows = (m.fields || []).filter(([k]) => !['Dimensões'].includes(k));
      if (!rows.length) return;
      $('#xpMeta').innerHTML = `<h5>Metadados${m.kind ? ` · ${esc(m.kind)}` : ''}</h5><dl class="kv">${rows.map(([k, v]) => `<dt>${esc(k)}</dt><dd>${esc(v)}</dd>`).join('')}</dl>`
        + (m.gps ? `<div class="sub" style="font-size:12.5px">Coordenadas: ${m.gps[0].toFixed(5)}, ${m.gps[1].toFixed(5)}</div>` : '');
    }).catch(() => {});
  }
  insp.addEventListener('click', (e) => {
    const a = e.target.closest('[data-i]')?.dataset.i;
    if (a === 'full') openLightbox(idxOf(E.curId));
    else if (a === 'rec1') onRecover({ ids: [E.curId] });
    else if (a === 'rec') onRecover(E.allFiltered ? { filter: toQuery() } : { ids: [...E.sel] });
    else if (a === 'clear') { E.sel = new Set(); E.allFiltered = false; paintSel(); paintInspector(); }
    else if (a === 'close') root.classList.remove('show-insp');
  });

  // ------------------------------------------------------------ tela cheia
  let stopLb = null;
  function openLightbox(i) {
    const it = E.items[i];
    if (!it) return;
    const d = document.querySelector('#lightbox');
    const [cl] = CONF[it.confidence];
    d.innerHTML = `<div class="lb-head"><button class="icon-btn" id="lbPrev" aria-label="Anterior" ${i <= 0 ? 'disabled' : ''}>‹</button><button class="icon-btn" id="lbNext" aria-label="Próximo" ${i >= E.items.length - 1 ? 'disabled' : ''}>›</button>
        <div class="ttl"><b>${esc(it.name)}</b><small>${esc(KIND_LABEL[kindOf(it)])} · ${human(it.size)}${it.when != null ? ' · ' + esc(fmtDate(it.when)) : ''} · confiança ${cl} · ${i + 1} de ${E.items.length}</small></div>
        <button class="icon-btn" id="lbClose" aria-label="Fechar">${I.close}</button></div>
      <div class="lb-body"><div class="viewer large" id="lbHost"></div></div>
      <div class="lb-foot"><span class="sub">${it.note ? esc(it.note) : '← → para navegar · Esc para fechar'}</span><button class="btn primary sm" id="lbRec">Recuperar este arquivo</button></div>`;
    stopLb?.();
    stopLb = renderPreview(d.querySelector('#lbHost'), it, { url: fileUrl(job, it.id), large: true, inlineMedia, onExternal: () => openExternal(it) });
    const go = (n) => { const t = i + n; if (t >= 0 && t < E.items.length) { setCursor(t, { reveal: true }); openLightbox(t); } };
    d.querySelector('#lbPrev').onclick = () => go(-1);
    d.querySelector('#lbNext').onclick = () => go(1);
    d.querySelector('#lbClose').onclick = () => d.close();
    d.querySelector('#lbRec').onclick = () => { d.close(); onRecover({ ids: [it.id] }); };
    d.onclose = () => { stopLb?.(); stopLb = null; scroll.focus({ preventScroll: true }); };
    d.onkeydown = (e) => { if (e.key === 'ArrowLeft') go(-1); else if (e.key === 'ArrowRight') go(1); };
    if (!d.open) d.showModal();
  }

  // ------------------------------------------------------------ paint geral
  function paintAll() { paintCrumb(); paintChips(); paintActions(); renderWindow(); }

  paintView(); paintCrumb(); paintActions(); paintInspector();
  reloadAll();

  return {
    /** Chamado a cada atualização de status da varredura. */
    onStatus(st) {
      E.status = st;
      const enriching = !!st.enriching;
      const now = Date.now();
      const foundChanged = st.found !== E.lastFound;
      const justDone = st.status !== 'running' && E.running;
      E.running = st.status === 'running';
      if ((foundChanged && now - E.lastReload > 1500) || justDone || (E.wasEnriching && !enriching) || (enriching && now - E.lastReload > 2500)) {
        E.lastFound = st.found;
        reloadAll();
      }
      E.wasEnriching = enriching;
    },
    query: () => toQuery(),
    destroy() { E.reloadToken++; stopPv?.(); stopLb?.(); host.innerHTML = ''; },
    focus() { scroll.focus({ preventScroll: true }); },
  };
}
