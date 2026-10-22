let meta = #{
    name: "meta-full",
    description: "every optional meta key, with phases and log lines",
    when_to_use: "to check that optional keys are read",
    phases: [#{ title: "look", detail: "read the input" }, #{ title: "report" }],
};
phase("look");
log("first event");
phase("report");
log("second event");
phase("look");
complete("reported");
