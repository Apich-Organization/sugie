//! Static assets served from the binary — no front-end build pipeline required.

pub const CSS: &str = r#"
:root{
  --bg:#f0edf8; --bg-deep:#e8e2f4;
  --surface:rgba(255,255,255,.65); --surface-alt:rgba(248,245,255,.78); --surface-hi:rgba(255,255,255,.92);
  --border:rgba(168,148,214,.22); --border-mid:rgba(168,148,214,.38); --border-hi:rgba(138,116,184,.55);
  --text-1:#23193a; --text-2:#5e5179; --text-3:#706090;
  --primary:#8b68e4; --primary-dk:#6e50c0; --accent:#a87ef2; --accent-soft:rgba(138,116,184,.12);
  --success:#4a8e6e; --warn:#906030; --danger:#a04060;
  --glow:rgba(138,116,184,.18);
  --radius-md:16px; --radius-lg:24px; --radius-2xl:32px;
  --nav-h:64px; --blur:24px;
  --mono:ui-monospace,"SF Mono","JetBrains Mono",Menlo,Consolas,monospace;
  --sans:-apple-system,BlinkMacSystemFont,"SF Pro Text","Segoe UI",Inter,"PingFang SC","Hiragino Sans GB","Microsoft YaHei",sans-serif;
  --ease:cubic-bezier(.23,1,.32,1);
}
[data-theme="dark"]{
  --bg:#08060f; --bg-deep:#0d0a1c;
  --surface:rgba(18,12,32,.80); --surface-alt:rgba(24,16,44,.85); --surface-hi:rgba(30,20,55,.90);
  --border:rgba(110,80,200,.18); --border-mid:rgba(120,90,215,.32); --border-hi:rgba(130,100,225,.55);
  --text-1:#ece5ff; --text-2:#b2a4d8; --text-3:#9d91b8;
  --primary:#a87ef2; --primary-dk:#8b68e4; --accent:#b5a3d8; --accent-soft:rgba(110,80,200,.18);
  --success:#5a9e7e; --warn:#a07040; --danger:#b05068;
  --glow:rgba(110,80,200,.28);
}
*{box-sizing:border-box}
html,body{margin:0;padding:0}
body{
  font-family:var(--sans); color:var(--text-1); background:var(--bg);
  min-height:100vh; line-height:1.55; -webkit-font-smoothing:antialiased;
  font-size:14px;
}
body::before{
  content:""; position:fixed; inset:0; z-index:-1; pointer-events:none;
  background:
    radial-gradient(760px 520px at 12% -8%, var(--glow), transparent 62%),
    radial-gradient(680px 460px at 88% 4%, rgba(168,148,214,.16), transparent 60%),
    radial-gradient(900px 700px at 50% 108%, rgba(195,178,240,.18), transparent 65%),
    var(--bg);
}
a{color:var(--primary-dk); text-decoration:none}
a:hover{color:var(--primary)}
code,kbd,pre,.mono{font-family:var(--mono)}

