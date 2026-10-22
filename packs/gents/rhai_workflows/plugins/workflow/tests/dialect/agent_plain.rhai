let meta = #{ name: "agent-plain", description: "sequential agents, the second reads the first, the third has options" };
let first = agent("list the files");
let second = agent("summarize: " + first.output);
phase("inspect");
let third = agent("count the modules", #{
    label: "counter",
    phase: "inspect",
    output_schema: #{ type: "object", required: ["n"], properties: #{ n: #{ type: "integer" } } },
});
complete(#{ first: first.output, second: second.output, kind: type_of(third.output) });
