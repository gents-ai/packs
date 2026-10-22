let meta = #{
    name: "wf-live-scratch-report",
    description: "Summarises notes.txt, writes report.md to the run's scratch directory and returns its path",
    when_to_use: "Row 16: the path in the result is the file the model then reads",
};

let r = agent("Read notes.txt and reply with a one-sentence summary.", #{ label: "summary", agent_type: "wf-live-worker" });
let report = render_template("report", #{
    title: "Notes report",
    summary: r.output,
    findings: "- notes.txt was read by one worker",
    next_steps: "- none",
});
let path = write_scratch_file("report.md", report);
complete(#{ report: path });
