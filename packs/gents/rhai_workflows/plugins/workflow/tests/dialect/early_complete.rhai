let meta = #{ name: "early-complete", description: "complete ends the run, later code never runs" };
let r = agent("only call");
if r.output == "ok" { complete(#{ early: true }); }
agent("never reached");
complete(#{ early: false });
