/* Sai Recorder — frontend wiring (authored by Sai). Talks to Tauri commands. */
(function(){
  "use strict";
  var T = window.__TAURI__ || null;
  var invoke = T && T.core ? T.core.invoke : null;
  var listenFn = T && T.event ? T.event.listen : null;
  var getCurWin = T && T.window ? T.window.getCurrentWindow : null;
  var page = (location.pathname.split("/").pop() || "index.html").toLowerCase();

  function $(s, r){ return (r||document).querySelector(s); }
  function $all(s, r){ return Array.prototype.slice.call((r||document).querySelectorAll(s)); }
  function txt(e){ return (e && e.textContent ? e.textContent : "").trim(); }
  function byText(tag, t, r){ t=t.toLowerCase(); return $all(tag, r).find(function(e){ return txt(e).toLowerCase().indexOf(t)>=0; }); }
  function go(u){ location.href = u; }
  function qs(n){ return new URLSearchParams(location.search).get(n); }
  function esc(s){ return encodeURIComponent(s); }
  async function inv(cmd, args){ if(!invoke){ console.warn("[no-tauri]", cmd); return null; } return invoke(cmd, args); }
  function listen(ev, cb){ if(listenFn){ try{ listenFn(ev, cb); }catch(e){ console.warn(e);} } }

  function toast(msg, type){
    var c = document.getElementById("__toast");
    if(!c){ c=document.createElement("div"); c.id="__toast"; c.style.cssText="position:fixed;bottom:20px;left:50%;transform:translateX(-50%);z-index:99999;display:flex;flex-direction:column;gap:8px;align-items:center;pointer-events:none"; document.body.appendChild(c); }
    var t=document.createElement("div");
    t.textContent=String(msg);
    t.style.cssText="background:"+(type==="error"?"#5b1a1a":(type==="ok"?"#173a26":"#1d2027"))+";color:#e1e2ec;padding:10px 16px;border-radius:10px;font:14px Inter,system-ui,sans-serif;box-shadow:0 8px 30px rgba(0,0,0,.55);border:1px solid rgba(255,255,255,.08);max-width:80vw";
    c.appendChild(t); setTimeout(function(){ t.remove(); }, 4200);
  }
  function fmtClock(ms){ var s=Math.floor((Number(ms)||0)/1000); var h=Math.floor(s/3600); var m=Math.floor((s%3600)/60); var ss=s%60; function p(n){ return (n<10?"0":"")+n; } return (h>0? h+":" : "")+p(m)+":"+p(ss); }
  function fmtDur(ms){ var s=Math.floor((Number(ms)||0)/1000); var m=Math.floor(s/60); return m+"m "+(s%60)+"s"; }
  function fmtDate(v){ try{ var d=new Date(v); return isNaN(d.getTime())? String(v) : d.toLocaleString(); }catch(e){ return String(v); } }
  function fmtBytes(n){ n=Number(n)||0; if(n<1024) return n+" B"; if(n<1048576) return (n/1024).toFixed(1)+" KB"; if(n<1073741824) return (n/1048576).toFixed(1)+" MB"; return (n/1073741824).toFixed(2)+" GB"; }
  function fmtCount(n){ n=Number(n)||0; if(n<1000) return String(Math.round(n)); if(n<1e6) return (n/1e3).toFixed(n<1e4?1:0).replace(/\.0$/,"")+"K"; return (n/1e6).toFixed(n<1e7?2:1).replace(/\.0$/,"")+"M"; }
  function fmtHm(ms){ var s=Math.floor((Number(ms)||0)/1000); var h=Math.floor(s/3600); var m=Math.floor((s%3600)/60); if(h>0) return h+"h "+m+"m"; var ss=s%60; return m+"m "+ss+"s"; }
  function comp(t){ var c=(t&&t.compression)||{}; return (typeof c==="object"&&c)?c:{}; }
  function findTimerEl(){ return $all("span,div,p").find(function(e){ return e.children.length===0 && /^\s*\d{1,2}:\d{2}(:\d{2})?\s*$/.test(e.textContent||""); }); }
  function startTimer(){
    var start=Number(localStorage.getItem("recStart"));
    if(!start){ start=Date.now(); localStorage.setItem("recStart", String(start)); }
    var el=findTimerEl();
    function tick(){ if(el) el.textContent=fmtClock(Date.now()-start); }
    tick(); setInterval(tick, 500);
  }
  function symBtn(sym){ return $all("button").find(function(b){ var s=b.querySelector(".material-symbols-outlined"); return s && txt(s)===sym; }); }

  function wireNav(){
    var map=[["home","dashboard.html"],["task library","library.html"],["replay","replay.html"],["export","export.html"],["insight","compression.html"],["settings","settings.html"]];
    $all("nav a, aside a, a").forEach(function(a){
      var t=txt(a).toLowerCase();
      for(var i=0;i<map.length;i++){ if(t.indexOf(map[i][0])>=0){ a.addEventListener("click", function(e){ e.preventDefault(); }); a.addEventListener("click", (function(h){ return function(){ go(h); }; })(map[i][1])); if(page===map[i][1]) a.classList.add("active"); break; } }
    });
  }
  function wireStartButtons(){
    $all("button").forEach(function(b){
      var t=txt(b).toLowerCase();
      if(/start recording|start your first recording/.test(t)){
        if(page==="setup.html" && /start recording/.test(t)) return;
        b.addEventListener("click", function(){ go("setup.html"); });
      }
    });
  }
  function readOpts(){
    var s={}; try{ s=JSON.parse(localStorage.getItem("recordOpts")||"{}"); }catch(e){}
    return { fps: Number(s.fps)||10, history_secs: Number(s.history_secs)||5, crop: !!s.crop, lossy: s.lossy!==false, quality: Number(s.quality)||60, max_dim: s.max_dim==null?384:Number(s.max_dim) };
  }

  /* ---------------- pages ---------------- */
  function pgOnboarding(){
    if(localStorage.getItem("onboarded")==="1"){ location.replace("dashboard.html"); return; }
    var cards=$all(".option-card");
    cards.forEach(function(c){ c.addEventListener("click", function(){
      cards.forEach(function(x){ x.classList.remove("selected"); });
      c.classList.add("selected");
      var t=txt(c).toLowerCase();
      localStorage.setItem("mode", (/dataset|demonstration|collect/.test(t))?"dataset":"sai");
    }); });
    var cont=document.getElementById("continue-btn")||byText("button","continue");
    if(cont) cont.addEventListener("click", function(){ localStorage.setItem("onboarded","1"); go("dashboard.html"); });
  }

  async function pgDashboard(){
    wireNav(); wireStartButtons();
    var tasks=[];
    try{ tasks=await inv("list_tasks")||[]; }catch(e){ console.warn(e); return; }

    // ---- summary stats ----
    var totalMs=0, baseTok=0, compTok=0, ratioSum=0, ratioN=0;
    tasks.forEach(function(t){
      totalMs+=Number(t.duration_ms)||0;
      var c=comp(t);
      baseTok+=Number(c.baselineTokensEst)||0;
      compTok+=Number(c.compressedTokensEst)||0;
      var b=Number(c.baselineTokensEst)||0;
      if(b>0){ ratioSum+=(1-(Number(c.compressedTokensEst)||0)/b); ratioN++; }
    });
    var elTotal=document.getElementById("stat-total"); if(elTotal) elTotal.textContent=String(tasks.length);
    var elTime=document.getElementById("stat-time"); if(elTime) elTime.textContent=fmtHm(totalMs);
    var elTok=document.getElementById("stat-tokens"); if(elTok) elTok.textContent=fmtCount(Math.max(0, baseTok-compTok));
    var elComp=document.getElementById("stat-comp"); if(elComp) elComp.textContent=ratioN? Math.round(100*ratioSum/ratioN)+"%" : "—";

    // ---- recent activity (most recent first) ----
    var tbody=document.getElementById("recent-tbody");
    if(!tbody) return;
    var recent=tasks.slice().sort(function(a,b){ return (new Date(b.created)-new Date(a.created))||0; }).slice(0,5);
    tbody.innerHTML="";
    if(!recent.length){
      var tr=document.createElement("tr");
      tr.innerHTML='<td class="py-8 px-4 text-center text-on-surface-variant" colspan="6">No recordings yet. Start one above.</td>';
      tbody.appendChild(tr); return;
    }
    recent.forEach(function(t){
      var c=comp(t);
      var tr=document.createElement("tr");
      tr.className="table-row-hover transition-colors group cursor-pointer";
      tr.innerHTML=
        '<td class="py-3 px-4"><div class="w-16 h-10 rounded bg-surface-container-highest border border-outline-variant/20 flex items-center justify-center text-on-surface-variant">'
          +'<span class="material-symbols-outlined text-[18px]">'+(t.mode==="dataset"?"dataset":"smart_toy")+'</span></div></td>'
        +'<td class="py-3 px-4 font-medium text-on-surface"></td>'
        +'<td class="py-3 px-4 text-on-surface-variant font-mono-label">'+fmtDur(t.duration_ms)+'</td>'
        +'<td class="py-3 px-4 text-on-surface-variant">'+(Number(t.action_count)||0)+' steps</td>'
        +'<td class="py-3 px-4 text-on-surface-variant">'+fmtBytes(c.compressedBytes)+'</td>'
        +'<td class="py-3 px-4 text-right"><button class="w-8 h-8 rounded-full bg-surface-container flex items-center justify-center text-primary opacity-0 group-hover:opacity-100 transition-all hover:bg-primary/20 hover:scale-110 ml-auto" title="Replay"><span class="material-symbols-outlined text-[20px]" style="font-variation-settings: \'FILL\' 1;">play_arrow</span></button></td>';
      tr.children[1].textContent=t.name||t.id;
      tr.addEventListener("click", function(e){ if(e.target.closest("button")) return; go("task-detail.html?id="+esc(t.id)); });
      var play=tr.querySelector("button");
      if(play) play.addEventListener("click", function(e){ e.stopPropagation(); go("replay.html?id="+esc(t.id)); });
      tbody.appendChild(tr);
    });
    var viewAll=byText("button","view all"); if(viewAll) viewAll.addEventListener("click", function(){ go("library.html"); });
  }

  async function pgCompression(){
    wireNav();
    var tasks=[];
    try{ tasks=await inv("list_tasks")||[]; }catch(e){ console.warn(e); return; }
    var withC=tasks.filter(function(t){ return (Number(comp(t).baselineTokensEst)||0)>0; });

    // ---- aggregate hero ratio ----
    var baseTok=0, compTok=0;
    withC.forEach(function(t){ var c=comp(t); baseTok+=Number(c.baselineTokensEst)||0; compTok+=Number(c.compressedTokensEst)||0; });
    var ratio=(compTok>0)? baseTok/compTok : 0;
    var elR=document.getElementById("comp-ratio");
    if(elR) elR.textContent= ratio>0 ? (ratio>=10?Math.round(ratio):ratio.toFixed(1))+"x" : "—";

    // ---- proportional bars (Sai relative to standard baseline) ----
    var sai=document.getElementById("comp-bar-sai");
    if(sai){ var pct=ratio>0? Math.max(2, Math.min(100, 100/ratio)) : 100; sai.style.height=pct.toFixed(1)+"%"; }

    // ---- per-recording table ----
    var title=document.getElementById("comp-table-title");
    if(title) title.textContent="Token Accumulation ("+withC.length+" recording"+(withC.length===1?"":"s")+")";
    var tbody=document.getElementById("comp-tbody");
    if(!tbody) return;
    tbody.innerHTML="";
    if(!withC.length){
      var tr=document.createElement("tr");
      tr.innerHTML='<td class="py-8 text-center text-on-surface-variant" colspan="4">No compression data yet — record a task to populate.</td>';
      tbody.appendChild(tr); return;
    }
    withC.slice().sort(function(a,b){ return (new Date(b.created)-new Date(a.created))||0; }).slice(0,12).forEach(function(t){
      var c=comp(t);
      var b=Number(c.baselineTokensEst)||0, cm=Number(c.compressedTokensEst)||0;
      var saved=b>0? Math.round(100*(1-cm/b)) : 0;
      var tr=document.createElement("tr");
      tr.className="border-b border-outline-variant/20 hover:bg-surface-variant/30 transition-colors";
      tr.innerHTML=
        '<td class="py-3 text-on-surface-variant"></td>'
        +'<td class="py-3 text-error-container">'+fmtCount(b)+' tok ('+fmtBytes(c.baselineBytes)+')</td>'
        +'<td class="py-3 text-primary">'+(Number(c.shots)||0)+' frames · '+fmtCount(cm)+' tok</td>'
        +'<td class="py-3 text-right text-surface-tint font-bold">~ '+saved+'%</td>';
      tr.children[0].textContent=t.name||t.id;
      tbody.appendChild(tr);
    });
  }

  function pgSetup(){
    wireNav();
    var startBtn=byText("button","start recording")||symBtn("fiber_manual_record")||symBtn("radio_button_checked")||$all("button").pop();
    if(!startBtn) return;
    startBtn.addEventListener("click", async function(){
      var mode=localStorage.getItem("mode")||"sai";
      var r=$all('input[name="recording_mode"]').find(function(x){ return x.checked; });
      if(r) mode=/dataset/i.test(r.value)?"dataset":"sai";
      var o=readOpts();
      var opts={ mode:mode, fps:o.fps, history_secs:o.history_secs, crop:o.crop, lossy:o.lossy, quality:o.quality, max_dim:o.max_dim };
      try{
        startBtn.disabled=true;
        await inv("start_recording", { opts: opts });
        localStorage.setItem("recStart", String(Date.now()));
        try{ await inv("open_float_window"); }catch(e){ console.warn("float", e); }
        go("recording.html");
      }catch(e){ toast("Couldn't start recording: "+e, "error"); startBtn.disabled=false; }
    });
  }

  function pgFloat(){
    // drag handled by data-tauri-drag-region; fallback for handle:
    var grip=symBtn("drag_indicator")||$(".cursor-grab");
    if(grip && getCurWin){ grip.addEventListener("mousedown", function(){ try{ getCurWin().startDragging(); }catch(e){} }); }
    var stop=symBtn("stop")||byText("button","stop");
    if(stop) stop.addEventListener("click", async function(){ try{ await inv("stop_recording"); }catch(e){ console.error(e); } });
    startTimer();
  }

  function pgRecording(){
    wireNav();
    var stop=byText("button","stop")||symBtn("stop");
    if(stop) stop.addEventListener("click", async function(){ try{ await inv("stop_recording"); }catch(e){ toast("Stop failed: "+e, "error"); } });
    startTimer();
    listen("recording-finished", function(e){ var id=e&&e.payload?e.payload.id:null; localStorage.removeItem("recStart"); go("task-detail.html"+(id?("?id="+esc(id)):"")); });
  }

  async function pgLibrary(){
    wireNav();
    var grid=$(".grid")||$("main .grid");
    var tpl=$(".recording-card");
    var tasks=[];
    try{ tasks=await inv("list_tasks")||[]; }catch(e){ toast("Failed to load tasks: "+e, "error"); }
    if(!grid||!tpl) return;
    var proto=tpl.cloneNode(true);
    $all(".recording-card", grid).forEach(function(c){ c.remove(); });
    if(!tasks.length){ var empty=document.createElement("div"); empty.className="col-span-full text-center text-on-surface-variant py-16"; empty.textContent="No recordings yet. Start one from the dashboard."; grid.appendChild(empty); return; }
    tasks.forEach(function(t){
      var card=proto.cloneNode(true);
      var nameEl=card.querySelector('[class*="font-title"],[class*="title"],h1,h2,h3,h4,[class*="headline"],[class*="font-label-lg"],[class*="font-body-lg"]');
      if(nameEl) nameEl.textContent=t.name||t.id;
      card.title=(t.name||t.id)+"\n"+(t.action_count!=null?t.action_count+" actions • ":"")+fmtDur(t.duration_ms)+" • "+fmtDate(t.created);
      card.style.cursor="pointer";
      card.addEventListener("click", function(e){ if(e.target.closest("button")) return; go("task-detail.html?id="+esc(t.id)); });
      var play=card.querySelector("button");
      if(play) play.addEventListener("click", function(e){ e.stopPropagation(); go("replay.html?id="+esc(t.id)); });
      grid.appendChild(card);
    });
  }

  function buildPlayer(frames){
    function normSrc(s){ if(!s) return ""; if(/^(data:|https?:|file:|\/)/.test(s)) return s; return "data:image/webp;base64,"+s; }
    var imgArea=$("main [data-alt][style*='background-image']")||$("main [style*='background-image']");
    if(!imgArea){ imgArea=$("main img"); }
    if(!frames||!frames.length){ if(imgArea){ imgArea.style.backgroundImage="none"; } var hint=document.createElement("div"); hint.style.cssText="position:absolute;inset:0;display:flex;align-items:center;justify-content:center;color:#c2c6d6;font:14px Inter"; hint.textContent="No keyframes for this recording."; if(imgArea&&imgArea.parentElement){ imgArea.parentElement.style.position="relative"; imgArea.parentElement.appendChild(hint);} return; }
    var idx=0, playing=true, timer=null;
    function setFrame(i){ idx=((i%frames.length)+frames.length)%frames.length; var f=frames[idx]; var src=normSrc(f.src||f.data||f.uri); if(imgArea){ if(imgArea.tagName==="IMG"){ imgArea.src=src; } else { imgArea.style.backgroundImage="url('"+src+"')"; imgArea.style.backgroundSize="contain"; imgArea.style.backgroundRepeat="no-repeat"; imgArea.style.backgroundPosition="center"; } } if(range) range.value=String(idx); if(counter) counter.textContent=(idx+1)+" / "+frames.length; if(label) label.textContent=(f.label||""); }
    function delayFor(i){ var a=frames[i], b=frames[(i+1)%frames.length]; var ta=Number(a&&(a.tMs!=null?a.tMs:a.t_ms))||0; var tb=Number(b&&(b.tMs!=null?b.tMs:b.t_ms))||0; var d=tb-ta; return (d>30&&d<8000)?d:600; }
    function stop(){ playing=false; if(timer){ clearTimeout(timer); timer=null; } if(ppIcon) ppIcon.textContent="play_arrow"; }
    function play(){ playing=true; if(ppIcon) ppIcon.textContent="pause"; step(); }
    function step(){ if(!playing) return; timer=setTimeout(function(){ setFrame(idx+1); step(); }, delayFor(idx)); }
    // transport bar
    var bar=document.createElement("div");
    bar.style.cssText="display:flex;align-items:center;gap:12px;padding:10px 14px;background:#191b23;border-top:1px solid rgba(255,255,255,.08)";
    var pp=document.createElement("button"); pp.style.cssText="width:36px;height:36px;border-radius:999px;background:#4d8eff;color:#00285d;border:none;cursor:pointer;display:flex;align-items:center;justify-content:center";
    var ppIcon=document.createElement("span"); ppIcon.className="material-symbols-outlined"; ppIcon.textContent="pause"; ppIcon.style.fontVariationSettings="'FILL' 1"; pp.appendChild(ppIcon);
    var range=document.createElement("input"); range.type="range"; range.min="0"; range.max=String(frames.length-1); range.value="0"; range.style.cssText="flex:1;accent-color:#adc6ff";
    var counter=document.createElement("span"); counter.style.cssText="font:12px 'JetBrains Mono',monospace;color:#c2c6d6;min-width:64px;text-align:right";
    var label=document.createElement("span"); label.style.cssText="font:12px Inter;color:#e1e2ec;min-width:120px";
    pp.addEventListener("click", function(){ if(playing) stop(); else play(); });
    range.addEventListener("input", function(){ stop(); setFrame(Number(range.value)); });
    bar.appendChild(pp); bar.appendChild(label); bar.appendChild(range); bar.appendChild(counter);
    var host=(imgArea && imgArea.parentElement) ? imgArea.parentElement : ($("main")||document.body);
    if(host && host.parentElement){ host.parentElement.insertBefore(bar, host.nextSibling); } else if(host){ host.appendChild(bar); }
    setFrame(0); play();
  }

  async function pgTaskDetail(){
    wireNav();
    var id=qs("id");
    if(!id){ try{ var ts=await inv("list_tasks")||[]; if(ts.length) id=ts[0].id; }catch(e){} }
    if(!id){ go("library.html"); return; }
    var meta={};
    try{ var d=await inv("get_task", { id: id }); meta=(d&&d.meta)?d.meta:(d||{}); }catch(e){ toast("Load failed: "+e, "error"); }
    // bind name into the prominent main heading; allow rename via dblclick
    var head=$("main h1")||$("main h2")||$('main [class*="headline"]');
    if(head && meta.name){ head.textContent=meta.name; head.title="Double-click to rename"; head.addEventListener("dblclick", async function(){ var nn=prompt("Rename recording", meta.name); if(nn && nn!==meta.name){ try{ await inv("rename_task", { id:id, name:nn }); head.textContent=nn; meta.name=nn; toast("Renamed", "ok"); }catch(e){ toast("Rename failed: "+e, "error"); } } }); }
    // hide any editing controls (no editing allowed)
    $all("button").forEach(function(b){ var t=txt(b).toLowerCase(); if(/\btrim\b|\bcut\b|delete step|edit step|split/.test(t)) b.style.display="none"; });
    // player
    var pb=null;
    try{ pb=await inv("get_task_playback", { id: id }); }catch(e){ console.warn("playback", e); }
    buildPlayer(pb && pb.frames ? pb.frames : []);
    // actions
    var runB=byText("button","run")||byText("button","replay")||symBtn("play_arrow");
    if(runB) runB.addEventListener("click", function(){ go("replay.html?id="+esc(id)); });
    var expB=byText("button","export")||symBtn("output");
    if(expB) expB.addEventListener("click", function(){ go("export.html?id="+esc(id)); });
    var delB=byText("button","delete")||symBtn("delete");
    if(delB) delB.addEventListener("click", async function(){ if(!confirm("Delete this recording? This cannot be undone.")) return; try{ await inv("delete_task", { id: id }); go("library.html"); }catch(e){ toast("Delete failed: "+e, "error"); } });
  }

  async function pgReplay(){
    wireNav();
    var id=qs("id");
    if(!id){ try{ var ts=await inv("list_tasks")||[]; if(ts.length) id=ts[0].id; }catch(e){} }
    if(!id){ go("library.html"); return; }
    var statusEl=$('main [class*="headline"]')||$("main h1")||$("main h2");
    var bar=$('[role="progressbar"]')||$("main [class*='bg-primary'][class*='h-']")||$("progress");
    function setStatus(s){ if(statusEl) statusEl.textContent=s; }
    function setProg(i,t){ if(bar && t){ var pct=Math.round(100*i/t); if(bar.tagName==="PROGRESS"){ bar.max=t; bar.value=i; } else { bar.style.width=pct+"%"; } } setStatus("Replaying step "+i+" / "+t); }
    listen("replay-countdown", function(e){ var n=e&&e.payload!=null?e.payload:0; setStatus(Number(n)>0?("Starting in "+n+"…"):"Running…"); });
    listen("replay-progress", function(e){ var p=e&&e.payload?e.payload:{}; setProg(p.index||0, p.total||0); });
    listen("replay-finished", function(){ setStatus("Replay complete ✓"); toast("Replay finished", "ok"); });
    var cancel=byText("button","cancel")||byText("button","back")||byText("button","stop");
    if(cancel) cancel.addEventListener("click", function(){ go("library.html"); });
    try{ await inv("run_task", { id: id }); }catch(e){ toast("Replay failed: "+e, "error"); }
  }

  async function pgExport(){
    wireNav();
    var id=qs("id");
    if(!id){ try{ var ts=await inv("list_tasks")||[]; if(ts.length) id=ts[0].id; }catch(e){} }
    try{ var st=await inv("get_telegram_status"); var tEl=byText("span","slack")||byText("div","slack"); if(tEl && st){ /* leave design; status known */ } }catch(e){}
    function fmt(){ var r=$all('input[type="radio"]').find(function(x){ return x.checked; }); if(r){ var l=(r.closest("label")?txt(r.closest("label")):r.value||"").toLowerCase(); if(l.indexOf("jsonl")>=0) return "jsonl"; if(l.indexOf("json")>=0) return "json"; } var sel=$('.selected,[aria-selected="true"],[data-selected="true"]'); if(sel){ var t=txt(sel).toLowerCase(); if(t.indexOf("jsonl")>=0) return "jsonl"; if(t.indexOf("json")>=0) return "json"; } return "json"; }
    async function payload(f){ return f==="jsonl"? inv("export_task_jsonl", { id: id }) : inv("export_task_json", { id: id }); }
    function download(name, text, mime){ var blob=new Blob([text], { type: mime }); var a=document.createElement("a"); a.href=URL.createObjectURL(blob); a.download=name; document.body.appendChild(a); a.click(); setTimeout(function(){ URL.revokeObjectURL(a.href); a.remove(); }, 1000); }
    async function doExport(){ if(!id){ toast("No task selected", "error"); return; } var f=fmt(); try{ var s=await payload(f); if(s==null){ toast("Export unavailable", "error"); return; } download((id)+(f==="jsonl"?".trajectory.jsonl":".sai.json"), s, f==="jsonl"?"application/x-ndjson":"application/json"); toast("Exported "+f.toUpperCase(), "ok"); }catch(e){ toast("Export failed: "+e, "error"); } }
    var dl=byText("button","download")||byText("button","execute export");
    if(dl) dl.addEventListener("click", doExport);
    var ex=byText("button","execute export");
    if(ex && ex!==dl) ex.addEventListener("click", doExport);
    var cp=byText("button","copy");
    if(cp) cp.addEventListener("click", async function(){ if(!id) return; var f=fmt(); try{ var s=await payload(f); await navigator.clipboard.writeText(s||""); toast("Copied "+f.toUpperCase(), "ok"); }catch(e){ toast("Copy failed: "+e, "error"); } });
    [byText("button","slack"), byText("button","agent"), byText("button","push")].forEach(function(b){ if(b) b.addEventListener("click", function(){ toast("Not configured in this build", "info"); }); });
  }

  async function pgSettings(){
    wireNav();
    try{ await inv("get_telegram_status"); }catch(e){}
    // persist recording option controls
    var saved={}; try{ saved=JSON.parse(localStorage.getItem("recordOpts")||"{}"); }catch(e){}
    function keyOf(el){ return (el.name||el.id||"").toLowerCase(); }
    function mapKey(k){ if(/fps|frame/.test(k)) return "fps"; if(/quality|qual/.test(k)) return "quality"; if(/history|buffer|pre/.test(k)) return "history_secs"; if(/max.?dim|resolution|size/.test(k)) return "max_dim"; if(/lossy|compress/.test(k)) return "lossy"; if(/crop/.test(k)) return "crop"; return null; }
    $all("input,select").forEach(function(el){ var mk=mapKey(keyOf(el)); if(!mk) return;
      if(saved[mk]!=null){ if(el.type==="checkbox") el.checked=!!saved[mk]; else el.value=String(saved[mk]); }
      el.addEventListener("change", function(){ var v=(el.type==="checkbox")?el.checked:(isNaN(Number(el.value))?el.value:Number(el.value)); saved[mk]=v; localStorage.setItem("recordOpts", JSON.stringify(saved)); toast("Saved", "ok"); });
    });
  }

  function pgStates(){
    wireNav();
    var f=byText("button","first recording")||byText("button","start");
    if(f) f.addEventListener("click", function(){ go("setup.html"); });
    var r=byText("button","retry");
    if(r) r.addEventListener("click", function(){ location.reload(); });
  }

  function init(){
    try{
      switch(page){
        case "": case "index.html": pgOnboarding(); break;
        case "dashboard.html": pgDashboard(); break;
        case "setup.html": pgSetup(); break;
        case "float.html": pgFloat(); break;
        case "recording.html": pgRecording(); break;
        case "task-detail.html": pgTaskDetail(); break;
        case "library.html": pgLibrary(); break;
        case "replay.html": pgReplay(); break;
        case "export.html": pgExport(); break;
        case "compression.html": pgCompression(); break;
        case "settings.html": pgSettings(); break;
        case "states.html": pgStates(); break;
        default: wireNav(); wireStartButtons();
      }
    }catch(e){ console.error("[app.js init]", e); }
  }
  if(document.readyState==="loading") document.addEventListener("DOMContentLoaded", init); else init();
})();
