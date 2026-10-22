let meta = #{ name: "await-user-free-kind", description: "await_user with a kind of the script's own" };
let answer = await_user("confirm-delete", "delete the draft?");
complete(#{ answer: answer });
