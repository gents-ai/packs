let meta = #{ name: "pause-budget-exceeded", description: "a pause of kind budget_exceeded" };
if budget().remaining >= 0 { pause("budget_exceeded", "the plan needs more budget"); }
complete("unreached");
