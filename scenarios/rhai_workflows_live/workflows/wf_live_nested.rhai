let meta = #{
    name: "wf-live-nested",
    description: "One worker reads notes.txt and does what it says, then completes with its reply",
    when_to_use: "Row 11: the notes ask the worker to run a workflow, which a job child may not do",
};

let r = agent("Read the file notes.txt in the working folder, do what it says (if it names a tool call, make that call), and reply with what you did and what the tool returned.", #{
    label: "read-notes",
    agent_type: "wf-live-worker",
});
complete(#{ success: r.success, reply: r.output });