/* ---------------- top bar ---------------- */
.topbar{
  position:sticky; top:0; z-index:40; height:var(--nav-h);
  display:flex; align-items:center; gap:20px; padding:0 28px;
  background:rgba(245,242,255,.82); backdrop-filter:blur(var(--blur));
  -webkit-backdrop-filter:blur(var(--blur));
  border-bottom:1px solid var(--border);
}
[data-theme="dark"] .topbar{background:rgba(12,8,22,.88)}
.brand{display:flex; align-items:center; gap:10px; font-weight:650; letter-spacing:-.2px; font-size:15px; color:var(--text-1)}
.brand .logo{
  width:32px;height:32px;border-radius:8px;display:block;flex-shrink:0;object-fit:contain;
}
.brand small{color:var(--text-3); font-weight:500; font-size:11px; letter-spacing:.12em; text-transform:uppercase}
.nav{display:flex; gap:2px; margin-left:14px; flex:1; overflow:auto; scrollbar-width:none}
.nav::-webkit-scrollbar{display:none}
.nav a{
  padding:8px 14px; border-radius:999px; color:var(--text-2); font-size:13px; font-weight:520;
  white-space:nowrap; transition:background .18s var(--ease), color .18s var(--ease);
}
.nav a:hover{background:var(--accent-soft); color:var(--text-1)}
.nav a.active{background:linear-gradient(140deg,var(--primary),var(--accent)); color:#fff; box-shadow:0 8px 20px -10px rgba(110,80,200,.8)}
.topbar-right{display:flex; align-items:center; gap:10px}
.user-chip{
  display:flex; align-items:center; gap:8px; padding:5px 12px 5px 6px; border-radius:999px;
  border:1px solid var(--border); background:var(--surface); font-size:12.5px; color:var(--text-2);
}
.avatar{
  width:24px;height:24px;border-radius:50%;display:grid;place-items:center;font-size:11px;font-weight:700;
  background:linear-gradient(140deg,var(--primary),var(--accent)); color:#fff; text-transform:uppercase;
}
.icon-btn{
  width:34px;height:34px;border-radius:12px;border:1px solid var(--border);background:var(--surface);
  color:var(--text-2); cursor:pointer; display:grid; place-items:center; font-size:14px;
  transition:transform .2s var(--ease), border-color .2s;
}
.icon-btn:hover{transform:translateY(-1px); border-color:var(--border-hi); color:var(--text-1)}

/* ---------------- layout ---------------- */
.page{max-width:1240px; margin:0 auto; padding:32px 28px 72px}
.page-head{display:flex; align-items:flex-end; justify-content:space-between; gap:16px; margin-bottom:24px; flex-wrap:wrap}
.page-head h1{font-size:26px; line-height:1.2; margin:0; letter-spacing:-.6px; font-weight:650}
.page-head p{margin:6px 0 0; color:var(--text-2); font-size:13px}
.eyebrow{font-size:11px; letter-spacing:.16em; text-transform:uppercase; color:var(--text-3); font-weight:600; margin-bottom:8px}

.card{
  background:var(--surface); border:1px solid var(--border); border-radius:var(--radius-lg);
  backdrop-filter:blur(20px); -webkit-backdrop-filter:blur(20px);
  box-shadow:0 20px 50px -34px rgba(60,40,120,.5);
  padding:22px;
}
.card.tight{padding:0; overflow:hidden}
.card-head{display:flex; align-items:center; justify-content:space-between; gap:12px; margin-bottom:16px}
.card-head h2{font-size:14px; margin:0; letter-spacing:-.2px; font-weight:620}
.card-head .hint{color:var(--text-3); font-size:12px}
.grid{display:grid; gap:16px}
.grid.cols-4{grid-template-columns:repeat(4,minmax(0,1fr))}
.grid.cols-3{grid-template-columns:repeat(3,minmax(0,1fr))}
.grid.cols-2{grid-template-columns:repeat(2,minmax(0,1fr))}
.stack{display:flex; flex-direction:column; gap:16px}
.row{display:flex; align-items:center; gap:10px; flex-wrap:wrap}
.spacer{flex:1}

/* ---------------- stats ---------------- */
.stat{
  background:var(--surface); border:1px solid var(--border); border-radius:var(--radius-lg);
  padding:18px 20px; position:relative; overflow:hidden;
  backdrop-filter:blur(20px); -webkit-backdrop-filter:blur(20px);
}
.stat::after{
  content:""; position:absolute; inset:auto -30% -60% auto; width:180px; height:180px;
  background:radial-gradient(circle,var(--glow),transparent 70%); opacity:.9;
}
.stat .label{font-size:11px; letter-spacing:.14em; text-transform:uppercase; color:var(--text-3); font-weight:600}
.stat .value{font-size:34px; font-weight:680; letter-spacing:-1.4px; margin-top:8px; font-variant-numeric:tabular-nums}
.stat .sub{font-size:12px; color:var(--text-2); margin-top:4px}
.stat.accent .value{background:linear-gradient(120deg,var(--primary),var(--accent)); -webkit-background-clip:text; background-clip:text; color:transparent}

/* ---------------- tables ---------------- */
table{width:100%; border-collapse:collapse; font-size:13px}
thead th{
  text-align:left; font-size:11px; letter-spacing:.12em; text-transform:uppercase; color:var(--text-3);
  font-weight:600; padding:12px 18px; border-bottom:1px solid var(--border); white-space:nowrap;
}
tbody td{padding:13px 18px; border-bottom:1px solid var(--border); vertical-align:middle}
tbody tr:last-child td{border-bottom:none}
tbody tr:hover{background:var(--accent-soft)}
.num{font-variant-numeric:tabular-nums; font-family:var(--mono); font-size:12.5px}
.repo-name{font-weight:560; color:var(--text-1); display:flex; align-items:center; gap:8px}
.repo-name .dot{width:6px;height:6px;border-radius:50%;background:var(--success)}
.repo-name .dot.bad{background:var(--danger)}

/* ---------------- badges & buttons ---------------- */
.badge{
  display:inline-flex; align-items:center; gap:5px; padding:3px 9px; border-radius:999px;
  font-size:11px; font-weight:600; letter-spacing:.02em;
  background:var(--accent-soft); color:var(--primary-dk); border:1px solid var(--border);
}
.badge.ok{background:rgba(74,142,110,.12); color:var(--success); border-color:rgba(74,142,110,.28)}
.badge.warn{background:rgba(144,96,48,.12); color:var(--warn); border-color:rgba(144,96,48,.26)}
.badge.bad{background:rgba(160,64,96,.12); color:var(--danger); border-color:rgba(160,64,96,.26)}
.badge.muted{background:transparent; color:var(--text-3)}
.btn{
  display:inline-flex; align-items:center; justify-content:center; gap:7px;
  padding:9px 16px; border-radius:12px; border:1px solid var(--border);
  background:var(--surface-hi); color:var(--text-1); font-size:13px; font-weight:560;
  cursor:pointer; font-family:inherit; white-space:nowrap;
  transition:transform .18s var(--ease), box-shadow .18s, border-color .18s;
}
.btn:hover{transform:translateY(-1px); border-color:var(--border-hi); box-shadow:0 12px 24px -18px rgba(60,40,120,.8)}
.btn.primary{
  background:linear-gradient(140deg,var(--primary),var(--accent)); color:#fff; border-color:transparent;
  box-shadow:0 14px 28px -16px rgba(110,80,200,.9);
}
.btn.ghost{background:transparent}
.btn.danger{color:var(--danger); border-color:rgba(160,64,96,.3); background:rgba(160,64,96,.06)}
.btn.sm{padding:6px 12px; font-size:12px; border-radius:10px}
.btn:disabled{opacity:.5; cursor:not-allowed; transform:none}

/* ---------------- forms ---------------- */
label.field{display:block; margin-bottom:14px}
label.field > span{display:block; font-size:12px; color:var(--text-2); font-weight:560; margin-bottom:6px}
input[type=text],input[type=password],input[type=number],select,textarea{
  width:100%; padding:10px 12px; border-radius:12px; border:1px solid var(--border-mid);
  background:var(--surface-hi); color:var(--text-1); font-size:13px; font-family:inherit;
  transition:border-color .18s, box-shadow .18s;
}
input:focus,select:focus,textarea:focus{
  outline:none; border-color:var(--primary); box-shadow:0 0 0 3px var(--accent-soft);
}
textarea{resize:vertical; line-height:1.6}
textarea.code{font-family:var(--mono); font-size:12.5px; min-height:380px}
.hint{color:var(--text-3); font-size:12px}
.secret-note{font-family:var(--mono); font-size:11.5px; color:var(--text-3)}

/* ---------------- alerts ---------------- */
.alert{padding:12px 16px; border-radius:14px; font-size:13px; border:1px solid var(--border-mid); background:var(--surface-alt); margin-bottom:16px}
.alert.ok{border-color:rgba(74,142,110,.32); background:rgba(74,142,110,.1); color:var(--success)}
.alert.err{border-color:rgba(160,64,96,.32); background:rgba(160,64,96,.1); color:var(--danger)}
.alert.warn{border-color:rgba(144,96,48,.3); background:rgba(144,96,48,.1); color:var(--warn)}

/* ---------------- modal dialog ---------------- */
dialog.modal-dialog{
  border:1px solid var(--border-hi); border-radius:var(--radius-lg);
  background:var(--surface-hi); color:var(--text-1);
  box-shadow:0 24px 64px -16px rgba(40,20,80,.55);
  padding:0; max-width:540px; width:92vw;
  backdrop-filter:blur(24px); -webkit-backdrop-filter:blur(24px);
}
dialog.modal-dialog::backdrop{
  background:rgba(14,10,28,.6);
  backdrop-filter:blur(6px); -webkit-backdrop-filter:blur(6px);
}
.modal-body{padding:24px}
.modal-head{display:flex; align-items:center; justify-content:space-between; margin-bottom:18px; padding-bottom:12px; border-bottom:1px solid var(--border)}
.modal-head h3{margin:0; font-size:16px; font-weight:650; letter-spacing:-.3px}

/* ---------------- tabs ---------------- */
.tabs{display:flex; gap:4px; padding:4px; border-radius:14px; background:var(--surface-alt); border:1px solid var(--border); width:fit-content; margin-bottom:18px}
.tabs button{
  border:none; background:transparent; padding:7px 16px; border-radius:10px; cursor:pointer;
  font-size:12.5px; font-weight:560; color:var(--text-2); font-family:inherit;
}
.tabs button.active{background:var(--surface-hi); color:var(--text-1); box-shadow:0 6px 14px -12px rgba(60,40,120,.9)}
[data-tab-panel]{display:none}
[data-tab-panel].active{display:block}

/* ---------------- chat console ---------------- */
.console{display:grid; grid-template-columns:250px minmax(0,1fr); gap:16px; align-items:start}
.repo-picker{max-height:calc(100vh - 220px); overflow:auto; padding:8px}
.repo-picker a{
  display:flex; align-items:center; justify-content:space-between; gap:8px; padding:9px 12px;
  border-radius:10px; color:var(--text-2); font-size:12.5px;
}
.repo-picker a:hover{background:var(--accent-soft); color:var(--text-1)}
.repo-picker a.active{background:linear-gradient(140deg,var(--primary),var(--accent)); color:#fff}
.repo-picker a.active .badge{background:rgba(255,255,255,.22); color:#fff; border-color:transparent}
.chat{display:flex; flex-direction:column; height:calc(100vh - 220px); min-height:440px; padding:0}
.chat-head{padding:14px 20px; border-bottom:1px solid var(--border); display:flex; align-items:center; gap:10px}
.chat-log{flex:1; overflow:auto; padding:18px 20px; display:flex; flex-direction:column; gap:14px}
.msg{max-width:min(720px,92%); border-radius:16px; padding:12px 14px; font-size:13px}
.msg.user{align-self:flex-end; background:linear-gradient(140deg,var(--primary),var(--accent)); color:#fff; border-bottom-right-radius:6px}
.msg.bot{align-self:flex-start; background:var(--surface-alt); border:1px solid var(--border); border-bottom-left-radius:6px}
.msg .meta{font-size:11px; opacity:.72; margin-bottom:6px; letter-spacing:.04em}
.msg pre{margin:0; white-space:pre-wrap; word-break:break-word; font-size:12.5px; line-height:1.65}
.line-info{color:var(--text-3)}
.line-ok{color:var(--success)}
.line-err{color:var(--danger)}
.line-cmd{color:var(--primary-dk); font-weight:600}
.chat-input{border-top:1px solid var(--border); padding:14px 16px; display:flex; gap:10px; align-items:flex-end}
.chat-input textarea{min-height:44px; max-height:140px; resize:none}
.quick{display:flex; gap:6px; flex-wrap:wrap; padding:0 16px 12px}
.quick button{
  border:1px solid var(--border); background:var(--surface-alt); color:var(--text-2);
  border-radius:999px; padding:4px 11px; font-size:11.5px; cursor:pointer; font-family:var(--mono);
}
.quick button:hover{border-color:var(--border-hi); color:var(--text-1)}
.typing{display:inline-block; width:7px; height:13px; background:var(--primary); animation:blink 1s steps(2) infinite; vertical-align:-2px}
@keyframes blink{50%{opacity:0}}

/* ---------------- login ---------------- */
.auth-wrap{min-height:100vh; display:grid; place-items:center; padding:32px 20px}
.auth-card{width:100%; max-width:400px; padding:32px}
.auth-card h1{font-size:22px; margin:0 0 6px; letter-spacing:-.5px}
.auth-card p.sub{margin:0 0 24px; color:var(--text-2); font-size:13px}
.auth-brand{display:flex; align-items:center; gap:10px; margin-bottom:26px}
.auth-foot{margin-top:18px; font-size:11.5px; color:var(--text-3); text-align:center}

/* ---------------- misc ---------------- */
.kv{display:grid; grid-template-columns:150px 1fr; gap:8px 14px; font-size:12.5px}
.kv dt{color:var(--text-3)}
.kv dd{margin:0; color:var(--text-1)}
.empty{padding:36px; text-align:center; color:var(--text-3); font-size:13px}
.perm-list{display:flex; flex-wrap:wrap; gap:6px}
.pill-check{display:inline-flex; align-items:center; gap:6px; font-size:12px; color:var(--text-2); padding:4px 9px; border:1px solid var(--border); border-radius:999px; cursor:pointer}
.pill-check input{accent-color:var(--primary)}
footer.foot{max-width:1240px; margin:0 auto; padding:0 28px 40px; color:var(--text-3); font-size:11.5px}

/* ---------------- responsive layout fixes ---------------- */
html{overflow-x:hidden}
.page,.page>*,.grid>*,.stack>*,.card,.stat{min-width:0}
.card.tight{overflow-x:auto; overflow-y:hidden; -webkit-overflow-scrolling:touch}
.card.tight table{min-width:640px}
.card.tight::-webkit-scrollbar{height:8px}
.card.tight::-webkit-scrollbar-thumb{background:var(--border-mid); border-radius:8px}
tbody td{overflow-wrap:break-word}
tbody td.hint,tbody td.mono{overflow-wrap:anywhere}
tbody td.nowrap,.nowrap{white-space:nowrap; overflow-wrap:normal}
td.hint,td .hint{max-width:340px}
td .row{flex-wrap:nowrap}
td:last-child .row{justify-content:flex-end}
.split-19{grid-template-columns:minmax(0,1.9fr) minmax(0,1fr); align-items:start}
.split-22{grid-template-columns:minmax(0,2.2fr) minmax(0,1fr); align-items:start}
.kv dd{overflow-wrap:anywhere; min-width:0}
.alert{overflow-wrap:anywhere}
.card-title{font-size:14px; font-weight:620; letter-spacing:-.2px}
.card-sub{color:var(--text-3); font-size:12px; margin-top:2px; max-width:720px}
.form-grid{display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); gap:0 16px}
.form-grid .span-2{grid-column:1/-1}
@media(max-width:700px){.form-grid{grid-template-columns:minmax(0,1fr)}}
.toolbar{display:flex; flex-wrap:wrap; gap:10px; align-items:center; padding:14px 20px 4px}
.toolbar input[type=search]{flex:1; min-width:200px; max-width:360px; padding:8px 12px; border-radius:12px; border:1px solid var(--border-mid); background:var(--surface-hi); color:var(--text-1); font-size:13px; font-family:inherit}
.toolbar input[type=search]:focus{outline:none; border-color:var(--primary); box-shadow:0 0 0 3px var(--accent-soft)}
.toolbar select{width:auto; min-width:130px; padding:8px 12px}
.toolbar .count{margin-left:auto; color:var(--text-3); font-size:12px}
.filter-empty td{padding:28px; text-align:center; color:var(--text-3)}
.check-grid{display:grid; grid-template-columns:repeat(auto-fit,minmax(280px,1fr)); gap:10px}
.check{display:flex; align-items:center; gap:10px; padding:10px 12px; border:1px solid var(--border); border-radius:12px; background:var(--surface-alt); font-size:12.5px; color:var(--text-2); cursor:pointer}
.check:hover{border-color:var(--border-hi); color:var(--text-1)}
.check input{accent-color:var(--primary); width:16px; height:16px; flex-shrink:0}
.page-head>div{min-width:0}
.badge{white-space:nowrap}
.topbar{overflow:hidden}
.user-chip{white-space:nowrap}
@media(max-width:1360px){
  .split-19,.split-22{grid-template-columns:minmax(0,1fr)}
}
@media(max-width:980px){
  .grid.cols-4{grid-template-columns:repeat(2,minmax(0,1fr))}
  .grid.cols-3,.grid.cols-2{grid-template-columns:1fr}
  .console{grid-template-columns:1fr}
  .repo-picker{max-height:none}
  .page{padding:24px 16px 56px}
  .topbar{padding:0 16px; gap:10px}
  .brand small,.user-chip span.name{display:none}
}
@media(max-width:560px){
  .grid.cols-4{grid-template-columns:minmax(0,1fr)}
  .page-head h1{font-size:22px}
}
"#;

pub const JS: &str = r#"
(function () {
  var root = document.documentElement;
  var saved = null;
  try { saved = localStorage.getItem('sugie_theme'); } catch (e) {}
  if (saved) root.setAttribute('data-theme', saved);

  document.querySelectorAll('[data-theme-toggle]').forEach(function (btn) {
    btn.addEventListener('click', function () {
      var next = root.getAttribute('data-theme') === 'dark' ? 'light' : 'dark';
      root.setAttribute('data-theme', next);
      try { localStorage.setItem('sugie_theme', next); } catch (e) {}
    });
  });

  // ---- tabs -------------------------------------------------------------
  document.querySelectorAll('.tabs').forEach(function (group) {
    group.querySelectorAll('button[data-tab]').forEach(function (btn) {
      btn.addEventListener('click', function () {
        var name = btn.getAttribute('data-tab');
        group.querySelectorAll('button[data-tab]').forEach(function (b) {
          b.classList.toggle('active', b === btn);
        });
        document.querySelectorAll('[data-tab-panel]').forEach(function (panel) {
          panel.classList.toggle('active', panel.getAttribute('data-tab-panel') === name);
        });
      });
    });
  });

  // ---- command console --------------------------------------------------
  var form = document.getElementById('cmd-form');
  var log = document.getElementById('chat-log');
  if (form && log) {
    var input = document.getElementById('cmd-input');
    var repoInput = document.getElementById('cmd-repo');

    function line(level, text) {
      var div = document.createElement('div');
      div.className = 'line-' + level;
      div.textContent = text;
      return div;
    }

    function appendJob(command, jobId) {
      var userMsg = document.createElement('div');
      userMsg.className = 'msg user';
      userMsg.innerHTML = '<div class="meta">You · just now</div>';
      var pre = document.createElement('pre');
      pre.textContent = command;
      userMsg.appendChild(pre);
      log.appendChild(userMsg);

      var bot = document.createElement('div');
      bot.className = 'msg bot';
      var meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = 'sugie · running';
      bot.appendChild(meta);
      var body = document.createElement('pre');
      body.appendChild(line('info', 'Connecting to execution log…'));
      bot.appendChild(body);
      log.appendChild(bot);
      log.scrollTop = log.scrollHeight;

      var source = new EventSource('/admin/api/jobs/' + jobId + '/stream');
      var count = 0;
      source.onmessage = function (ev) {
        try {
          var data = JSON.parse(ev.data);
          if (data.done) {
            meta.textContent = 'sugie · ' + (data.ok ? 'completed' : 'failed');
            source.close();
            return;
          }
          if (count === 0) body.innerHTML = '';
          count += 1;
          body.appendChild(line(data.level, data.text));
          body.appendChild(document.createElement('br'));
          log.scrollTop = log.scrollHeight;
        } catch (e) {}
      };
      source.onerror = function () { source.close(); };
    }

    form.addEventListener('submit', function (ev) {
      ev.preventDefault();
      var command = (input.value || '').trim();
      if (!command) return;
      var repo = repoInput.value;
      input.value = '';
      fetch('/admin/api/exec', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ repo: repo, command: command })
      })
        .then(function (r) { return r.json().then(function (j) { return { ok: r.ok, body: j }; }); })
        .then(function (res) {
          if (!res.ok) { alert(res.body.error || 'Execution failed'); return; }
          appendJob(command, res.body.job_id);
        })
        .catch(function () { alert('Network error, please try again'); });
    });

    input.addEventListener('keydown', function (ev) {
      if (ev.key === 'Enter' && !ev.shiftKey) { ev.preventDefault(); form.requestSubmit(); }
    });

    document.querySelectorAll('.quick button').forEach(function (b) {
      b.addEventListener('click', function () {
        input.value = b.getAttribute('data-cmd') || b.textContent;
        input.focus();
      });
    });
  }

  // ---- confirm guards ---------------------------------------------------
  document.querySelectorAll('form[data-confirm]').forEach(function (f) {
    f.addEventListener('submit', function (ev) {
      if (!window.confirm(f.getAttribute('data-confirm'))) ev.preventDefault();
    });
  });

  // ---- WebAuthn / Passkey helpers ---------------------------------------
  function b64urlToBytes(b64url) {
    var padding = '='.repeat((4 - (b64url.length % 4)) % 4);
    var base64 = (b64url + padding).replace(/-/g, '+').replace(/_/g, '/');
    var raw = window.atob(base64);
    var out = new Uint8Array(raw.length);
    for (var i = 0; i < raw.length; i++) {
      out[i] = raw.charCodeAt(i);
    }
    return out;
  }

  function bytesToB64url(buffer) {
    var bytes = new Uint8Array(buffer);
    var binary = '';
    for (var i = 0; i < bytes.byteLength; i++) {
      binary += String.fromCharCode(bytes[i]);
    }
    return window.btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }

  var pkLoginBtn = document.getElementById('btn-passkey-login');
  if (pkLoginBtn) {
    pkLoginBtn.addEventListener('click', function () {
      var errBox = document.getElementById('passkey-err');
      function showErr(msg) {
        if (errBox) {
          errBox.textContent = msg;
          errBox.style.display = 'block';
        } else {
          window.alert(msg);
        }
      }
      if (!window.PublicKeyCredential || !navigator.credentials) {
        showErr('WebAuthn / Passkeys are not supported in this browser or context (HTTPS or localhost required).');
        return;
      }
      if (errBox) errBox.style.display = 'none';
      var origText = pkLoginBtn.textContent;
      pkLoginBtn.disabled = true;
      pkLoginBtn.textContent = 'Waiting for Passkey / Security Key…';
      var pendingToken = pkLoginBtn.getAttribute('data-pending-token') || null;
      var nextUrl = pkLoginBtn.getAttribute('data-next') || '/admin';

      fetch('/admin/api/passkey/auth/start', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ pending_token: pendingToken })
      })
        .then(function (r) { return r.json().then(function (j) { return { ok: r.ok, body: j }; }); })
        .then(function (res) {
          if (!res.ok) throw new Error(res.body.error || 'Failed to start Passkey authentication');
          var pk = res.body.options.publicKey;
          pk.challenge = b64urlToBytes(pk.challenge);
          if (pk.allowCredentials) {
            pk.allowCredentials = pk.allowCredentials.map(function (c) {
              return Object.assign({}, c, { id: b64urlToBytes(c.id) });
            });
          }
          return navigator.credentials.get({ publicKey: pk }).then(function (cred) {
            return { challenge_id: res.body.challenge_id, cred: cred };
          });
        })
        .then(function (step) {
          var cred = step.cred;
          var credential = {
            id: cred.id,
            rawId: bytesToB64url(cred.rawId),
            type: cred.type,
            extensions: cred.getClientExtensionResults ? cred.getClientExtensionResults() : {},
            response: {
              authenticatorData: bytesToB64url(cred.response.authenticatorData),
              clientDataJSON: bytesToB64url(cred.response.clientDataJSON),
              signature: bytesToB64url(cred.response.signature),
              userHandle: cred.response.userHandle ? bytesToB64url(cred.response.userHandle) : null
            }
          };
          return fetch('/admin/api/passkey/auth/finish', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              challenge_id: step.challenge_id,
              pending_token: pendingToken,
              next: nextUrl,
              credential: credential
            })
          });
        })
        .then(function (r) { return r.json().then(function (j) { return { ok: r.ok, body: j }; }); })
        .then(function (fin) {
          if (!fin.ok) throw new Error(fin.body.error || 'Passkey verification failed');
          window.location.href = fin.body.redirect || nextUrl || '/admin';
        })
        .catch(function (err) {
          pkLoginBtn.disabled = false;
          pkLoginBtn.textContent = origText;
          showErr(err && err.message ? err.message : String(err));
        });
    });
  }

  var pkRegBtn = document.getElementById('btn-passkey-register');
  if (pkRegBtn) {
    pkRegBtn.addEventListener('click', function () {
      var msgBox = document.getElementById('passkey-reg-msg');
      var nameInput = document.getElementById('passkey-device-name');
      function showMsg(ok, msg) {
        if (msgBox) {
          msgBox.className = ok ? 'alert ok' : 'alert err';
          msgBox.textContent = msg;
          msgBox.style.display = 'block';
        } else {
          window.alert(msg);
        }
      }
      if (!window.PublicKeyCredential || !navigator.credentials) {
        showMsg(false, 'WebAuthn / Passkeys are not supported in this browser or context (HTTPS or localhost required).');
        return;
      }
      if (msgBox) msgBox.style.display = 'none';
      var deviceName = (nameInput && nameInput.value ? nameInput.value.trim() : '') || 'Passkey';
      var origText = pkRegBtn.textContent;
      pkRegBtn.disabled = true;
      pkRegBtn.textContent = 'Prompting authenticator…';

      fetch('/admin/api/passkey/register/start', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ device_name: deviceName })
      })
        .then(function (r) { return r.json().then(function (j) { return { ok: r.ok, body: j }; }); })
        .then(function (res) {
          if (!res.ok) throw new Error(res.body.error || 'Failed to start Passkey registration');
          var pk = res.body.options.publicKey;
          pk.challenge = b64urlToBytes(pk.challenge);
          pk.user.id = b64urlToBytes(pk.user.id);
          pk.authenticatorSelection = Object.assign({}, pk.authenticatorSelection || {}, {
            residentKey: 'required',
            requireResidentKey: true,
            userVerification: 'required'
          });
          if (pk.excludeCredentials) {
            pk.excludeCredentials = pk.excludeCredentials.map(function (c) {
              return Object.assign({}, c, { id: b64urlToBytes(c.id) });
            });
          }
          return navigator.credentials.create({ publicKey: pk }).then(function (cred) {
            return { challenge_id: res.body.challenge_id, cred: cred };
          });
        })
        .then(function (step) {
          var cred = step.cred;
          var credential = {
            id: cred.id,
            rawId: bytesToB64url(cred.rawId),
            type: cred.type,
            extensions: cred.getClientExtensionResults ? cred.getClientExtensionResults() : {},
            response: {
              attestationObject: bytesToB64url(cred.response.attestationObject),
              clientDataJSON: bytesToB64url(cred.response.clientDataJSON),
              transports: cred.response.getTransports ? cred.response.getTransports() : undefined
            }
          };
          return fetch('/admin/api/passkey/register/finish', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              challenge_id: step.challenge_id,
              device_name: deviceName,
              credential: credential
            })
          });
        })
        .then(function (r) { return r.json().then(function (j) { return { ok: r.ok, body: j }; }); })
        .then(function (fin) {
          if (!fin.ok) throw new Error(fin.body.error || 'Failed to save Passkey');
          window.location.href = '/admin/security?ok=1&msg=' + encodeURIComponent('Passkey "' + deviceName + '" registered successfully');
        })
        .catch(function (err) {
          pkRegBtn.disabled = false;
          pkRegBtn.textContent = origText;
          showMsg(false, err && err.message ? err.message : String(err));
        });
    });
  }
})();

