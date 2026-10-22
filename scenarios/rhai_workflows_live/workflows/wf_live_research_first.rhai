let meta = #{
    name: "wf-live-research-first",
    description: "Spawns a wf-research agent first, a target outside the dynamic ceiling",
    when_to_use: "Row 13: run from an untrusted mount under dynamic check; the first spawn is refused",
};

let r = agent("Reply with the word research.", #{ label: "research", agent_type: "wf-research" });
complete(#{ output: r.output });
