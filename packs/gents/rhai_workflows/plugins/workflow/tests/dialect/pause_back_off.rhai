let meta = #{ name: "pause-back-off", description: "a pause of kind back_off after a catch" };
let hit = false;
try { agent("probe", #{ phase: 3 }); } catch (e) { hit = true; }
if hit { pause("back_off", "the probe was refused, back off"); }
complete("unreached");
