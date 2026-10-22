let meta = #{ name: "resume-from", description: "a second prompt re-targets the first agent's session" };
let first = agent("draft the summary");
let second = agent("shorten it", #{ resume_from: first.agent_id });
complete(#{ first: first.output, second: second.output, spent: budget().spent });
