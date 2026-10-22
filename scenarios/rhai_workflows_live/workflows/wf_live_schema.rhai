let meta = #{
    name: "wf-live-schema",
    description: "One agent whose answer must match an output schema",
    when_to_use: "Row 4: the agent may answer in prose first; the engine corrects it once",
};

let schema = #{
    type: "object",
    required: ["colour", "words"],
    properties: #{ colour: #{ type: "string" }, words: #{ type: "integer" } },
};
let r = agent("Read notes.txt. Reply with a JSON object: colour is the colour named on its last line, words is the number of lines in the file.", #{
    label: "schema",
    agent_type: "wf-live-worker",
    output_schema: schema,
});
complete(#{ success: r.success, output: r.output });
