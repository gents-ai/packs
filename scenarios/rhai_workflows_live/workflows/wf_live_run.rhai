let meta = #{
    name: "wf-live-run",
    description: "Reads notes.txt with one worker agent and completes with its first line",
    when_to_use: "Row 1: the smallest trusted workflow, one agent step and a result",
};

let r = agent("Read the file notes.txt in the working folder and reply with its first line, nothing else.", #{
    label: "read-notes",
    agent_type: "wf-live-worker",
});
complete(#{ success: r.success, first_line: r.output });
