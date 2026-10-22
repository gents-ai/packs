let meta = #{ name: "pause-user", description: "a pause of kind user" };
agent("prepare");
pause("user", "wait for the operator to look");
complete("unreached");
