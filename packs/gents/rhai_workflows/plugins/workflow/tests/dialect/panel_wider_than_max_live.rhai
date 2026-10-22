let meta = #{ name: "panel-wider-than-max-live", description: "a panel of 66 under max_live 64 is admitted in windows" };
let items = [];
for i in 0..66 { items.push(#{ prompt: "check " + i, label: "item-" + i }); }
let panel = parallel(items);
let ok = 0;
for p in panel { if p != () && p.success { ok += 1; } }
complete(#{ count: panel.len(), ok: ok });
