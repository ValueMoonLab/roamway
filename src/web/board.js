/* roamway 只读看板 — 原生 ES2017，只发 GET，不会向 daemon 发任何写请求。 */
(function () {
  'use strict';

  var TOKEN_KEY = 'roamway.token';
  var STATUSES = ['queued', 'running', 'done', 'failed', 'timeout', 'killed', 'interrupted'];
  var state = {
    token: localStorage.getItem(TOKEN_KEY) || '',
    health: null,
    runs: [],
    agents: [],
    jobs: [],
    metricsText: '',
    connected: false,
    lastRefresh: null,
    inflight: false,
    timer: null
  };

  var $ = function (sel) { return document.querySelector(sel); };
  var el = function (tag, cls, text) {
    var n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text !== undefined && text !== null) n.textContent = text;
    return n;
  };
  var clear = function (node) { while (node.firstChild) node.removeChild(node.firstChild); };

  function authHeaders() {
    var h = {};
    if (state.token) h['Authorization'] = 'Bearer ' + state.token;
    return h;
  }

  function getJson(path) {
    return fetch(path, { headers: authHeaders() }).then(function (res) {
      if (res.status === 401) throw new Error('unauthorized');
      if (!res.ok) throw new Error('HTTP ' + res.status);
      return res.json();
    });
  }

  function getText(path) {
    return fetch(path, { headers: authHeaders() }).then(function (res) {
      if (res.status === 401) throw new Error('unauthorized');
      if (!res.ok) throw new Error('HTTP ' + res.status);
      return res.text();
    });
  }

  function fmtTs(ts) {
    if (!ts) return '-';
    return new Date(ts * 1000).toLocaleString();
  }

  function fmtIso(v) {
    if (!v) return '-';
    var t = Date.parse(v);
    if (!isNaN(t)) return new Date(t).toLocaleString();
    return v;
  }

  function fmtUp(s) {
    if (typeof s !== 'number' || isNaN(s)) return '-';
    var d = Math.floor(s / 86400);
    var h = Math.floor((s % 86400) / 3600);
    var m = Math.floor((s % 3600) / 60);
    if (d > 0) return d + 'd ' + h + 'h';
    if (h > 0) return h + 'h ' + m + 'm';
    return m + 'm ' + (s % 60) + 's';
  }

  function fmtDur(r) {
    if (!r.started_at) return '-';
    var end = r.ended_at || Math.floor(Date.now() / 1000);
    if (end < r.started_at) return '-';
    return (end - r.started_at) + 's';
  }

  function parseCounts(text) {
    if (!text) return null;
    var counts = {};
    STATUSES.forEach(function (st) {
      var re = new RegExp('roamway_runs\\{status="' + st + '"\\} (\\d+)');
      var m = text.match(re);
      counts[st] = m ? parseInt(m[1], 10) : 0;
    });
    return counts;
  }

  function drawTop() {
    var version = (state.health && state.health.version) ? state.health.version : '-';
    $('#board-title').textContent = 'roamway ' + version;
    var dot = $('#board-dot');
    dot.style.background = state.connected ? 'var(--ok)' : 'var(--danger)';
    dot.title = state.connected ? 'online' : 'offline';
    var parts = [state.connected ? 'online' : 'offline'];
    if (state.health && typeof state.health.uptime_s === 'number') parts.push('uptime ' + fmtUp(state.health.uptime_s));
    parts.push('刷新 ' + (state.lastRefresh ? state.lastRefresh.toLocaleTimeString() : '-'));
    $('#board-meta').textContent = parts.join(' · ');
  }

  function drawCounts() {
    var box = $('#board-counts');
    clear(box);
    var counts = parseCounts(state.metricsText || '');
    STATUSES.forEach(function (st) {
      var card = el('div', 'card');
      var n = el('div', 'stat');
      n.textContent = counts ? String(counts[st]) : '-';
      var lab = el('div', 'stat-label');
      lab.textContent = st;
      card.appendChild(n);
      card.appendChild(lab);
      box.appendChild(card);
    });
    var ag = el('div', 'card');
    ag.appendChild(el('div', 'stat', String((state.agents || []).length)));
    ag.appendChild(el('div', 'stat-label', 'agents'));
    box.appendChild(ag);
  }

  function drawRuns() {
    var box = $('#board-runs');
    clear(box);
    var runs = state.runs || [];
    if (!runs.length) { box.appendChild(el('p', 'muted', '暂无 run')); return; }
    var table = el('table');
    var thead = el('thead');
    var tr = el('tr');
    ['ID', 'AGENT', 'STATUS', '用时', 'TITLE'].forEach(function (t) { tr.appendChild(el('th', null, t)); });
    thead.appendChild(tr);
    table.appendChild(thead);
    var tbody = el('tbody');
    runs.forEach(function (r) {
      var row = el('tr');
      if (r.status === 'running') row.classList.add('running-row');
      var c1 = el('td', 'mono', r.id);
      var c2 = el('td', null, r.agent || '-');
      var c3 = el('td');
      var st = r.status || '-';
      var tg = el('span', 'tag ' + st, st);
      c3.appendChild(tg);
      var c4 = el('td', 'mono', fmtDur(r));
      var c5 = el('td', null, r.title || '-');
      row.appendChild(c1); row.appendChild(c2); row.appendChild(c3); row.appendChild(c4); row.appendChild(c5);
      tbody.appendChild(row);
    });
    table.appendChild(tbody);
    box.appendChild(table);
  }

  function jobEvery(j) {
    if (j.every_secs) return 'every ' + j.every_secs + 's';
    if (j.daily) return 'daily ' + j.daily;
    return '-';
  }

  function drawJobs() {
    var box = $('#board-jobs');
    clear(box);
    var jobs = state.jobs || [];
    if (!jobs.length) { box.appendChild(el('p', 'muted', '暂无 job')); return; }
    var table = el('table');
    var thead = el('thead');
    var tr = el('tr');
    ['ID', 'EVERY 或 CRON', '上次状态', 'AFTER', '等待原因'].forEach(function (t) { tr.appendChild(el('th', null, t)); });
    thead.appendChild(tr);
    table.appendChild(thead);
    var tbody = el('tbody');
    jobs.forEach(function (j) {
      var row = el('tr');
      row.appendChild(el('td', 'mono', j.id));
      row.appendChild(el('td', 'mono', jobEvery(j)));
      var last = el('td', null, fmtIso(j.last_run));
      if (j.last_error) last.appendChild(el('div', 'error', j.last_error));
      row.appendChild(last);
      var after = (j.depends_on && j.depends_on.length) ? j.depends_on.join(',') : '-';
      row.appendChild(el('td', 'mono', after));
      row.appendChild(el('td', null, j.last_skip || '-'));
      tbody.appendChild(row);
    });
    table.appendChild(tbody);
    box.appendChild(table);
  }

  function drawMetrics() {
    var lines = (state.metricsText || '').split('\n').filter(function (l) { return l.indexOf('roamway_') === 0; });
    $('#board-metrics').textContent = lines.length ? lines.join('\n') : '-';
  }

  function draw() {
    drawTop();
    drawCounts();
    drawRuns();
    drawJobs();
    drawMetrics();
  }

  function showToken(withHint) {
    var box = $('#token-box');
    var hint = $('#board-token-hint');
    hint.textContent = withHint ? 'token 无效或已过期，请重新输入。' : '输入 daemon token 后回车，只发 GET 请求。';
    var input = $('#board-token');
    input.value = '';
    box.hidden = false;
    input.focus();
  }

  function saveToken() {
    var v = $('#board-token').value.trim();
    if (!v) { $('#board-token').focus(); return; }
    state.token = v;
    state.connected = false;
    localStorage.setItem(TOKEN_KEY, v);
    $('#token-box').hidden = true;
    refreshAll();
  }

  // 局部请求失败不拖垮整个刷新（保留旧数据）；unauthorized 必须冒泡到外层清 token
  function swallowUnlessUnauthorized(err) {
    if (err && err.message === 'unauthorized') throw err;
  }

  function refreshAll() {
    if (state.inflight) return;
    state.inflight = true;
    return getJson('/health').then(function (h) {
      state.health = h;
      state.connected = true;
      return Promise.all([
        getJson('/v1/runs?limit=20').then(function (d) { state.runs = (d && d.runs) || []; }, swallowUnlessUnauthorized),
        getJson('/v1/agents?limit=20').then(function (d) { state.agents = (d && d.agents) || []; }, swallowUnlessUnauthorized),
        getJson('/v1/jobs').then(function (d) { state.jobs = (d && d.jobs) || []; }, swallowUnlessUnauthorized),
        getText('/metrics').then(function (t) { state.metricsText = t || ''; }, swallowUnlessUnauthorized)
      ]);
    }).then(function () {
      state.lastRefresh = new Date();
    }).catch(function (err) {
      if (err && err.message === 'unauthorized') {
        state.token = '';
        localStorage.removeItem(TOKEN_KEY);
        showToken(true);
      } else {
        state.connected = false;
      }
      state.lastRefresh = new Date();
    }).then(function () {
      state.inflight = false;
      draw();
    });
  }

  $('#board-save').addEventListener('click', saveToken);
  $('#board-token').addEventListener('keydown', function (ev) {
    if (ev.key === 'Enter') { ev.preventDefault(); saveToken(); }
  });

  if (state.token) {
    refreshAll();
  } else {
    showToken(false);
    draw();
  }
  state.timer = setInterval(function () {
    if (!document.hidden && state.token) refreshAll();
  }, 5000);
  document.addEventListener('visibilitychange', function () {
    if (!document.hidden && state.token) refreshAll();
  });
})();
