let meta = #{
    name: "wf-live-narrowing",
    description: "Two panels of three workers; the operator narrows the job block between them",
    when_to_use: "Row 10: a lowered limit or removed target at the second panel is refused, and no child is left running",
};

phase("first panel");
let one = parallel([
    #{ prompt: "Reply with the word a.", label: "one-a", phase: "first panel", agent_type: "wf-live-worker" },
    #{ prompt: "Reply with the word b.", label: "one-b", phase: "first panel", agent_type: "wf-live-worker" },
    #{ prompt: "Reply with the word c.", label: "one-c", phase: "first panel", agent_type: "wf-live-worker" },
]);
phase("second panel");
try {
    let two = parallel([
        #{ prompt: "Reply with the word d.", label: "two-d", phase: "second panel", agent_type: "wf-live-worker" },
        #{ prompt: "Reply with the word e.", label: "two-e", phase: "second panel", agent_type: "wf-live-worker" },
        #{ prompt: "Reply with the word f.", label: "two-f", phase: "second panel", agent_type: "wf-live-worker" },
    ]);
    complete(#{ one: one.len(), two: two.len() });
} catch (e) {
    complete(#{ refused: e });
}
