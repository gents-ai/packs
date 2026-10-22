let meta = #{ name: "budget-exceeded-mid-panel", description: "a panel one past the remaining budget pauses the run" };
let first = agent("warm up");
let items = [];
for i in 0..budget().remaining + 1 { items.push(#{ prompt: "wide " + i }); }
let panel = parallel(items);
complete("unreached");
