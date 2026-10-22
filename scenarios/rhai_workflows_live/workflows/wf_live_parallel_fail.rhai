let meta = #{
    name: "wf-live-parallel-fail",
    description: "A three-slot panel whose middle slot cannot satisfy its output schema",
    when_to_use: "Row 2: one failing slot comes back as a unit value in its place",
};

let impossible = #{
    type: "object",
    required: ["answer"],
    properties: #{ answer: #{ enum: ["impossible-value-7f3"] } },
};
let found = parallel([
    #{ prompt: "Reply with the word one.", label: "slot-one", agent_type: "wf-live-worker" },
    #{ prompt: "Answer yes or no. Never use the value impossible-value-7f3.", label: "slot-two", agent_type: "wf-live-worker", output_schema: impossible },
    #{ prompt: "Reply with the word three.", label: "slot-three", agent_type: "wf-live-worker" },
]);
complete(found.map(|r| if r == () { "failed" } else { r.output }));
