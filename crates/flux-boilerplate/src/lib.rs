//! flux-boilerplate — fluxc-owned UI app boilerplates for the qwen-build / design route.
//!
//! The design route used to generate every app cold (slow, inconsistent). Instead the AI now
//! **scaffolds from a boilerplate**: a known-good structural skeleton for the app KIND, which it
//! customizes for the user's prompt. Faster, consistent structure, real layout — "use boilerplates
//! in fluxc" (Viktor, 2026-06-03). Complements the backend money-molds in
//! `flux/templates/` (TEMPLATES_CATALOG.md); these are the frontend/UI layer.
//!
//! Each [`Kind`] maps to a compact, self-contained HTML skeleton with a shared dark/cyan shell and
//! the DISTINCTIVE structure of that app type, plus `<!-- AI: … -->` fill-points. `detect()` picks a
//! kind from a free-text prompt. Pure data + a tiny keyword classifier — std only (FLUXFOOD-trivial).

/// The app kinds we hold a boilerplate for (the qwen-build "big projects").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Game, Social, Streaming, Trading, Music, Ecommerce, Defi, Chat, Travel, Saas,
    // simulation boilerplates (the in-silico digital-twin lane), built on the void-walker droplet-field pattern
    Bioreactor, Cowherd,
}

impl Kind {
    pub fn slug(self) -> &'static str {
        use Kind::*;
        match self { Game=>"game", Social=>"social", Streaming=>"streaming", Trading=>"trading",
            Music=>"music", Ecommerce=>"ecommerce", Defi=>"defi", Chat=>"chat", Travel=>"travel", Saas=>"saas",
            Bioreactor=>"bioreactor", Cowherd=>"cowherd" }
    }
    pub fn label(self) -> &'static str {
        use Kind::*;
        match self { Game=>"3D browser game", Social=>"Social media app", Streaming=>"Streaming platform",
            Trading=>"Trading dashboard", Music=>"Music app", Ecommerce=>"E-commerce store",
            Defi=>"DeFi exchange", Chat=>"Chat app", Travel=>"Travel app", Saas=>"SaaS dashboard",
            Bioreactor=>"Bioreactor / lab-meat sim", Cowherd=>"Virtual cow-herd sim" }
    }
    pub fn from_slug(s: &str) -> Option<Kind> {
        ALL.iter().copied().find(|k| k.slug() == s)
    }
}

pub const ALL: &[Kind] = &[Kind::Game, Kind::Social, Kind::Streaming, Kind::Trading, Kind::Music,
    Kind::Ecommerce, Kind::Defi, Kind::Chat, Kind::Travel, Kind::Saas, Kind::Bioreactor, Kind::Cowherd];

/// Classify a free-text prompt to the best-matching boilerplate kind (None if nothing fits well).
/// Ordered keyword scan — first/strongest signal wins.
pub fn detect(prompt: &str) -> Option<Kind> {
    let p = prompt.to_lowercase();
    let has = |kws: &[&str]| kws.iter().any(|k| p.contains(k));
    use Kind::*;
    // most specific first — simulation lane before the generic UI kinds
    if has(&["bioreactor", "lab meat", "lab-grown", "cultured meat", "cellular agric", "cell growth", "dna", "biomass", "monod", "cell sim", "petri", "stem cell"]) { return Some(Bioreactor); }
    if has(&["cow", "herd", "cattle", "livestock", "virtual fence", "virtual fencing", "grazing", "graze", "pasture", "boids", "collar", "nofence", "flock"]) { return Some(Cowherd); }
    if has(&["3d", "webgl", "game", "shooter", "arena", "fps", "racing"]) { return Some(Game); }
    if has(&["dex", "swap", "defi", "liquidity", "amm", "yield"]) { return Some(Defi); }
    if has(&["trading", "candlestick", "order book", "orderbook", "portfolio", "watchlist", "ticker"]) { return Some(Trading); }
    if has(&["stream", "netflix", "video platform", "movies", "series", "watch"]) { return Some(Streaming); }
    if has(&["music", "spotify", "playlist", "song", "album", "audio player"]) { return Some(Music); }
    if has(&["social", "feed", "posts", "followers", "stories", "timeline"]) { return Some(Social); }
    if has(&["chat", "messaging", "conversation", "dm", "inbox"]) { return Some(Chat); }
    if has(&["shop", "store", "e-commerce", "ecommerce", "cart", "checkout", "product"]) { return Some(Ecommerce); }
    if has(&["travel", "booking", "map", "trip", "itinerary", "hotel", "flight"]) { return Some(Travel); }
    if has(&["saas", "analytics", "dashboard", "kpi", "metrics", "admin panel"]) { return Some(Saas); }
    None
}

