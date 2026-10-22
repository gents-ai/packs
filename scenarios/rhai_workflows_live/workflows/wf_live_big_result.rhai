let meta = #{
    name: "wf-live-big-result",
    description: "Completes with a value of about 64 KiB, far over the completion message cap",
    when_to_use: "Row 19: the message is capped at 4,000 B and workflow result returns the whole value",
};

// One agent first: a run that ends in its first step has no job row to read the result from.
agent("Reply with the word go.", #{ label: "go", agent_type: "wf-live-worker" });
let line = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
let out = "";
for i in 0..1000 {
    out += line + "\n";
}
complete(#{ size: out.len(), body: out });
