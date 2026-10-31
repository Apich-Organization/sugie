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
  /* Lightened for >=4.5:1 contrast on --bg-deep; the previous values were
     ~2.7:1 and made every error message hard to read. */
  --success:#6fc39a; --warn:#e0a860; --danger:#f08aa4;
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
:where(a,button,[role="button"],summary,input,select,textarea):focus-visible{
  outline:2px solid var(--primary); outline-offset:2px; border-radius:6px;
}
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
.nav a.active{background:linear-gradient(140deg,var(--primary-dk),var(--primary)); color:#fff; box-shadow:0 8px 20px -10px rgba(110,80,200,.8)}
[data-theme="dark"] .nav a.active{background:linear-gradient(140deg,#7a55d8,#9b6ef0)}
.topbar-right{display:flex; align-items:center; gap:10px; flex-shrink:0}
@media (max-width:640px){
  .topbar{height:auto; min-height:var(--nav-h); flex-wrap:wrap; gap:8px; padding:10px 16px}
  .nav{flex:1 1 100%; order:3; margin:0; padding-bottom:4px}
  .topbar-right{margin-left:auto}
}
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
/* Anchors cannot be disabled; the pager renders a span with aria-disabled instead. */
.btn[aria-disabled="true"]{opacity:.5; cursor:not-allowed; pointer-events:none; transform:none}

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
/* An advisory row with a call to action on the right. */
.alert.notice-row{display:flex; align-items:center; gap:12px; justify-content:space-between}
.alert.notice-row .btn{flex:none; font-family:inherit}

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
.repo-picker{max-height:calc(100dvh - 220px); overflow:auto; padding:8px}
.repo-picker a{
  display:flex; align-items:center; justify-content:space-between; gap:8px; padding:9px 12px;
  border-radius:10px; color:var(--text-2); font-size:12.5px;
}
.repo-picker a:hover{background:var(--accent-soft); color:var(--text-1)}
.repo-picker a.active{background:linear-gradient(140deg,var(--primary),var(--accent)); color:#fff}
.repo-picker a.active .badge{background:rgba(255,255,255,.22); color:#fff; border-color:transparent}
.repo-filter-wrap{padding:4px 12px 8px}
.repo-filter{
  width:100%; padding:7px 10px; font-size:12.5px; color:var(--text-1);
  background:var(--surface-alt); border:1px solid var(--border); border-radius:9px;
}
.repo-filter::placeholder{color:var(--text-3)}
.repo-filter:focus-visible{outline:none; border-color:var(--primary); box-shadow:0 0 0 3px var(--accent-soft)}
.repo-filter-empty{margin:6px 4px 10px}
/* clearable field: an input with a × that empties it */
.cf{position:relative; display:block; min-width:0}
.cf > input{padding-right:22px}
.cf > textarea{padding-right:22px}
.cf-x{
  position:absolute; right:3px; top:50%; transform:translateY(-50%);
  width:17px; height:17px; padding:0; display:grid; place-items:center;
  border:0; border-radius:50%; background:transparent; color:var(--text-3);
  font-size:13px; line-height:1; cursor:pointer; opacity:.8;
}
.cf-x:hover{background:var(--accent-soft); color:var(--text-1); opacity:1}
.cf-x:focus-visible{outline:2px solid var(--primary); outline-offset:1px; opacity:1}
.cf-x[hidden]{display:none}
.chat{display:flex; flex-direction:column; height:calc(100dvh - 220px); min-height:440px; padding:0}
.chat-head{padding:14px 20px; border-bottom:1px solid var(--border); display:flex; align-items:center; gap:10px}
/* repository statistics strip above the chat log */
.repo-stats{
  display:flex; flex-wrap:wrap; align-items:center; gap:6px 18px;
  padding:12px 20px; border-bottom:1px solid var(--border);
  background:var(--surface-alt); font-size:12px;
}
.repo-stats .rs-item{display:flex; flex-direction:column; gap:2px; min-width:0}
.repo-stats .rs-repo{font-weight:600; color:var(--text-1); font-size:12.5px; margin-right:4px}
.repo-stats .rs-label{color:var(--text-3); font-size:10.5px; letter-spacing:.06em; text-transform:uppercase}
.repo-stats .rs-value{color:var(--text-1); font-weight:600; font-size:13.5px; font-variant-numeric:tabular-nums}
.repo-stats .rs-value.mono{font-weight:500; font-size:12px; color:var(--text-2)}
.repo-stats .rs-muted{color:var(--text-3)}
.repo-stats .rs-error{color:var(--warn); max-width:52ch}
.repo-stats .rs-refresh{
  margin-left:auto; color:var(--primary-dk); font-size:11.5px; font-weight:600;
  padding:4px 10px; border-radius:8px; border:1px solid var(--border);
}
.repo-stats .rs-refresh:hover{background:var(--accent-soft); border-color:var(--primary)}
.chat-log{flex:1; overflow:auto; padding:18px 20px; display:flex; flex-direction:column; gap:14px}
.msg{max-width:min(720px,92%); border-radius:16px; padding:12px 14px; font-size:13px}
.msg.user{align-self:flex-end; background:linear-gradient(140deg,var(--primary),var(--accent)); color:#fff; border-bottom-right-radius:6px}
.msg.bot{align-self:flex-start; background:var(--surface-alt); border:1px solid var(--border); border-bottom-left-radius:6px}
/* A failed job must not look like a successful run with no output. */
.msg.bot.failed{border-color:var(--danger); background:rgba(160,64,96,.12)}
[data-theme="dark"] .msg.bot.failed{background:rgba(240,138,164,.12)}
/* Cancelled runs are a deliberate stop, not a failure: neutral, not red. */
.msg.bot.cancelled{border-color:var(--border-mid); background:var(--surface-alt); opacity:.85}
.msg .meta{display:flex; align-items:center; gap:10px}
.msg .meta .stop-btn{margin-left:auto; font-family:inherit; padding:2px 9px; font-size:11px}
.msg .meta .stop-btn[disabled]{opacity:.5; cursor:default}
/* Inline "are you sure" gate for destructive console commands. */
.msg.confirm{
  align-self:flex-start; display:flex; flex-direction:column; gap:8px;
  max-width:min(560px,92%); padding:12px 14px; border-radius:16px; border-bottom-left-radius:6px;
  background:var(--surface-alt); border:1px solid var(--border-mid);
}
.msg.confirm .meta{font-size:11px; color:var(--text-3); letter-spacing:.04em}
.msg.confirm pre{margin:0; white-space:pre-wrap; word-break:break-word; font-size:12.5px; font-family:var(--mono)}
.msg.confirm .confirm-actions{display:flex; gap:8px}
.msg.confirm .confirm-actions .btn{font-family:inherit}
.msg .meta{font-size:11px; opacity:.72; margin-bottom:6px; letter-spacing:.04em}
.msg pre{margin:0; white-space:pre-wrap; word-break:break-word; font-size:12.5px; line-height:1.65}
.line-info{color:var(--text-3)}
.line-info .btn{margin-left:8px; font-family:inherit; padding:1px 8px; font-size:11px}
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
/* The clearable wrapper takes over the search field's flex sizing. */
.toolbar .cf{flex:1 1 200px; min-width:200px; max-width:360px}
.toolbar .cf > input[type=search]{width:100%; min-width:0; max-width:none; flex:none; padding-right:26px}
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

.card.tight table.audit-table{min-width:1040px}
.audit-table th.col-ts,.audit-table td.col-ts{min-width:145px}
.filter-row th{padding:6px 10px 10px; background:var(--surface-alt); border-bottom:1px solid var(--border-mid); font-weight:normal}
.col-filter{width:100%; min-width:85px; box-sizing:border-box; padding:5px 8px; font-size:11.5px; font-family:inherit; border-radius:8px; border:1px solid var(--border-mid); background:var(--surface-hi); color:var(--text-1); transition:border-color .2s, box-shadow .2s}
.col-filter:focus{border-color:var(--primary); outline:none; box-shadow:0 0 0 2px var(--accent-soft)}
select.col-filter{padding:4px 6px; cursor:pointer}
input[type=date].col-filter{min-width:135px; padding:4px 6px; cursor:pointer}
[data-theme="dark"] input[type=date]::-webkit-calendar-picker-indicator{filter:invert(0.85); cursor:pointer}

/* ================= visual refresh: Bezier motion, living gradient, frosted glass ================= */
:root{
  color-scheme:light;
  --ease-out:cubic-bezier(.22,1,.36,1);
  --ease-spring:cubic-bezier(.34,1.56,.64,1);
  --ease-inout:cubic-bezier(.65,0,.35,1);
  --glass:rgba(255,255,255,.52); --glass-hi:rgba(255,255,255,.72);
  --glass-edge:rgba(255,255,255,.7); --glass-shade:rgba(110,80,200,.10);
  --orb-a:rgba(160,120,255,.55); --orb-b:rgba(110,160,255,.42);
  --orb-c:rgba(236,130,230,.40); --orb-d:rgba(120,90,230,.38);
}
[data-theme="dark"]{
  color-scheme:dark;
  --glass:rgba(26,18,48,.50); --glass-hi:rgba(40,28,72,.66);
  --glass-edge:rgba(190,160,255,.22); --glass-shade:rgba(0,0,0,.45);
  --orb-a:rgba(120,80,240,.50); --orb-b:rgba(60,110,230,.34);
  --orb-c:rgba(200,80,210,.28); --orb-d:rgba(90,60,200,.40);
}
body{background:transparent; position:relative}
html{background:var(--bg)}
body::before{
  background:linear-gradient(135deg,var(--bg) 0%,var(--bg-deep) 50%,var(--bg) 100%);
  background-size:220% 220%; animation:bg-shift 28s var(--ease-inout) infinite alternate;
}
@keyframes bg-shift{0%{background-position:0% 30%}100%{background-position:100% 70%}}

/* ambient layer: drifting gradient orbs + Bezier ribbons */
.ambient-bg{position:fixed; inset:0; z-index:-1; overflow:hidden; pointer-events:none}
.orb{position:absolute; border-radius:50%; filter:blur(70px); opacity:.85; will-change:transform}
.orb-1{width:46vmax;height:46vmax; left:-12vmax; top:-14vmax; background:radial-gradient(circle at 35% 35%,var(--orb-a),transparent 68%); animation:orb-1 26s var(--ease-inout) infinite alternate}
.orb-2{width:40vmax;height:40vmax; right:-10vmax; top:-6vmax; background:radial-gradient(circle at 60% 40%,var(--orb-b),transparent 68%); animation:orb-2 32s var(--ease-inout) infinite alternate}
.orb-3{width:44vmax;height:44vmax; left:22vw; bottom:-22vmax; background:radial-gradient(circle,var(--orb-c),transparent 68%); animation:orb-3 36s var(--ease-inout) infinite alternate}
.orb-4{width:30vmax;height:30vmax; right:4vw; bottom:6vh; background:radial-gradient(circle,var(--orb-d),transparent 70%); animation:orb-4 30s var(--ease-inout) infinite alternate}
@keyframes orb-1{0%{transform:translate(0,0) scale(1)}50%{transform:translate(9vw,7vh) scale(1.12)}100%{transform:translate(3vw,16vh) scale(.94)}}
@keyframes orb-2{0%{transform:translate(0,0) scale(1)}50%{transform:translate(-8vw,10vh) scale(1.1)}100%{transform:translate(-14vw,2vh) scale(.92)}}
@keyframes orb-3{0%{transform:translate(0,0) scale(1)}50%{transform:translate(-10vw,-8vh) scale(1.08)}100%{transform:translate(8vw,-14vh) scale(.96)}}
@keyframes orb-4{0%{transform:translate(0,0) scale(.9)}100%{transform:translate(-12vw,-10vh) scale(1.15)}}
.ambient-bezier{position:absolute; inset:0; width:100%; height:100%}
.bz-wave-1{animation:wave-1 22s var(--ease-inout) infinite alternate; transform-origin:50% 100%}
.bz-wave-2{animation:wave-2 28s var(--ease-inout) infinite alternate; transform-origin:50% 100%}
@keyframes wave-1{0%{transform:translateY(0) scaleY(1)}100%{transform:translateY(-28px) scaleY(1.08)}}
@keyframes wave-2{0%{transform:translateY(0) scaleY(1)}100%{transform:translateY(24px) scaleY(.93)}}
.bz-line{stroke-dasharray:900 1400; animation:line-flow 18s linear infinite}
.bz-line-2{animation-duration:24s; animation-direction:reverse}
.bz-line-3{stroke-dasharray:8 10; animation:none; opacity:.7}
@keyframes line-flow{to{stroke-dashoffset:-2300}}

/* frosted glass surfaces */
.topbar{
  background:var(--glass); backdrop-filter:blur(26px) saturate(180%); -webkit-backdrop-filter:blur(26px) saturate(180%);
  border-bottom:1px solid var(--glass-edge);
  box-shadow:inset 0 -1px 0 rgba(255,255,255,.18), 0 10px 40px -24px var(--glass-shade);
}
[data-theme="dark"] .topbar{background:var(--glass)}
.card,.stat,.alert,.tabs,.user-chip,.icon-btn,.check,.msg.bot{
  backdrop-filter:blur(22px) saturate(170%); -webkit-backdrop-filter:blur(22px) saturate(170%);
}
.card,.stat{
  background:linear-gradient(155deg,var(--glass-hi),var(--glass)) ;
  border:1px solid var(--glass-edge);
  box-shadow:inset 0 1px 0 rgba(255,255,255,.55), inset 0 -1px 0 rgba(255,255,255,.08), 0 24px 60px -34px var(--glass-shade);
  position:relative;
  transition:transform .5s var(--ease-out), box-shadow .5s var(--ease-out), border-color .4s var(--ease-out);
}
[data-theme="dark"] .card,[data-theme="dark"] .stat{box-shadow:inset 0 1px 0 rgba(255,255,255,.10), 0 24px 60px -30px var(--glass-shade)}
.card::before,.stat::before{
  content:""; position:absolute; inset:0; border-radius:inherit; pointer-events:none;
  background:linear-gradient(180deg,rgba(255,255,255,.28),transparent 38%);
  opacity:.55; mask:linear-gradient(#000,transparent 70%); -webkit-mask:linear-gradient(#000,transparent 70%);
}
.stat:hover,.card:not(.chat):not(.tight):hover{
  transform:translateY(-3px); border-color:var(--border-hi);
  box-shadow:inset 0 1px 0 rgba(255,255,255,.6), 0 34px 70px -34px rgba(110,80,200,.55);
}
.stat{isolation:isolate}
.stat::after{display:none}
.stat-bezier{position:absolute; left:0; right:0; bottom:0; width:100%; height:46px; z-index:-1; opacity:.9}
.stat-bz-area{fill:var(--accent-soft); transition:transform .8s var(--ease-out); transform-origin:50% 100%}
.stat-bz-stroke{stroke:var(--primary); stroke-opacity:.55}
.stat:hover .stat-bz-area{transform:scaleY(1.35)}
.stat .value{position:relative}
.alert{background:var(--glass)}
.alert.ok{background:rgba(74,142,110,.14)} .alert.err{background:rgba(160,64,96,.14)} .alert.warn{background:rgba(144,96,48,.14)}
.tabs{background:var(--glass)}
.tabs button{transition:background .3s var(--ease-out), color .3s, box-shadow .3s, transform .3s var(--ease-spring)}
.tabs button:active{transform:scale(.94)}
.tabs button.active{background:var(--glass-hi); box-shadow:inset 0 1px 0 rgba(255,255,255,.6), 0 8px 18px -12px rgba(60,40,120,.8)}
.card.tight{overflow-y:hidden}
dialog.modal-dialog{
  background:linear-gradient(155deg,var(--glass-hi),var(--glass)); border:1px solid var(--glass-edge);
  backdrop-filter:blur(34px) saturate(190%); -webkit-backdrop-filter:blur(34px) saturate(190%);
  box-shadow:inset 0 1px 0 rgba(255,255,255,.6), 0 40px 90px -24px rgba(30,10,80,.6);
}
dialog.modal-dialog[open]{animation:pop-in .5s var(--ease-spring)}
dialog.modal-dialog::backdrop{background:rgba(20,10,50,.38); backdrop-filter:blur(10px) saturate(140%); -webkit-backdrop-filter:blur(10px) saturate(140%); animation:fade-in .35s var(--ease-out)}
@keyframes pop-in{from{opacity:0; transform:translateY(18px) scale(.93)}to{opacity:1; transform:none}}
@keyframes fade-in{from{opacity:0}to{opacity:1}}

/* controls with springy Bezier motion */
.nav a{transition:background .35s var(--ease-out), color .3s, transform .35s var(--ease-spring)}
.nav a:hover{transform:translateY(-1px)}
.nav a.active{box-shadow:inset 0 1px 0 rgba(255,255,255,.35), 0 10px 24px -10px rgba(110,80,200,.85)}
.btn{
  background:var(--glass-hi); backdrop-filter:blur(14px); -webkit-backdrop-filter:blur(14px);
  box-shadow:inset 0 1px 0 rgba(255,255,255,.5);
  transition:transform .35s var(--ease-spring), box-shadow .35s var(--ease-out), border-color .25s, background .25s;
}
.btn:hover{transform:translateY(-2px) scale(1.015)}
.btn:active{transform:translateY(0) scale(.96); transition-duration:.12s}
.btn.primary{
  background:linear-gradient(135deg,var(--primary),var(--accent) 55%,#c690f8); background-size:180% 180%;
  box-shadow:inset 0 1px 0 rgba(255,255,255,.4), 0 16px 30px -14px rgba(110,80,200,.95);
  animation:btn-sheen 7s var(--ease-inout) infinite alternate;
}
@keyframes btn-sheen{from{background-position:0% 50%}to{background-position:100% 50%}}
.icon-btn{background:var(--glass); transition:transform .4s var(--ease-spring), border-color .25s}
.icon-btn:hover{transform:rotate(18deg) scale(1.08)}
.user-chip{background:var(--glass)}
input[type=text],input[type=password],input[type=number],input[type=search],select,textarea{
  background:var(--glass-hi); box-shadow:inset 0 1px 2px rgba(60,40,120,.06);
  transition:border-color .3s var(--ease-out), box-shadow .4s var(--ease-out), transform .3s var(--ease-out);
}
input:focus,select:focus,textarea:focus{box-shadow:0 0 0 4px var(--accent-soft), 0 10px 24px -14px rgba(110,80,200,.7); transform:translateY(-1px)}
tbody tr{transition:background .3s var(--ease-out)}
.repo-picker a{transition:background .3s var(--ease-out), transform .35s var(--ease-spring), color .2s}
.repo-picker a:hover{transform:translateX(3px)}
.badge{backdrop-filter:blur(8px); -webkit-backdrop-filter:blur(8px)}
.msg{animation:msg-in .5s var(--ease-spring)}
@keyframes msg-in{from{opacity:0; transform:translateY(10px) scale(.97)}to{opacity:1; transform:none}}
.msg.bot{background:var(--glass)}
.quick button{transition:transform .3s var(--ease-spring), border-color .2s}
.quick button:hover{transform:translateY(-2px)}

/* entrance choreography */
.page-head,.page>.card,.page>.grid>*,.page>.stack>*,.page>.alert,.page>.console>*,.page>.row{animation:rise .8s var(--ease-out) both}
.page>.grid>*:nth-child(2),.page>.console>*:nth-child(2){animation-delay:.07s}
.page>.grid>*:nth-child(3){animation-delay:.14s}
.page>.grid>*:nth-child(4){animation-delay:.21s}
@keyframes rise{from{opacity:0; transform:translateY(22px)}to{opacity:1; transform:none}}
.auth-card{animation:pop-in .8s var(--ease-spring) both}
.page-head h1{background:linear-gradient(120deg,var(--text-1) 30%,var(--primary-dk)); -webkit-background-clip:text; background-clip:text; color:transparent}

@media(prefers-reduced-motion:reduce){
  *,*::before,*::after{animation:none !important; transition-duration:.01ms !important}
}
@media(max-width:700px){.orb{filter:blur(50px)} .bz-line-3{display:none}}
"#;

pub const JS: &str = r#"
(function () {
  var root = document.documentElement;
  var saved = null;
  try { saved = localStorage.getItem('sugie_theme'); } catch (e) {}
  if (saved) root.setAttribute('data-theme', saved);

  // ---- flash hygiene ----------------------------------------------------
  // Flash banners ride in the query string (`?ok=1&msg=…`). Drop them once the
  // page has painted: a refresh should not replay a stale "Saved" banner, and a
  // crafted link cannot show a message the server never produced.
  (function () {
    try {
      var url = new URL(window.location.href);
      if (url.searchParams.has('msg') || url.searchParams.has('ok')) {
        url.searchParams.delete('msg');
        url.searchParams.delete('ok');
        var next = url.pathname + (url.searchParams.toString() ? '?' + url.searchParams.toString() : '') + url.hash;
        window.history.replaceState({}, '', next);
      }
    } catch (e) {}
  })();

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

  // ---- clearable fields -------------------------------------------------
  // Every `.cf` wrapper renders an input plus a × that appears only when the
  // field has content. `cf-apply` marks GET-form filters, where clearing should
  // take effect immediately instead of waiting for the Filter button.
  document.querySelectorAll('.cf').forEach(function (wrap) {
    var input = wrap.querySelector('input, textarea');
    var clear = wrap.querySelector('.cf-x');
    if (!input || !clear) return;

    function sync() { clear.hidden = input.value.length === 0; }
    sync();

    input.addEventListener('input', sync);

    clear.addEventListener('click', function () {
      if (!input.value) return;
      input.value = '';
      sync();
      input.dispatchEvent(new Event('input', { bubbles: true }));
      input.focus();
      var form = input.form;
      if (wrap.classList.contains('cf-apply') && form && String(form.method).toLowerCase() === 'get') {
        form.submit();
      }
    });

    input.addEventListener('keydown', function (ev) {
      if (ev.key === 'Escape' && input.value) {
        ev.preventDefault();
        input.value = '';
        sync();
        input.dispatchEvent(new Event('input', { bubbles: true }));
      }
    });
  });

  // ---- repository filter ------------------------------------------------
  // Client-side only: the list is server-rendered and can be long.
  var filter = document.querySelector('.repo-filter');
  var picker = document.querySelector('.repo-picker');
  if (filter && picker) {
    var emptyNote = picker.querySelector('.repo-filter-empty');
    filter.addEventListener('input', function () {
      var needle = filter.value.trim().toLowerCase();
      var shown = 0;
      picker.querySelectorAll('a').forEach(function (link) {
        var hit = !needle || link.textContent.toLowerCase().indexOf(needle) !== -1;
        link.style.display = hit ? '' : 'none';
        if (hit) shown++;
      });
      if (emptyNote) emptyNote.style.display = shown === 0 ? '' : 'none';
    });
    // Escape clears the filter so the full list comes back with one keystroke.
    filter.addEventListener('keydown', function (ev) {
      if (ev.key === 'Escape') {
        filter.value = '';
        filter.dispatchEvent(new Event('input'));
      }
    });
  }

  // ---- command console --------------------------------------------------
  var form = document.getElementById('cmd-form');
  var log = document.getElementById('chat-log');
  if (form && log) {
    var input = document.getElementById('cmd-input');
    var repoInput = document.getElementById('cmd-repo');

    function line(level, text) {
      var div = document.createElement('div');
      div.className = 'line-' + level;
      // The list commands end with "Next page: `/issues 10 2`". Turning that
      // into a button is the difference between a hint and a feature. Only these
      // two read-only commands are linked, so a click can never mutate anything.
      var more = level === 'info' && /^Next page: `\/(issues|prs) (\d+) (\d+)`$/.exec(text);
      if (!more) {
        div.textContent = text;
        return div;
      }
      div.textContent = text.replace(/`/g, '') + ' ';
      var btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'btn sm ghost';
      btn.textContent = 'Show more';
      btn.addEventListener('click', function () {
        btn.disabled = true;
        dispatch('/' + more[1] + ' ' + more[2] + ' ' + more[3], repoInput.value);
      });
      div.appendChild(btn);
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

      // Stop button: cancelling is a server-side abort, so the stream still has
      // to be read until it reports the terminal state.
      var stop = document.createElement('button');
      stop.type = 'button';
      stop.className = 'btn sm ghost stop-btn';
      stop.textContent = 'Stop';
      stop.setAttribute('aria-label', 'Stop this command');
      stop.addEventListener('click', function () {
        stop.disabled = true;
        stop.textContent = 'Stopping…';
        fetch('/admin/api/jobs/' + jobId + '/cancel', {
          method: 'POST',
          credentials: 'same-origin',
          headers: { 'Content-Type': 'application/json' }
        }).then(function (res) {
          // 409 means it finished between the click and the request: let the
          // stream deliver the real outcome rather than showing an error.
          if (!res.ok && res.status !== 409) throw new Error('cancel failed');
        }).catch(function () {
          stop.disabled = false;
          stop.textContent = 'Stop';
        });
      });
      meta.appendChild(stop);

      function settle(text, failed) {
        meta.textContent = 'sugie · ' + text;
        if (failed) bot.classList.add('failed');
        if (count === 0) {
          body.innerHTML = '';
          body.appendChild(line(failed ? 'err' : 'info', failed ? 'Command failed (no output)' : '(no output)'));
        }
        source.close();
      }
      source.onmessage = function (ev) {
        try {
          var data = JSON.parse(ev.data);
          if (data.done) {
            // Always replace the placeholder, even for a job that produced no
            // output — otherwise "Connecting…" stays on screen forever.
            if (data.cancelled) {
              bot.classList.add('cancelled');
              settle('cancelled', false);
            } else {
              settle(data.ok ? 'completed' : 'failed', !data.ok);
            }
            return;
          }
          if (count === 0) body.innerHTML = '';
          count += 1;
          body.appendChild(line(data.level, data.text));
          body.appendChild(document.createElement('br'));
          log.scrollTop = log.scrollHeight;
        } catch (e) {}
      };
      source.onerror = function () { settle('disconnected', false); };
    }

    var sending = false;
    function showError(msg) {
      var div = document.createElement('div');
      div.className = 'msg bot failed';
      var meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = 'sugie · error';
      var body = document.createElement('pre');
      body.appendChild(line('err', msg));
      div.appendChild(meta);
      div.appendChild(body);
      log.appendChild(div);
      log.scrollTop = log.scrollHeight;
    }

    // Client-side only gate for destructive commands: the /admin/api/exec
    // contract is unchanged, we just refuse to fire on a single Enter.
    var MUTATING = ['close', 'move', 'duplicate', 'stale', 'retest', 'assign', 'unassign', 'unlabel'];

    function commandName(raw) {
      var first = raw.replace(/^\s+/, '').split(/\s+/)[0] || '';
      return first.replace(/^\//, '').toLowerCase();
    }

    function isMutating(raw) {
      return MUTATING.indexOf(commandName(raw)) !== -1;
    }

    // Tracks the dismiss handler of the confirmation currently on screen so a
    // second request (or Escape) cannot leave a stale keydown listener behind.
    var pendingDismiss = null;

    function askConfirm(command, repo, run) {
      if (pendingDismiss) pendingDismiss();

      var box = document.createElement('div');
      box.className = 'msg confirm';
      box.setAttribute('role', 'group');
      box.setAttribute('aria-label', 'Confirm command');

      var meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = 'Confirm · ' + repo;
      var pre = document.createElement('pre');
      pre.textContent = command;

      var actions = document.createElement('div');
      actions.className = 'confirm-actions';
      var runBtn = document.createElement('button');
      runBtn.type = 'button';
      runBtn.className = 'btn sm primary';
      runBtn.textContent = 'Run';
      var cancelBtn = document.createElement('button');
      cancelBtn.type = 'button';
      cancelBtn.className = 'btn sm ghost';
      cancelBtn.textContent = 'Cancel';
      actions.appendChild(runBtn);
      actions.appendChild(cancelBtn);

      box.appendChild(meta);
      box.appendChild(pre);
      box.appendChild(actions);

      function dismiss() {
        document.removeEventListener('keydown', onKey, true);
        if (pendingDismiss === dismiss) pendingDismiss = null;
        box.remove();
        input.focus();
      }
      function onKey(ev) {
        if (ev.key === 'Escape') { ev.preventDefault(); dismiss(); }
      }
      runBtn.addEventListener('click', function () { dismiss(); run(); });
      cancelBtn.addEventListener('click', dismiss);
      document.addEventListener('keydown', onKey, true);
      pendingDismiss = dismiss;

      log.appendChild(box);
      log.scrollTop = log.scrollHeight;
      runBtn.focus();
    }

    function dispatch(command, repo) {
      sending = true;
      var sendBtn = form.querySelector('button[type="submit"]');
      if (sendBtn) sendBtn.disabled = true;
      // Keep the text so a failed request does not lose what the user typed.
      fetch('/admin/api/exec', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ repo: repo, command: command })
      })
        .then(function (r) { return r.json().then(function (j) { return { ok: r.ok, body: j }; }); })
        .then(function (res) {
          if (!res.ok) { showError(res.body.error || 'Execution failed'); return; }
          input.value = '';
          appendJob(command, res.body.job_id);
        })
        .catch(function () { showError('Network error, please try again'); })
        .finally(function () {
          sending = false;
          if (sendBtn) sendBtn.disabled = false;
          input.focus();
        });
    }

    form.addEventListener('submit', function (ev) {
      ev.preventDefault();
      if (sending) return;
      var command = (input.value || '').trim();
      if (!command) return;
      var repo = repoInput.value;
      if (isMutating(command)) {
        askConfirm(command, repo, function () { dispatch(command, repo); });
        return;
      }
      dispatch(command, repo);
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

    // Stop buttons rendered into the history (a job that was still running when
    // the page loaded). Live jobs wire their own button in appendJob.
    log.addEventListener('click', function (ev) {
      var btn = ev.target.closest ? ev.target.closest('button[data-job-id]') : null;
      if (!btn) return;
      var id = btn.getAttribute('data-job-id');
      btn.disabled = true;
      btn.textContent = 'Stopping…';
      fetch('/admin/api/jobs/' + id + '/cancel', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' }
      }).then(function (res) {
        if (res.ok) {
          // Rewrite only the label's text node: assigning textContent would
          // delete the button we are standing on.
          var meta = btn.parentNode;
          if (meta && meta.firstChild && meta.firstChild.nodeType === 3) {
            meta.firstChild.nodeValue = meta.firstChild.nodeValue.replace('running', 'cancelled');
          }
          btn.textContent = 'Cancelled';
          return;
        }
        // 409: it finished while the click was in flight — a refresh shows the
        // real outcome, so just put the button back.
        btn.disabled = false;
        btn.textContent = 'Stop';
      }).catch(function () {
        btn.disabled = false;
        btn.textContent = 'Stop';
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

// ---- scroll position preservation on form submit / filter / navigation ----
(function () {
  function saveScroll() {
    try {
      sessionStorage.setItem('sugie_scroll_' + location.pathname, String(window.scrollY || window.pageYOffset));
    } catch (e) {}
  }

  try {
    var saved = sessionStorage.getItem('sugie_scroll_' + location.pathname);
    if (saved !== null) {
      var y = parseInt(saved, 10);
      if (!isNaN(y) && y > 0) {
        window.scrollTo(0, y);
        requestAnimationFrame(function () { window.scrollTo(0, y); });
        setTimeout(function () {
          window.scrollTo(0, y);
          sessionStorage.removeItem('sugie_scroll_' + location.pathname);
        }, 80);
      } else {
        sessionStorage.removeItem('sugie_scroll_' + location.pathname);
      }
    }
  } catch (e) {}

  window.addEventListener('beforeunload', saveScroll);
  document.addEventListener('submit', saveScroll);

  document.querySelectorAll('input[type=date]').forEach(function (inp) {
    inp.addEventListener('change', saveScroll);
  });

  document.querySelectorAll('.card a, .page-head a, .tabs a').forEach(function (a) {
    a.addEventListener('click', saveScroll);
  });

  document.querySelectorAll('.topbar a').forEach(function (a) {
    a.addEventListener('click', function () {
      try { sessionStorage.removeItem('sugie_scroll_' + location.pathname); } catch (e) {}
    });
  });
})();

// ---- auto-submit for fixed-length codes (2FA) ----
// An 8-digit TOTP has no reason to wait for a click: the code is complete at
// exactly `data-autosubmit` digits. Typing and paste/authenticator autofill
// both end in an `input` event, so one handler covers both paths.
(function () {
  document.querySelectorAll('input[data-autosubmit]').forEach(function (input) {
    var form = input.form;
    var target = parseInt(input.getAttribute('data-autosubmit'), 10);
    if (!form || !target || target <= 0) return;

    var sent = false;

    function submitIfComplete() {
      if (sent) return;
      // Digits only: a pasted space or dash must not count toward the length.
      var digits = input.value.replace(/\D/g, '');
      if (digits !== input.value) input.value = digits;
      if (digits.length < target) return;
      input.value = digits.slice(0, target);

      sent = true;
      var btn = form.querySelector('button[type="submit"]');
      if (btn) {
        btn.disabled = true;
        btn.textContent = 'Verifying…';
      }
      // requestSubmit keeps constraint validation and the submit event intact;
      // form.submit() would bypass both.
      if (form.requestSubmit) form.requestSubmit(); else form.submit();
    }

    input.addEventListener('input', submitIfComplete);

    // `input` fires after paste in modern browsers, but a few autofill paths
    // only emit `paste`, so re-check on the next tick as well.
    input.addEventListener('paste', function () { setTimeout(submitIfComplete, 0); });

    // Track manual submission so a double click cannot queue two requests.
    form.addEventListener('submit', function () { sent = true; });
  });
})();
"#;

pub const LOGO_SVG: &str = include_str!("../../sugie-mark.svg");
