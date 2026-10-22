let meta = #{
    name: "wf-live-long-panel",
    description: "A six-slot panel of slow read tasks that is still running when it is stopped or restarted",
    when_to_use: "Rows 7 and 8: stop with live children, and restart the runtime mid-panel",
};

phase("long panel");
let panel = [];
for i in 0..6 {
    panel.push(#{
        prompt: "Read notes.txt five separate times, one read_file call each, restating it in your own words after each read, and finish with the number " + i + ".",
        label: "slot-" + i,
        phase: "long panel",
        agent_type: "wf-live-worker",
    });
}
let found = parallel(panel);
complete(found.map(|r| if r == () { "failed" } else { r.output }));
