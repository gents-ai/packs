let meta = #{
    name: "wf-live-scratch-complete",
    description: "Writes a scratch file and completes in the same step right after one agent",
    when_to_use: "Row 18: the scratch write and the completion land in one terminal output",
};

let r = agent("Reply with the word done.", #{ label: "only", agent_type: "wf-live-worker" });
let path = write_scratch_file("scratch.txt", "agent said: " + r.output + "\n");
complete(#{ scratch: path, agent: r.output });
