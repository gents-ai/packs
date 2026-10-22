let meta = #{ name: "shadowing-refusal", description: "a script fn may not take a host function's name" };
fn agent(prompt) { prompt }
complete(agent("hello"));
