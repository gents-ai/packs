let meta = #{
    name: "wf-live-pause",
    description: "Two agents in sequence, then their results",
    when_to_use: "Row 5: workflow pause while it runs, then workflow resume replays the finished agent from the journal",
};

let r = agent("Reply with the word before.", #{ label: "before-pause", agent_type: "wf-live-worker" });
let s = agent("Reply with the word after.", #{ label: "after-pause", agent_type: "wf-live-worker" });
complete(#{ before: r.output, after: s.output });
