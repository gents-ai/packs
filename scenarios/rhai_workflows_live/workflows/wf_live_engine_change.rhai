let meta = #{
    name: "wf-live-engine-change",
    description: "One agent, a user pause to swap the engine unless the budget is at least 3, then a second agent",
    when_to_use: "Row 9: install a rebuilt engine during the pause, run with agent_budget 2, then resume --budget 3 with and without --engine current",
};

let first = agent("Reply with the word one.", #{ label: "before-swap", agent_type: "wf-live-worker" });
if budget().total < 3 {
    pause("user", "Holding for the engine swap. Next: workflow resume --budget 3");
}
let second = agent("Reply with the word two.", #{ label: "after-swap", agent_type: "wf-live-worker" });
complete(#{ first: first.output, second: second.output });
