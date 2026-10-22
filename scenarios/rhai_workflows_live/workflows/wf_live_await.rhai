let meta = #{
    name: "wf-live-await",
    description: "Asks the operator one question and completes with the answer",
    when_to_use: "Row 6: workflow answer delivers the text once; a second question would suspend again",
};

let answer = await_user("user", "Which colour should the run report?");
complete(#{ answer: answer });
