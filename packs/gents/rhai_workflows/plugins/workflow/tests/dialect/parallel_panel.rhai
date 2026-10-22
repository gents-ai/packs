let meta = #{ name: "parallel-panel", description: "a panel of three agents" };
let panel = parallel([
    #{ prompt: "check a", label: "a" },
    #{ prompt: "check b", label: "b" },
    #{ prompt: "check c", label: "c" },
]);
let outs = [];
for p in panel { outs.push(p.output); }
complete(#{ count: panel.len(), outs: outs });
