/* roamway 控制面 — 原生 ES2017，无框架，无外部依赖 */
(function () {
  'use strict';

  var TOKEN_KEY = 'roamway.token';
  var THEME_KEY = 'roamway.theme';
  var state = { token: localStorage.getItem(TOKEN_KEY) || '', scopes: [], scopesLoaded: false };
  var es = null; // 当前 EventSource

  var $ = function (sel, root) { return (root || document).querySelector(sel); };
  var view = function () { return $('#view'); };
  var el = function (tag, cls, text) {
    var n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text !== undefined && text !== null) n.textContent = text;
    return n;
  };
  var clear = function (node) { while (node.firstChild) node.removeChild(node.firstChild); };
  var btn = function (label, onClick, cls) {
    var b = el('button', cls, label);
    b.type = 'button';
    if (onClick) b.addEventListener('click', onClick);
    return b;
  };

  function applyTheme(theme) {
    var want = theme || localStorage.getItem(THEME_KEY) || 'auto';
    if (want === 'auto') {
      document.documentElement.removeAttribute('data-theme');
    } else {
      document.documentElement.setAttribute('data-theme', want);
    }
  }
  applyTheme();

  function toast(msg) {
    var t = $('#toast');
    t.textContent = msg;
    t.hidden = false;
    clearTimeout(toast._timer);
    toast._timer = setTimeout(function () { t.hidden = true; }, 2600);
  }

  function authHeaders() {
    var h = { 'Content-Type': 'application/json' };
    if (state.token) h['Authorization'] = 'Bearer ' + state.token;
    return h;
  }

  function canWrite(scopes) {
    return (scopes || []).indexOf('admin') >= 0 || (scopes || []).indexOf('run') >= 0;
  }

  function showLogin(message) {
    if (es) { try { es.close(); } catch (e) {} es = null; }
    clear(view());
    var wrap = el('div', 'login-wrap');
    var card = el('div', 'card');
    var h = el('h1', null, 'roamway 控制面');
    var p = el('p', 'muted', message || '输入 daemon token 后继续。');
    var input = el('input');
    input.type = 'password';
    input.placeholder = 'Bearer token';
    input.setAttribute('autocomplete', 'current-password');
    var go = btn('进入控制面', function () {
      var v = input.value.trim();
      if (!v) { toast('请输入 token'); return; }
      state.token = v;
      state.scopes = [];
      state.scopesLoaded = false;
      localStorage.setItem(TOKEN_KEY, v);
      location.hash = '#/agents';
      render();
    }, 'primary');
    go.style.width = '100%';
    card.appendChild(h);
    card.appendChild(p);
    card.appendChild(el('label', null, 'Token'));
    card.appendChild(input);
    card.appendChild(el('div', null, ' '));
    card.appendChild(go);
    wrap.appendChild(card);
    view().appendChild(wrap);
    input.focus();
  }

  function api(method, path, body) {
    var opt = { method: method, headers: authHeaders() };
    if (body !== undefined) opt.body = JSON.stringify(body);
    return fetch(path, opt).then(function (res) {
      if (res.status === 401) {
        state.token = '';
        localStorage.removeItem(TOKEN_KEY);
        showLogin('登录已过期，请重新输入 token。');
        throw new Error('unauthorized');
      }
      return res.json().then(function (data) {
        if (!res.ok) {
          var e = new Error(data && data.error ? data.error : ('HTTP ' + res.status));
          e.status = res.status;
          throw e;
        }
        return data;
      });
    });
  }

  function fmtTime(ts) {
    if (!ts) return '-';
    return new Date(ts * 1000).toLocaleString();
  }
  function durSec(r) {
    if (r.started_at && r.ended_at && r.ended_at >= r.started_at) return (r.ended_at - r.started_at) + 's';
    return '-';
  }

  function navItem(hash, label) {
    var b = btn(label, function () { location.hash = hash; });
    if (('#' + location.hash).replace(/^##/, '#') === hash || location.hash === hash) b.classList.add('active');
    return b;
  }

  function renderNav() {
    var nav = $('#nav');
    clear(nav);
    var items = [['#/agents', 'Agents'], ['#/hosts', 'Hosts'], ['#/schedules', 'Schedules'], ['#/terminals', 'Terminals'], ['#/usage', 'Usage'], ['#/settings', 'Settings']];
    items.forEach(function (it) { nav.appendChild(navItem(it[0], it[1])); });
    $('#topbar').hidden = false;
  }

  function parseRoute() {
    var h = location.hash || '#/agents';
    var parts = h.replace(/^#\/?/, '').split('/');
    return { page: parts[0] || 'agents', id: parts[1] || '', sub: parts[2] || '' };
  }

  function tag(text, cls) {
    var s = el('span', 'tag ' + (cls || text), text);
    return s;
  }

  function field(labelText, inputNode) {
    var wrap = el('div');
    wrap.appendChild(el('label', null, labelText));
    wrap.appendChild(inputNode);
    return wrap;
  }

  /* ---------- Agents 列表 ---------- */
  var agentsTimer = null;
  function agentsPage() {
    var root = el('div');
    var head = el('div', 'spread');
    head.appendChild(el('h1', null, 'Agents'));
    var headBtns = el('div', 'row');
    headBtns.appendChild(btn('刷新', function () { load(); }));
    if (canWrite(state.scopes)) headBtns.appendChild(btn('新建 Agent', function () { location.hash = '#/agents/new'; }, 'primary'));
    head.appendChild(headBtns);
    root.appendChild(head);

    var filters = el('div', 'row');
    var search = el('input');
    search.placeholder = '搜索标题';
    var status = el('select');
    ['', 'running', 'idle', 'failed', 'error', 'archived', 'queued'].forEach(function (s) {
      var o = el('option', null, s === '' ? '全部状态' : s);
      o.value = s;
      status.appendChild(o);
    });
    filters.appendChild(search);
    filters.appendChild(status);
    root.appendChild(filters);

    var tableWrap = el('div');
    root.appendChild(tableWrap);

    var agents = [];
    function draw() {
      clear(tableWrap);
      var q = search.value.trim().toLowerCase();
      var st = status.value;
      var list = agents.filter(function (a) {
        var ok = true;
        if (st && a.state !== st) ok = false;
        if (q && (a.title || '').toLowerCase().indexOf(q) < 0) ok = false;
        return ok;
      });
      if (!list.length) { tableWrap.appendChild(el('p', 'muted', '暂无 agent')); return; }
      var table = el('table');
      var thead = el('thead');
      var tr = el('tr');
      ['标题', '状态', 'Provider', '模型', '耗时', '更新'].forEach(function (t) { var th = el('th', null, t); tr.appendChild(th); });
      thead.appendChild(tr);
      table.appendChild(thead);
      var tbody = el('tbody');
      list.forEach(function (a) {
        var r = el('tr');
        var c1 = el('td');
        var link = el('a', null, a.title || a.id);
        link.href = '#/agents/' + a.id;
        c1.appendChild(link);
        var c2 = el('td'); c2.appendChild(tag(a.state, a.state));
        var c3 = el('td', null, a.kind || '-');
        var c4 = el('td', null, a.model || '-');
        var c5 = el('td', 'mono', durSec(a));
        var c6 = el('td', 'muted', fmtTime(a.updated_at));
        r.appendChild(c1); r.appendChild(c2); r.appendChild(c3); r.appendChild(c4); r.appendChild(c5); r.appendChild(c6);
        tbody.appendChild(r);
      });
      table.appendChild(tbody);
      tableWrap.appendChild(table);
    }

    function load() {
      api('GET', '/v1/agents?all=1').then(function (d) {
        agents = d.agents || [];
        draw();
      }).catch(function (e) { toast(e.message); });
    }
    search.addEventListener('input', draw);
    status.addEventListener('change', draw);
    load();
    if (agentsTimer) clearInterval(agentsTimer);
    agentsTimer = setInterval(load, 30000);
    return root;
  }

  function agentNewPage() {
    var root = el('div');
    root.appendChild(el('h1', null, '新建 Agent'));
    var form = el('div', 'card');
    var title = el('input'); title.placeholder = '标题（可选）';
    var prompt = el('textarea'); prompt.rows = 4; prompt.placeholder = 'prompt';
    var workspace = el('input'); workspace.placeholder = 'workspace 路径（可选）';
    var provider = el('select');
    ['shell', 'codex', 'hermes', 'claude', 'api:deepseek', 'api:qwen'].forEach(function (p) {
      var o = el('option', null, p); o.value = p; provider.appendChild(o);
    });
    form.appendChild(field('标题', title));
    form.appendChild(field('Prompt', prompt));
    form.appendChild(field('Workspace', workspace));
    form.appendChild(field('Provider', provider));
    var actions = el('div', 'row');
    actions.appendChild(btn('创建', function () {
      var body = { prompt: prompt.value, kind: provider.value };
      if (title.value.trim()) body.title = title.value.trim();
      if (workspace.value.trim()) body.workspace = workspace.value.trim();
      api('POST', '/v1/agents', body).then(function (d) {
        var id = d.agent && d.agent.id;
        if (id) location.hash = '#/agents/' + id;
        else render();
      }).catch(function (e) { toast(e.message); });
    }, 'primary'));
    actions.appendChild(btn('返回', function () { location.hash = '#/agents'; }));
    form.appendChild(actions);
    root.appendChild(form);
    return root;
  }

  /* ---------- Agent 详情 ---------- */
  function agentDetail(id) {
    var root = el('div');
    var head = el('div', 'spread');
    head.appendChild(el('h1', null, 'Agent ' + id));
    var actions = el('div', 'row');
    if (canWrite(state.scopes)) {
      actions.appendChild(btn('停止', function () { agentAction('stop', id); }, 'danger'));
      actions.appendChild(btn('重跑', function () { rerunPrompt(id); }));
      actions.appendChild(btn('归档', function () { archiveAgent(id); }));
    }
    actions.appendChild(btn('复制 run_id', function () { copyRunId(id); }));
    actions.appendChild(btn('查看 diff', function () { location.hash = '#/agents/' + id + '/diff'; }));
    head.appendChild(actions);
    root.appendChild(head);

    var meta = el('div', 'card');
    var timeline = el('div', 'card');
    var live = el('div', 'card');
    var liveTitle = el('div', 'spread');
    liveTitle.appendChild(el('h2', null, '实时输出'));
    liveTitle.appendChild(btn('暂停/继续', function () { toggleLive(liveBody); }));
    var liveBody = el('div', 'log-view live-out');
    live.appendChild(liveTitle);
    live.appendChild(liveBody);
    root.appendChild(meta);
    root.appendChild(timeline);
    root.appendChild(live);

    var currentAgent = null;

    function drawMeta(a) {
      clear(meta);
      if (!a) return;
      var dl = el('div', 'grid');
      [['状态', a.state], ['Provider', a.kind], ['模型', a.model || '-'], ['Workspace', a.workspace || '-'], ['session_id', a.session_id || '-'], ['最近 run', a.last_run_id || '-']].forEach(function (kv) {
        var c = el('div');
        c.appendChild(el('div', 'stat-label', kv[0]));
        c.appendChild(el('div', null, kv[1]));
        dl.appendChild(c);
      });
      if (a.last_error) { var e = el('div', 'error log-view'); e.textContent = 'error: ' + a.last_error; meta.appendChild(e); }
      meta.appendChild(dl);
    }

    function drawTimeline(a, runs) {
      clear(timeline);
      timeline.appendChild(el('h2', null, '时间线'));
      var items = [];
      (runs || []).forEach(function (r) {
        items.push({ t: r.created_at, kind: 'run', data: r });
      });
      (a && a.history ? a.history : []).forEach(function (m) {
        items.push({ t: a.updated_at, kind: 'msg', data: m });
      });
      items.sort(function (x, y) { return (y.t || 0) - (x.t || 0); });
      var box = el('div', 'log-view');
      items.forEach(function (it) {
        if (it.kind === 'run') {
          var r = it.data;
          var s = el('span', 'tag ' + r.status, r.status);
          var line = el('div');
          line.appendChild(el('span', 'muted', fmtTime(r.created_at) + ' run ' + r.id + ' '));
          line.appendChild(s);
          if (r.error) { var er = el('div', 'error', r.error); line.appendChild(er); }
          box.appendChild(line);
        } else {
          var m = it.data;
          var d = el('div', m.role === 'user' ? 'user' : 'assistant');
          d.textContent = (m.role === 'user' ? '» ' : '') + m.content;
          box.appendChild(d);
        }
      });
      if (!items.length) box.appendChild(el('p', 'muted', '暂无时间线'));
      timeline.appendChild(box);
    }

    function toggleLive(body) {
      body.classList.toggle('collapse');
      body.classList.toggle('open');
    }

    function load() {
      api('GET', '/v1/agents/' + id).then(function (d) {
        currentAgent = d.agent;
        drawMeta(d.agent);
        drawTimeline(d.agent, d.runs);
        startLive(id);
      }).catch(function (e) { toast(e.message); });
    }
    load();
    return root;
  }

  function startLive(id) {
    if (es) { try { es.close(); } catch (e) {} es = null; }
    var body = $('#view .live-out');
    if (!body) return;
    var url = '/v1/agents/' + id + '/events';
    if (state.token) url += '?token=' + encodeURIComponent(state.token);
    es = new EventSource(url);
    es.onmessage = function (ev) {
      try {
        var d = JSON.parse(ev.data);
        if (d.type === 'log' && d.chunk) {
          var n = el('span', 'assistant');
          n.textContent = d.chunk;
          body.appendChild(n);
          body.scrollTop = body.scrollHeight;
        } else if (d.type === 'state' && d.state) {
          var s = el('div', 'error');
          s.textContent = '[state ' + d.state + ']';
          body.appendChild(s);
        }
      } catch (e) {}
    };
    es.onerror = function () {};
  }

  function agentAction(kind, id) {
    if (kind === 'archive') {
      if (!confirm('归档 agent ' + id + '？')) return;
      api('DELETE', '/v1/agents/' + id).then(function () { location.hash = '#/agents'; }).catch(function (e) { toast(e.message); });
    } else if (kind === 'stop') {
      api('GET', '/v1/agents/' + id).then(function (d) {
        var rid = d.agent && d.agent.last_run_id;
        if (!rid) { toast('暂无运行中的 run'); return; }
        api('POST', '/v1/runs/' + rid + '/stop', {}).then(function () { toast('已停止'); render(); }).catch(function (e) { toast(e.message); });
      }).catch(function (e) { toast(e.message); });
    }
  }
  function archiveAgent(id) { agentAction('archive', id); }

  function rerunPrompt(id) {
    var p = prompt('重跑 prompt：', '');
    if (p === null) return;
    if (!p.trim()) { toast('prompt 不能为空'); return; }
    api('POST', '/v1/agents/' + id + '/run', { prompt: p }).then(function () { toast('已重跑'); render(); }).catch(function (e) { toast(e.message); });
  }
  function copyRunId(id) {
    api('GET', '/v1/agents/' + id).then(function (d) {
      var rid = d.agent && d.agent.last_run_id;
      if (!rid) { toast('暂无 run'); return; }
      if (navigator.clipboard) navigator.clipboard.writeText(rid).then(function () { toast('已复制 ' + rid); });
      else { prompt('run_id：', rid); }
    }).catch(function (e) { toast(e.message); });
  }

  /* ---------- Diff ---------- */
  function diffPage(id) {
    var root = el('div');
    root.appendChild(el('h1', null, 'diff — ' + id));
    var box = el('div');
    root.appendChild(box);
    api('GET', '/v1/agents/' + id + '/diff').then(function (d) {
      clear(box);
      var diff = d.diff || '';
      var lines = diff.split('\n');
      var truncated = lines.length > 2000;
      var shown = truncated ? lines.slice(0, 2000) : lines;
      var wrap = el('div', 'diff');
      shown.forEach(function (line) {
        var s = el('span', 'diff-line');
        if (line.indexOf('+++') === 0 || line.indexOf('---') === 0) s.className += ' diff-meta';
        else if (line.indexOf('@@') === 0) s.className += ' diff-hunk';
        else if (line.indexOf('diff --git') === 0 || line.indexOf('new file') === 0 || line.indexOf('index ') === 0) s.className += ' diff-file';
        else if (line.charAt(0) === '+') s.className += ' diff-add';
        else if (line.charAt(0) === '-') s.className += ' diff-del';
        s.textContent = line;
        wrap.appendChild(s);
      });
      box.appendChild(wrap);
      if (truncated) {
        var note = el('p', 'muted', '文件超过 2000 行，仅显示前 2000 行。');
        box.appendChild(note);
      }
      if (!diff.trim()) box.appendChild(el('p', 'muted', '无 worktree diff。'));
    }).catch(function (e) { box.appendChild(el('p', 'error', e.message)); });
    return root;
  }

  /* ---------- Hosts ---------- */
  function hostsPage() {
    var root = el('div');
    var head = el('div', 'spread');
    head.appendChild(el('h1', null, 'Hosts'));
    if (canWrite(state.scopes)) head.appendChild(btn('添加 Host', function () { showHostForm(root); }, 'primary'));
    root.appendChild(head);
    var list = el('div');
    root.appendChild(list);

    function load() {
      api('GET', '/v1/hosts').then(function (d) {
        clear(list);
        var hosts = d.hosts || [];
        if (!hosts.length) { list.appendChild(el('p', 'muted', '暂无 host')); return; }
        var table = el('table');
        var thead = el('thead'); var tr = el('tr');
        var cols = ['', '名称', 'Endpoint', '在线', 'Token'];
        if (canWrite(state.scopes)) cols.push('操作');
        cols.forEach(function (t) { var th = el('th', null, t); tr.appendChild(th); });
        thead.appendChild(tr); table.appendChild(thead);
        var tbody = el('tbody');
        hosts.forEach(function (h) {
          var r = el('tr');
          var c0 = el('td'); var dot = el('span', 'dot'); dot.style.background = h.last_probe && h.last_probe.ok ? 'var(--ok)' : 'var(--muted)'; c0.appendChild(dot);
          var c1 = el('td', null, h.name);
          var c2 = el('td', 'mono', h.endpoint);
          var c3 = el('td', null, h.last_probe && h.last_probe.ok ? ('ok ' + h.last_probe.latency_ms + 'ms') : (h.last_probe ? h.last_probe.error : '未知'));
          var c4 = el('td', 'mono', h.token || '[set]');
          r.appendChild(c0); r.appendChild(c1); r.appendChild(c2); r.appendChild(c3); r.appendChild(c4);
          if (canWrite(state.scopes)) {
            var c5 = el('td', 'row');
            c5.appendChild(btn('probe', function () { probeHost(h.name, load); }));
            c5.appendChild(btn('删除', function () { deleteHost(h.name, load); }, 'danger'));
            r.appendChild(c5);
          }
          tbody.appendChild(r);
        });
        table.appendChild(tbody);
        list.appendChild(table);
      }).catch(function (e) { toast(e.message); });
    }
    load();
    return root;
  }
  function showHostForm(root) {
    var form = el('div', 'card');
    var name = el('input'); name.placeholder = 'name（小写/数字/短横线）';
    var ep = el('input'); ep.placeholder = 'endpoint（如 192.0.2.10:8796）';
    var token = el('input'); token.type = 'password'; token.placeholder = 'token（提交后只显示 [set]）';
    form.appendChild(field('Name', name));
    form.appendChild(field('Endpoint', ep));
    form.appendChild(field('Token', token));
    var actions = el('div', 'row');
    actions.appendChild(btn('添加', function () {
      var body = { name: name.value.trim(), endpoint: ep.value.trim(), token: token.value };
      api('POST', '/v1/hosts', body).then(function () { toast('已添加'); root.querySelector('.card:last-child') && root.querySelector('.card:last-child').remove(); location.hash = '#/hosts'; render(); }).catch(function (e) { toast(e.message); });
    }, 'primary'));
    actions.appendChild(btn('取消', function () { form.remove(); }));
    form.appendChild(actions);
    root.appendChild(form);
  }
  function probeHost(name, reload) {
    api('POST', '/v1/hosts/' + name + '/probe', {}).then(function (d) { toast(d.ok ? ('在线 ' + (d.latency_ms || '') + 'ms') : (d.error || '失败')); reload(); }).catch(function (e) { toast(e.message); });
  }
  function deleteHost(name, reload) {
    if (!confirm('删除 host ' + name + '？')) return;
    api('DELETE', '/v1/hosts/' + name).then(function () { toast('已删除'); reload(); }).catch(function (e) { toast(e.message); });
  }

  /* ---------- Schedules ---------- */
  function schedulesPage() {
    var root = el('div');
    root.appendChild(el('h1', null, 'Schedules'));
    var list = el('div');
    root.appendChild(list);
    function load() {
      api('GET', '/v1/schedules?all=1').then(function (d) {
        clear(list);
        var ss = d.schedules || [];
        if (!ss.length) { list.appendChild(el('p', 'muted', '暂无 schedule')); return; }
        ss.forEach(function (s) { list.appendChild(scheduleCard(s)); });
      }).catch(function (e) { toast(e.message); });
    }
    load();
    return root;
  }
  function scheduleCard(s) {
    var card = el('div', 'card');
    var head = el('div', 'spread');
    head.appendChild(el('div', null, s.name));
    head.appendChild(tag(s.paused ? 'paused' : (s.enabled ? 'enabled' : 'disabled'), s.paused ? 'archived' : (s.enabled ? 'running' : 'archived')));
    card.appendChild(head);
    card.appendChild(el('div', 'muted', '下次运行: ' + fmtTime(s.next_run_at)));
    card.appendChild(el('div', 'muted', '最近状态: ' + (s.last_status || '-') + '  失败 ' + s.consecutive_failures + ' 次'));
    var actions = el('div', 'row');
    if (canWrite(state.scopes)) {
      actions.appendChild(btn(s.paused ? '恢复' : '暂停', function () {
        var method = s.paused ? 'resume' : 'pause';
        api('POST', '/v1/schedules/' + s.id + '/' + method, {}).then(function () { render(); }).catch(function (e) { toast(e.message); });
      }));
      actions.appendChild(btn(s.enabled ? '禁用' : '启用', function () {
        api('PATCH', '/v1/schedules/' + s.id, { enabled: !s.enabled }).then(function () { render(); }).catch(function (e) { toast(e.message); });
      }));
    }
    actions.appendChild(btn('最近 5 次', function () { showScheduleRuns(s, card); }));
    card.appendChild(actions);
    return card;
  }
  function showScheduleRuns(s, card) {
    api('GET', '/v1/schedules/' + s.id).then(function (d) {
      var old = $('.sched-runs', card);
      if (old) old.remove();
      var box = el('div', 'sched-runs log-view');
      (d.runs || []).forEach(function (r) {
        var line = el('div');
        line.appendChild(el('span', 'muted', r.id + ' ' + r.status + ' ' + fmtTime(r.created_at)));
        if (r.error) line.appendChild(el('div', 'error', r.error));
        box.appendChild(line);
      });
      card.appendChild(box);
    }).catch(function (e) { toast(e.message); });
  }

  /* ---------- Terminals ---------- */
  var termTimer = null;
  function terminalsPage() {
    var root = el('div');
    var head = el('div', 'spread');
    head.appendChild(el('h1', null, 'Terminals'));
    if (canWrite(state.scopes)) head.appendChild(btn('新建终端', function () { createTerminal(); }, 'primary'));
    root.appendChild(head);
    var list = el('div');
    var panel = el('div');
    root.appendChild(list);
    root.appendChild(panel);
    var currentId = null;
    var next = 0;

    function load() {
      api('GET', '/v1/terminals').then(function (d) {
        clear(list);
        var ts = d.terminals || [];
        if (!ts.length) { list.appendChild(el('p', 'muted', '暂无终端')); return; }
        ts.forEach(function (t) {
          var b = btn((t.id || '') + ' ' + (t.state || ''), function () { select(t.id); });
          b.style.width = '100%'; b.style.textAlign = 'left';
          if (currentId === t.id) b.classList.add('primary');
          list.appendChild(b);
        });
      }).catch(function (e) { toast(e.message); });
    }

    function select(id) {
      currentId = id;
      next = 0;
      clear(panel);
      var out = el('div', 'terminal-out');
      panel.appendChild(out);
      if (canWrite(state.scopes)) {
        var sendRow = el('div', 'row');
        var keys = el('input'); keys.placeholder = '按键输入（如 ls -la）';
        var enter = el('input'); enter.type = 'checkbox'; enter.id = 'term-enter'; enter.style.width = 'auto'; enter.style.minHeight = 'auto';
        var send = btn('发送', function () { sendKeys(id, keys.value, enter.checked, sendRow); }, 'primary');
        sendRow.appendChild(keys); sendRow.appendChild(el('label', null, '回车')); sendRow.appendChild(enter); sendRow.appendChild(send);
        panel.appendChild(sendRow);
      }
      poll(id, out);
    }

    function poll(id, out) {
      clearInterval(termTimer);
      function tick() {
        api('GET', '/v1/terminals/' + id + '/output?since=' + next).then(function (d) {
          if (d.output) { var n = el('span'); n.textContent = d.output; out.appendChild(n); out.scrollTop = out.scrollHeight; }
          if (typeof d.next === 'number') next = d.next;
        }).catch(function () {});
      }
      tick();
      termTimer = setInterval(tick, 2000);
    }

    function sendKeys(id, keys, doEnter, row) {
      var first = $('.confirm-send', row);
      if (!first) {
        var c = btn('确认发送？', function () {
          api('POST', '/v1/terminals/' + id + '/keys', { keys: keys, enter: doEnter }).then(function () { c.remove(); toast('已发送'); }).catch(function (e) { toast(e.message); });
        }, 'danger');
        c.classList.add('confirm-send');
        row.appendChild(c);
        return;
      }
      first.remove();
      sendKeys(id, keys, doEnter, row);
    }

    load();
    return root;
  }
  function createTerminal() {
    api('POST', '/v1/terminals', {}).then(function () { toast('已创建终端'); render(); }).catch(function (e) { toast(e.message); });
  }

  /* ---------- Usage ---------- */
  function usagePage() {
    var root = el('div');
    root.appendChild(el('h1', null, 'Usage'));
    var cards = el('div', 'cards');
    var tableWrap = el('div');
    root.appendChild(cards);
    root.appendChild(tableWrap);
    api('GET', '/v1/usage').then(function (d) {
      clear(cards);
      var ov = d.overview || {};
      [['runs', ov.runs], ['prompt_tokens', ov.prompt_tokens], ['completion_tokens', ov.completion_tokens], ['total_tokens', ov.total_tokens]].forEach(function (kv) {
        var c = el('div', 'card');
        c.appendChild(el('div', 'stat', String(kv[1])));
        c.appendChild(el('div', 'stat-label', kv[0]));
        cards.appendChild(c);
      });
      clear(tableWrap);
      tableWrap.appendChild(el('h2', null, '按天 / 模型'));
      var table = el('table');
      var thead = el('thead'); var tr = el('tr');
      ['天', 'Provider', '模型', 'RUNS', 'IN', 'OUT', 'TOTAL'].forEach(function (t) { var th = el('th', null, t); tr.appendChild(th); });
      thead.appendChild(tr); table.appendChild(thead);
      var tbody = el('tbody');
      (d.by_day || d.by_model || []).forEach(function (row) {
        var r = el('tr');
        ['day', 'provider', 'model', 'runs', 'in_tokens', 'out_tokens', 'total_tokens'].forEach(function (k) {
          var td = el('td', 'mono', String(row[k] !== undefined ? row[k] : '-'));
          r.appendChild(td);
        });
        tbody.appendChild(r);
      });
      table.appendChild(tbody);
      tableWrap.appendChild(table);
    }).catch(function (e) { toast(e.message); });
    return root;
  }

  /* ---------- Settings ---------- */
  function settingsPage() {
    var root = el('div');
    root.appendChild(el('h1', null, 'Settings'));

    var who = el('div', 'card');
    root.appendChild(who);
    api('GET', '/v1/auth/whoami').then(function (d) {
      clear(who);
      who.appendChild(el('h2', null, '当前 Token'));
      who.appendChild(el('div', null, 'scope: ' + ((d.scopes && d.scopes.join(',')) || '-')));
      who.appendChild(el('div', 'muted', 'token_id: ' + (d.token_id || '-')));
    }).catch(function () {});

    if (canWrite(state.scopes)) {
      var tok = el('div', 'card');
      root.appendChild(tok);
      var name = el('input'); name.placeholder = 'token 名称（可选）';
      var scope = el('select');
      [['read', 'read'], ['run', 'run'], ['admin', 'admin']].forEach(function (s) { var o = el('option', null, s[0]); o.value = s[1]; scope.appendChild(o); });
      var gen = btn('生成新 token', function () {
        api('POST', '/v1/tokens', { name: name.value.trim(), scopes: [scope.value] }).then(function (d) {
          var out = el('div', 'log-view');
          out.textContent = 'token（仅显示一次）：' + (d.token || '');
          tok.appendChild(out);
        }).catch(function (e) { toast(e.message); });
      }, 'primary');
      tok.appendChild(el('h2', null, '生成 Token'));
      tok.appendChild(field('名称', name));
      tok.appendChild(field('Scope', scope));
      tok.appendChild(gen);
      var tokList = el('div');
      tok.appendChild(tokList);
      api('GET', '/v1/tokens').then(function (d) {
        clear(tokList);
        (d.tokens || []).forEach(function (t) {
          var row = el('div', 'row');
          row.appendChild(el('span', 'mono', t.id + ' ' + (t.name || '')));
          row.appendChild(el('span', 'muted', (t.scopes || []).join(',')));
          row.appendChild(btn('撤销', function () { api('DELETE', '/v1/tokens/' + t.id).then(function () { render(); }).catch(function (e) { toast(e.message); }); }, 'danger'));
          tokList.appendChild(row);
        });
      }).catch(function () {});
    } else {
      var ro = el('div', 'card');
      ro.appendChild(el('h2', null, '生成 Token'));
      ro.appendChild(el('div', 'muted', '只读：当前 token scope 为 read'));
      root.appendChild(ro);
    }

    var prov = el('div', 'card');
    root.appendChild(prov);
    api('GET', '/v1/providers').then(function (d) {
      clear(prov);
      prov.appendChild(el('h2', null, 'Provider 概览'));
      (d.providers || []).forEach(function (p) {
        prov.appendChild(el('div', null, p.id + '  ' + (p.label || '') + '  状态 ' + (p.enabled ? 'enabled' : 'disabled') + '  模型 ' + ((p.models || []).length)));
      });
    }).catch(function () {});

    var meta = el('div', 'card');
    root.appendChild(meta);
    api('GET', '/v1/health').then(function (d) {
      clear(meta);
      meta.appendChild(el('h2', null, 'Daemon'));
      meta.appendChild(el('div', null, 'version ' + (d.version || '-')));
      meta.appendChild(el('div', 'muted', 'uptime ' + (d.uptime_s || 0) + 's'));
      meta.appendChild(el('div', 'muted', 'running ' + d.running + ' / queued ' + d.queued));
    }).catch(function () {});

    var theme = el('div', 'card');
    root.appendChild(theme);
    theme.appendChild(el('h2', null, '主题'));
    var auto = btn('跟随系统', function () { localStorage.setItem(THEME_KEY, 'auto'); applyTheme('auto'); }, 'primary');
    var light = btn('浅色', function () { localStorage.setItem(THEME_KEY, 'light'); applyTheme('light'); });
    var dark = btn('深色', function () { localStorage.setItem(THEME_KEY, 'dark'); applyTheme('dark'); });
    var tr = el('div', 'row');
    tr.appendChild(auto); tr.appendChild(light); tr.appendChild(dark);
    theme.appendChild(tr);
    return root;
  }

  /* ---------- 路由 ---------- */
  function render() {
    if (!state.token) { showLogin(); return; }
    if (!state.scopesLoaded) {
      api('GET', '/v1/auth/whoami').then(function (d) {
        state.scopes = (d && d.scopes) || [];
        state.scopesLoaded = true;
        render();
      }).catch(function () {
        state.scopes = [];
        state.scopesLoaded = true;
        render();
      });
      return;
    }
    renderNav();
    // 页面级定时器统一清理：离开页面后不再发无效请求（agents 30s / terminals 2s）
    if (agentsTimer) { clearInterval(agentsTimer); agentsTimer = null; }
    if (termTimer) { clearInterval(termTimer); termTimer = null; }
    clear(view());
    var route = parseRoute();
    if (route.page === 'agents' && route.id === 'new') view().appendChild(canWrite(state.scopes) ? agentNewPage() : agentsPage());
    else if (route.page === 'agents' && route.sub === 'diff') view().appendChild(diffPage(route.id));
    else if (route.page === 'agents' && route.id) view().appendChild(agentDetail(route.id));
    else if (route.page === 'agents') view().appendChild(agentsPage());
    else if (route.page === 'hosts') view().appendChild(hostsPage());
    else if (route.page === 'schedules') view().appendChild(schedulesPage());
    else if (route.page === 'terminals') view().appendChild(terminalsPage());
    else if (route.page === 'usage') view().appendChild(usagePage());
    else if (route.page === 'settings') view().appendChild(settingsPage());
    else view().appendChild(agentsPage());
  }

  window.addEventListener('hashchange', function () { if (es) { try { es.close(); } catch (e) {} es = null; } render(); });
  render();
})();
