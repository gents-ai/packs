let meta = #{
    name: "wf-live-budget-panel",
    description: "One agent, then a three-agent panel that a budget of 2 cannot admit",
    when_to_use: "Row 3: run with agent_budget 2; the panel admits none and the run pauses",
};

let first = agent("Reply with the word start.", #{ label: "first", agent_type: "wf-live-worker" });
let panel = parallel([
    #{ prompt: "Reply with the word a.", label: "panel-a", agent_type: "wf-live-worker" },
    #{ prompt: "Reply with the word b.", label: "panel-b", agent_type: "wf-live-worker" },
    #{ prompt: "Reply with the word c.", label: "panel-c", agent_type: "wf-live-worker" },
]);
complete(#{ first: first.output, panel: panel.map(|r| if r == () { "failed" } else { r.output }) });