/// The boilerplate skeleton for a kind — a self-contained HTML starter the AI customizes.
pub fn boilerplate(kind: Kind) -> &'static str {
    use Kind::*;
    match kind {
        Game => GAME, Social => SOCIAL, Streaming => STREAMING, Trading => TRADING, Music => MUSIC,
        Ecommerce => ECOMMERCE, Defi => DEFI, Chat => CHAT, Travel => TRAVEL, Saas => SAAS,
        Bioreactor => BIOREACTOR, Cowherd => COWHERD,
    }
}

/// Shared head — dark, cyan, Inter. Each skeleton starts from this then adds its structure.
const HEAD: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{{TITLE}}</title><style>
:root{--bg:#06080f;--panel:#0b0e16;--ink:#eaf0f7;--mut:#7d859a;--line:rgba(255,255,255,.08);--a:#22d3ee;--a2:#0e7490;--g:#34d399;--gold:#fbbf24;--font:'Inter',system-ui,sans-serif;--mono:ui-monospace,monospace}
*{box-sizing:border-box;margin:0}body{background:var(--bg);color:var(--ink);font-family:var(--font);-webkit-font-smoothing:antialiased}
.row{display:flex}.col{display:flex;flex-direction:column}.fill{flex:1}.mut{color:var(--mut)}
.panel{background:var(--panel);border:1px solid var(--line);border-radius:14px}
.btn{background:var(--a);color:#021016;border:0;border-radius:10px;padding:9px 16px;font-weight:700;cursor:pointer}
</style></head><body>"#;

// Each boilerplate: HEAD + the DISTINCTIVE shell for that app type. <!-- AI: … --> marks fill-points.
const GAME: &str = "<!-- KIND: 3D browser game. AI: keep the canvas + HUD + leaderboard structure; \
fill real WebGL (three.js via CDN or raw) gameplay, theme to the prompt. -->\n\
<canvas id=\"scene\" style=\"position:fixed;inset:0;width:100%;height:100%;display:block;background:#05070d\"></canvas>\
<div style=\"position:fixed;top:16px;left:16px\" class=\"panel col\"><b id=\"hp\">HP 100</b><b id=\"score\">SCORE 0</b><b id=\"ammo\">AMMO 30</b></div>\
<div style=\"position:fixed;top:16px;right:16px;min-width:160px\" class=\"panel\"><b style=\"color:var(--a)\">LEADERBOARD</b><ol id=\"lb\"></ol></div>\
<div style=\"position:fixed;bottom:16px;left:50%;transform:translateX(-50%)\" class=\"row\" id=\"weapons\"><!-- AI: weapon select --></div>\
<!-- AI: <script> three.js scene, pointer-lock controls, targets, scoring, live leaderboard --></body></html>";

const SOCIAL: &str = "<!-- KIND: social media app. AI: keep 3-column shell (nav | feed | sidebar); \
fill real posts/stories/profiles, theme to the prompt. -->\n\
<div class=\"row\" style=\"height:100vh\">\
<nav class=\"panel col\" style=\"width:220px;margin:10px;padding:14px;gap:8px\"><b style=\"color:var(--a)\">{{BRAND}}</b><!-- AI: nav links --></nav>\
<main class=\"fill col\" style=\"max-width:600px;margin:10px auto;gap:12px;overflow:auto\">\
<div class=\"row panel\" style=\"padding:10px;gap:8px;overflow:auto\" id=\"stories\"><!-- AI: stories bar --></div>\
<div class=\"panel\" style=\"padding:14px\"><textarea placeholder=\"What's happening?\" style=\"width:100%;background:transparent;border:0;color:var(--ink);resize:none\"></textarea><div class=\"row\"><span class=\"fill\"></span><button class=\"btn\">Post</button></div></div>\
<div id=\"feed\" class=\"col\" style=\"gap:12px\"><!-- AI: post cards: avatar, name, text/media, like/comment/share --></div></main>\
<aside class=\"panel\" style=\"width:280px;margin:10px;padding:14px\"><!-- AI: trends / who to follow --></aside></div></body></html>";

const STREAMING: &str = "<!-- KIND: video streaming app (Netflix-style). AI: keep hero + genre rows; \
fill real thumbnails/rows/player, theme to the prompt. -->\n\
<header class=\"row\" style=\"padding:16px 28px;position:fixed;top:0;left:0;right:0;background:linear-gradient(#000,transparent)\"><b style=\"color:var(--a);font-size:20px\">{{BRAND}}</b><span class=\"fill\"></span><!-- AI: nav --></header>\
<section style=\"height:62vh;display:flex;align-items:flex-end;padding:40px;background:linear-gradient(0deg,var(--bg),transparent 70%),#11131c\"><div><h1 style=\"font-size:44px\"><!-- AI: featured title --></h1><p class=\"mut\" style=\"max-width:480px\"><!-- AI: synopsis --></p><button class=\"btn\" style=\"margin-top:14px\">▶ Play</button></div></section>\
<div class=\"col\" style=\"padding:20px 28px;gap:26px\" id=\"rows\"><!-- AI: genre rows, each: <h3> + horizontal scroll of thumbnail cards --></div></body></html>";

const TRADING: &str = "<!-- KIND: crypto trading dashboard. AI: keep chart | orderbook | trade-panel grid; \
fill real candlestick (canvas/lightweight-charts CDN), depth, buy/sell, theme to prompt. -->\n\
<div style=\"display:grid;grid-template-columns:1fr 280px 300px;grid-gap:10px;height:100vh;padding:10px\">\
<div class=\"panel col\"><div class=\"row\" style=\"padding:10px\"><b><!-- AI: pair --> BTC/USDS</b><span class=\"fill\"></span><b style=\"color:var(--g)\" id=\"price\">$—</b></div><canvas id=\"chart\" class=\"fill\"></canvas></div>\
<div class=\"panel col\" style=\"padding:10px\"><b style=\"color:var(--a)\">ORDER BOOK</b><div id=\"asks\" class=\"col\"></div><b id=\"mid\"></b><div id=\"bids\" class=\"col\"></div></div>\
<div class=\"panel col\" style=\"padding:12px;gap:8px\"><div class=\"row\"><button class=\"btn fill\" style=\"background:var(--g)\">Buy</button><button class=\"btn fill\" style=\"background:#f43f5e;margin-left:8px\">Sell</button></div><!-- AI: amount/price inputs, portfolio, watchlist --></div></div>\
<!-- AI: <script> render candles + live order book --></body></html>";

const MUSIC: &str = "<!-- KIND: music streaming app (Spotify-style). AI: keep sidebar | grid | now-playing bar; \
fill real playlists/albums/player + waveform, theme to prompt. -->\n\
<div style=\"display:grid;grid-template-rows:1fr 84px;height:100vh\"><div class=\"row\">\
<nav class=\"panel col\" style=\"width:230px;margin:8px;padding:14px;gap:6px\"><b style=\"color:var(--a)\">{{BRAND}}</b><!-- AI: library / playlists --></nav>\
<main class=\"fill\" style=\"padding:18px;overflow:auto\"><h2><!-- AI: heading --></h2><div style=\"display:grid;grid-template-columns:repeat(auto-fill,minmax(160px,1fr));gap:14px\" id=\"albums\"><!-- AI: album cards --></div></main></div>\
<div class=\"panel row\" style=\"align-items:center;padding:0 16px;gap:14px;border-radius:0\"><div id=\"track\"><!-- AI: now playing --></div><div class=\"fill col\" style=\"align-items:center\"><div><!-- AI: ⏮ ⏯ ⏭ --></div><div style=\"height:4px;background:#1e2633;border-radius:2px;width:60%\"><div style=\"height:100%;width:35%;background:var(--a);border-radius:2px\"></div></div></div></div></div></body></html>";

const ECOMMERCE: &str = "<!-- KIND: e-commerce storefront. AI: keep header + hero + filters + product grid + cart drawer; \
fill real products, theme to prompt. -->\n\
<header class=\"row\" style=\"padding:14px 24px;align-items:center;gap:14px\" ><b style=\"color:var(--a);font-size:19px\">{{BRAND}}</b><input placeholder=\"Search\" class=\"fill panel\" style=\"padding:9px 12px;color:var(--ink)\"><button class=\"btn\">Cart (0)</button></header>\
<section style=\"padding:36px 24px;background:#0b0e16\"><h1 style=\"font-size:36px\"><!-- AI: hero headline --></h1></section>\
<div class=\"row\" style=\"padding:18px 24px;gap:18px\"><aside style=\"width:200px\" class=\"col\"><b>Filters</b><!-- AI: category / price filters --></aside>\
<main class=\"fill\" style=\"display:grid;grid-template-columns:repeat(auto-fill,minmax(200px,1fr));gap:16px\" id=\"products\"><!-- AI: product cards: image, title, price, Add to cart --></main></div></body></html>";

const DEFI: &str = "<!-- KIND: DeFi DEX app. AI: keep swap card + pools + chart + APR cards; fill real swap UI, theme to prompt. -->\n\
<header class=\"row\" style=\"padding:14px 26px;align-items:center\"><b style=\"color:var(--a);font-size:19px\">{{BRAND}}</b><span class=\"fill\"></span><button class=\"btn\">Connect Wallet</button></header>\
<div class=\"row\" style=\"padding:24px;gap:18px;flex-wrap:wrap\">\
<div class=\"panel col\" style=\"width:380px;padding:18px;gap:10px\"><b>Swap</b><div class=\"panel row\" style=\"padding:12px\"><input value=\"1.0\" class=\"fill\" style=\"background:transparent;border:0;color:var(--ink);font-size:20px\"><b><!-- AI: token A --></b></div><div style=\"text-align:center;color:var(--a)\">↓</div><div class=\"panel row\" style=\"padding:12px\"><input class=\"fill\" style=\"background:transparent;border:0;color:var(--ink);font-size:20px\" placeholder=\"0.0\"><b><!-- AI: token B --></b></div><button class=\"btn\">Swap</button></div>\
<div class=\"fill col\" style=\"gap:14px\"><div class=\"panel\" style=\"height:240px;padding:12px\"><b>Price</b><canvas id=\"chart\"></canvas></div><div style=\"display:grid;grid-template-columns:repeat(auto-fill,minmax(180px,1fr));gap:12px\" id=\"pools\"><!-- AI: pool / APR cards --></div></div></div></body></html>";

const CHAT: &str = "<!-- KIND: real-time chat app. AI: keep conversation list | thread | composer; fill messages, theme to prompt. -->\n\
<div class=\"row\" style=\"height:100vh\">\
<aside class=\"panel col\" style=\"width:280px;margin:8px;overflow:auto\"><div style=\"padding:12px\"><b style=\"color:var(--a)\">{{BRAND}}</b></div><div id=\"convos\"><!-- AI: conversation rows: avatar, name, last msg, time --></div></aside>\
<main class=\"fill col\" style=\"margin:8px\"><header class=\"panel\" style=\"padding:12px\"><b><!-- AI: active contact --></b></header>\
<div class=\"fill col\" style=\"gap:8px;padding:14px;overflow:auto\" id=\"thread\"><!-- AI: message bubbles (mine=right cyan, theirs=left panel) + typing indicator --></div>\
<div class=\"panel row\" style=\"padding:10px;gap:8px\"><input class=\"fill\" placeholder=\"Message…\" style=\"background:transparent;border:0;color:var(--ink)\"><button class=\"btn\">Send</button></div></main></div></body></html>";

const TRAVEL: &str = "<!-- KIND: travel booking app. AI: keep map + search + destination cards + itinerary; fill real content, theme to prompt. -->\n\
<header class=\"row\" style=\"padding:14px 24px;align-items:center;gap:12px\"><b style=\"color:var(--a);font-size:19px\">{{BRAND}}</b><input placeholder=\"Where to?\" class=\"panel\" style=\"padding:9px 12px;color:var(--ink)\"><!-- AI: dates / guests pickers --><button class=\"btn\">Search</button></header>\
<div class=\"row\" style=\"height:calc(100vh - 60px)\">\
<main class=\"fill\" style=\"padding:18px;overflow:auto\"><h2><!-- AI: heading --></h2><div style=\"display:grid;grid-template-columns:repeat(auto-fill,minmax(220px,1fr));gap:16px\" id=\"dests\"><!-- AI: destination cards: photo, place, price/night, rating --></div></main>\
<div id=\"map\" class=\"panel\" style=\"width:42%;margin:8px;background:#0e1420;display:grid;place-items:center\" ><!-- AI: interactive map (leaflet CDN) with pins --></div></div></body></html>";

const SAAS: &str = "<!-- KIND: SaaS analytics dashboard. AI: keep sidebar + KPI cards + charts + table; fill real metrics, theme to prompt. -->\n\
<div class=\"row\" style=\"height:100vh\">\
<nav class=\"panel col\" style=\"width:220px;margin:8px;padding:14px;gap:8px\"><b style=\"color:var(--a)\">{{BRAND}}</b><!-- AI: nav links --></nav>\
<main class=\"fill\" style=\"padding:18px;overflow:auto\"><h2><!-- AI: dashboard title --></h2>\
<div style=\"display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:14px;margin:14px 0\" id=\"kpis\"><!-- AI: KPI cards: label, big number, delta --></div>\
<div class=\"row\" style=\"gap:14px;flex-wrap:wrap\"><div class=\"panel fill\" style=\"height:260px;padding:14px;min-width:300px\"><b>Trend</b><canvas id=\"line\"></canvas></div><div class=\"panel\" style=\"height:260px;padding:14px;width:300px\"><b>Breakdown</b><canvas id=\"bar\"></canvas></div></div>\
<div class=\"panel\" style=\"margin-top:14px;padding:14px\"><b>Recent</b><table style=\"width:100%\" id=\"tbl\"><!-- AI: data table --></table></div></main></div></body></html>";

// ── simulation boilerplates (real running browser sims; the molecular/cell backend is the
// q-narwhalknight water-robots stack: q-bio-dsl + mitochondria-sim + void-walker, driven by MCP combos) ──
const COWHERD: &str = r#"<!-- KIND: virtual cow-herd sim (Agent-Based Model): Boids flocking + a GPS virtual fence + the collar state machine (calm -> sound -> haptic). REAL headless CPU backend exists: the `flux-cowsim` crate (deterministic ABM, Pavlovian learning, welfare_score = containment - shock_weight*shock_rate; binary `flux-cowsim --steps N`). The browser canvas below is the viz; for the authoritative run/optimize use flux-cowsim. AI: keep the canvas + loop + controls; theme + extend (RL learn-curve, paddock rotation) and wire to a /capi/cowsim endpoint that shells the flux-cowsim binary. Pattern from q-narwhalknight void-walker agent-field + q-robot-cli collar. -->
<header style="display:flex;align-items:center;gap:12px;padding:14px 22px"><b style="color:var(--g);font-size:19px">🐄 {{BRAND}}</b><span class="mut" style="font-size:12px">Agent-Based Model · void-walker agent-field · q-robot-cli collar</span></header>
<div class="row" style="gap:14px;padding:0 16px 16px;flex-wrap:wrap">
  <canvas id="field" width="860" height="540" class="panel" style="background:#0a1410"></canvas>
  <aside class="panel col" style="flex:1;min-width:240px;padding:16px;gap:12px">
    <b>Herd</b>
    <label class="mut">cows <input id="n" type="range" min="10" max="200" value="60"></label>
    <label class="mut">fence radius <input id="r" type="range" min="80" max="260" value="200"></label>
    <label class="mut">sound zone <input id="warn" type="range" min="10" max="90" value="45"></label>
    <div id="stat" style="font-family:var(--mono);font-size:13px;line-height:1.8"></div>
    <div class="mut" style="font-size:11px;border-top:1px solid var(--line);padding-top:10px"><b style="color:var(--a)">AI MCP combos</b><br>flux_combo (≈50ms physics recompile) · flux_batch_compile (1000s of sound/haptic variants across cores) · flux_swarm_* (agent-swarm runs N herds in parallel) · flux_chronos_run (deterministic) · flux_iterate (AI tunes for fastest stress-free learning)</div>
  </aside>
</div>
<script>
const cv=document.getElementById('field'),x=cv.getContext('2d'),cx=cv.width/2,cy=cv.height/2;let cows=[];
function reset(n){cows=[];for(let i=0;i<n;i++)cows.push({x:Math.random()*cv.width,y:Math.random()*cv.height,vx:0,vy:0,learn:0,st:0})}
function tick(){const R=+document.getElementById('r').value,W=+document.getElementById('warn').value;
 x.fillStyle='#0a1410';x.fillRect(0,0,cv.width,cv.height);
 x.strokeStyle='#f43f5e';x.setLineDash([6,6]);x.beginPath();x.arc(cx,cy,R,0,7);x.stroke();x.setLineDash([]);
 let inside=0,learned=0;
 for(const c of cows){let dx=cx-c.x,dy=cy-c.y,d=Math.hypot(dx,dy)||1;c.vx+=dx/d*0.02;c.vy+=dy/d*0.02;
  for(const o of cows){if(o===c)continue;const ex=c.x-o.x,ey=c.y-o.y,ed=Math.hypot(ex,ey);if(ed<16&&ed>0){c.vx+=ex/ed*0.15;c.vy+=ey/ed*0.15;}}
  c.vx+=(Math.random()-0.5)*0.1;c.vy+=(Math.random()-0.5)*0.1;const fromFence=R-d;
  if(fromFence<0){c.st=3;c.vx-=dx/d*0.7;c.vy-=dy/d*0.7;} else if(fromFence<W){c.st=2;c.vx-=dx/d*0.22;c.vy-=dy/d*0.22;c.learn=Math.min(1,c.learn+0.001);} else c.st=0;
  if(d<R)inside++; if(c.learn>0.5)learned++;
  const s=Math.hypot(c.vx,c.vy);if(s>2){c.vx*=2/s;c.vy*=2/s;}c.x+=c.vx;c.y+=c.vy;
  x.fillStyle=c.st===3?'#f43f5e':c.st===2?'#fbbf24':'#34d399';x.beginPath();x.arc(c.x,c.y,4,0,7);x.fill();}
 document.getElementById('stat').innerHTML='inside fence: <b style="color:#34d399">'+Math.round(inside/cows.length*100)+'%</b><br>learned the fence: <b style="color:#22d3ee">'+Math.round(learned/cows.length*100)+'%</b><br>herd: '+cows.length;
 requestAnimationFrame(tick);}
document.getElementById('n').oninput=e=>reset(+e.target.value);reset(60);tick();
</script></body></html>"#;

const BIOREACTOR: &str = r#"<!-- KIND: bioreactor / lab-meat sim. Cell-population Monod kinetics (mu = mu_max*S/(Ks+S)) + a nutrient/substrate field (the void-walker EWOD droplet-field pattern) + a DNA strand assembly nod to q-bio-dsl (place_atom/form_bond, nucleotide-by-nucleotide). AI: keep the tank sim + biomass curve + controls; the REAL molecular + cell-division backend is q-bio-dsl + mitochondria-sim, driven by the MCP combos below. -->
<header style="display:flex;align-items:center;gap:12px;padding:14px 22px"><b style="color:var(--a);font-size:19px">🧫 {{BRAND}}</b><span class="mut" style="font-size:12px">in-silico digital twin · void-walker droplet-field · mitochondria-sim · q-bio-dsl</span></header>
<div class="row" style="gap:14px;padding:0 16px 16px;flex-wrap:wrap">
  <canvas id="tank" width="500" height="500" class="panel" style="background:#070b14"></canvas>
  <aside class="panel col" style="flex:1;min-width:280px;padding:16px;gap:12px">
    <b>Feed (the EWOD pump)</b>
    <label class="mut">glucose <input id="glu" type="range" min="0" max="100" value="70"></label>
    <label class="mut">O₂ <input id="o2" type="range" min="0" max="100" value="60"></label>
    <div id="stat" style="font-family:var(--mono);font-size:13px;line-height:1.7"></div>
    <b style="margin-top:6px">Biomass</b><canvas id="curve" width="300" height="84" style="background:#0a0f1c;border-radius:8px"></canvas>
    <b style="margin-top:6px">DNA assembly <span class="mut" style="font-size:11px">(q-bio-dsl)</span></b>
    <div id="dna" style="font-family:var(--mono);font-size:14px;letter-spacing:3px;color:#34d399;word-break:break-all"></div>
    <div class="mut" style="font-size:11px;border-top:1px solid var(--line);padding-top:10px"><b style="color:var(--a)">AI MCP combos</b><br>flux_combo (≈50ms recompile of the physics core) · flux_batch_compile (1000s of recipe variants across cores) · flux_vast_recommend/fleet (GPU compute market — millions of cells in 3D) · flux_chronos_run (deterministic) · flux_iterate (AI finds the optimal glucose/O₂ recipe)</div>
  </aside>
</div>
<script>
const cv=document.getElementById('tank'),x=cv.getContext('2d'),cur=document.getElementById('curve'),c2=cur.getContext('2d');
let cells=[],hist=[],S=1.0,t=0;const Ks=0.3,muMax=0.045,B='ACGT';let dna='';
function reset(){cells=[];for(let i=0;i<20;i++)cells.push({x:Math.random()*cv.width,y:Math.random()*cv.height,e:0});hist=[];S=1.0;t=0;dna='';}
function tick(){const glu=+document.getElementById('glu').value/100,o2=+document.getElementById('o2').value/100;
 S=Math.min(1,S+glu*0.012);const mu=muMax*S/(Ks+S)*o2;
 x.fillStyle='#070b14';x.fillRect(0,0,cv.width,cv.height);const next=[];
 for(const c of cells){c.x+=(Math.random()-0.5)*1.4;c.y+=(Math.random()-0.5)*1.4;c.x=Math.max(4,Math.min(cv.width-4,c.x));c.y=Math.max(4,Math.min(cv.height-4,c.y));c.e+=mu;
  if(c.e>1&&cells.length+next.length<4000&&S>0.05){c.e=0;S-=0.0004;next.push({x:c.x+4,y:c.y+4,e:0});}
  x.fillStyle='rgba(52,211,153,'+(0.45+0.55*o2)+')';x.beginPath();x.arc(c.x,c.y,2.4,0,7);x.fill();}
 cells=cells.concat(next);t++;if(t%4===0){hist.push(cells.length);if(hist.length>150)hist.shift();}
 c2.fillStyle='#0a0f1c';c2.fillRect(0,0,cur.width,cur.height);c2.strokeStyle='#22d3ee';c2.beginPath();const mx=Math.max(...hist,1);
 hist.forEach((v,i)=>{const px=i/150*cur.width,py=cur.height-v/mx*cur.height;i?c2.lineTo(px,py):c2.moveTo(px,py);});c2.stroke();
 if(t%6===0&&dna.length<140){dna+=B[Math.floor(Math.random()*4)];document.getElementById('dna').textContent=dna;}
 document.getElementById('stat').innerHTML='cells: <b style="color:#34d399">'+cells.length+'</b><br>substrate S: <b style="color:#fbbf24">'+S.toFixed(2)+'</b><br>growth μ: <b style="color:#22d3ee">'+mu.toFixed(4)+'</b>/step';
 requestAnimationFrame(tick);}
reset();tick();
</script></body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_picks_the_right_kind() {
        assert_eq!(detect("design a 3D browser game with WebGL"), Some(Kind::Game));
        assert_eq!(detect("a social media app with a feed and stories"), Some(Kind::Social));
        assert_eq!(detect("netflix-style video streaming platform"), Some(Kind::Streaming));
        assert_eq!(detect("a crypto trading dashboard with candlestick + order book"), Some(Kind::Trading));
        assert_eq!(detect("DEX swap interface with liquidity pools"), Some(Kind::Defi));
        assert_eq!(detect("spotify-style music app with playlists"), Some(Kind::Music));
        assert_eq!(detect("real-time chat app messaging"), Some(Kind::Chat));
        assert_eq!(detect("ecommerce store with cart and checkout"), Some(Kind::Ecommerce));
        assert_eq!(detect("travel booking app with a map"), Some(Kind::Travel));
        assert_eq!(detect("a SaaS analytics dashboard with KPI cards"), Some(Kind::Saas));
        assert_eq!(detect("simulate a bioreactor growing lab meat with cell division and DNA"), Some(Kind::Bioreactor));
        assert_eq!(detect("a virtual cow herd sim with a virtual fence and grazing"), Some(Kind::Cowherd));
        assert_eq!(detect("write me a haiku about the sea"), None);
    }

    #[test]
    fn every_kind_has_a_nonempty_boilerplate_with_fill_points() {
        for &k in ALL {
            let b = boilerplate(k);
            assert!(b.len() > 200, "{} boilerplate too small", k.slug());
            assert!(b.contains("AI:"), "{} should mark AI fill-points", k.slug());
            assert!(Kind::from_slug(k.slug()) == Some(k), "slug roundtrips");
        }
    }
}
