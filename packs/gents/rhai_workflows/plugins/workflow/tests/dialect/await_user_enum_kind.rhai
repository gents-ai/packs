let meta = #{ name: "await-user-enum-kind", description: "await_user with a kind the pause enum knows" };
phase("ask");
let answer = await_user("user", "which branch should be reviewed?");
complete(#{ answer: answer });
