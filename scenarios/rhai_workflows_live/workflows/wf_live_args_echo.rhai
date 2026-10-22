let meta = #{
    name: "wf-live-args-echo",
    description: "Pauses unless the budget is at least 2, then asks a question, then completes with its original args and the answer",
    when_to_use: "Row 20: resume and answer by key keep the handle and chain root, and the result reflects the original args",
};

let r = agent("Reply with the word start.", #{ label: "start", agent_type: "wf-live-worker" });
if budget().total < 2 {
    pause("user", "Holding before the question. Next: workflow resume --budget 2");
}
let answer = await_user("user", "Which word should the run echo back?");
complete(#{ args: args, started: r.output, answer: answer });