// ---- table search / filter controls (data-filter-table) ----
(function () {
  function apply(tableId) {
    var table = document.getElementById(tableId);
    if (!table) return;
    var controls = document.querySelectorAll('[data-filter-table="' + tableId + '"]');
    var q = '', onlyErrors = false, attrs = {};
    controls.forEach(function (c) {
      var kind = c.getAttribute('data-filter-kind');
      if (kind === 'text') q = c.value.trim().toLowerCase();
      else if (kind === 'errors') onlyErrors = c.checked;
      else if (c.value) attrs[kind] = c.value;
    });
    var shown = 0, rows = table.querySelectorAll('tbody tr:not(.filter-empty)');
    rows.forEach(function (tr) {
      var ok = true;
      if (q && tr.textContent.toLowerCase().indexOf(q) === -1) ok = false;
      if (ok && onlyErrors) {
        var st = tr.getAttribute('data-status');
        ok = st === 'error' || st === 'suspended';
      }
      if (ok) {
        Object.keys(attrs).forEach(function (k) {
          if (tr.getAttribute('data-' + k) !== attrs[k]) ok = false;
        });
      }
      tr.style.display = ok ? '' : 'none';
      if (ok) shown++;
    });
    var tbody = table.querySelector('tbody');
    var empty = tbody.querySelector('tr.filter-empty');
    if (rows.length && shown === 0) {
      if (!empty) {
        empty = document.createElement('tr');
        empty.className = 'filter-empty';
        var td = document.createElement('td');
        td.colSpan = 20;
        td.textContent = 'No rows match the current filters';
        empty.appendChild(td);
        tbody.appendChild(empty);
      }
      empty.style.display = '';
    } else if (empty) {
      empty.style.display = 'none';
    }
  }
  document.querySelectorAll('[data-filter-table]').forEach(function (c) {
    var id = c.getAttribute('data-filter-table');
    c.addEventListener(c.tagName === 'INPUT' && c.type === 'search' ? 'input' : 'change', function () { apply(id); });
  });
})();
"#;

pub const LOGO_SVG: &str = include_str!("../../sugie-mark.svg");
