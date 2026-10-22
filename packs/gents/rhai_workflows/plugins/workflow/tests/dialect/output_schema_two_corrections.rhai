let meta = #{ name: "output-schema-two-corrections", description: "two bad replies are corrected, the third is accepted" };
let r = agent("report the count", #{
    output_schema: #{ type: "object", required: ["n"], properties: #{ n: #{ type: "integer" } } },
});
complete(#{ success: r.success, output: r.output });
