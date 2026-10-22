let meta = #{ name: "budget-reads", description: "budget() before and after an agent" };
let before = budget();
let r = agent("spend one");
let after = budget();
complete(#{ before: before.spent, after: after.spent, total: after.total, left: after.remaining });
