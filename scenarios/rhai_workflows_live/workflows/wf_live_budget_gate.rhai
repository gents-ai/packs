let meta = #{
    name: "wf-live-budget-gate",
    description: "Pauses before its first agent unless the budget covers three agents",
    when_to_use: "Row 21: under a low agent_budget the run pauses with budget_exceeded before any child starts",
};

if budget().total < 3 {
    pause("budget_exceeded", "Needs an agent budget of at least 3; resume the run with --budget 3");
}
let found = parallel([
    #{ prompt: "Reply with the word a.", label: "gate-a", agent_type: "wf-live-worker" },
    #{ prompt: "Reply with the word b.", label: "gate-b", agent_type: "wf-live-worker" },
    #{ prompt: "Reply with the word c.", label: "gate-c", agent_type: "wf-live-worker" },
]);
complete(found.map(|r| if r == () { "failed" } else { r.output }));
