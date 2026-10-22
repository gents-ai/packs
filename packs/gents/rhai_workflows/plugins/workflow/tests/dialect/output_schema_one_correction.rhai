let meta = #{ name: "output-schema-one-correction", description: "a reply without a fence is corrected once, then accepted" };
let r = agent("report the count", #{
    output_schema: #{ type: "object", required: ["n"], properties: #{ n: #{ type: "integer" } } },
});
complete(#{ success: r.success, output: r.output });
