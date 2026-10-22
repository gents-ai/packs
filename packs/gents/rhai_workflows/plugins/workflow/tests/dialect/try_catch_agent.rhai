let meta = #{ name: "try-catch-agent", description: "a refused agent call is caught, a good one is used" };
let note = "none";
try {
    agent("bad option", #{ label: 7 });
} catch (e) {
    note = "caught";
}
let ok = agent("good call");
complete(#{ note: note, ok: ok.output });
