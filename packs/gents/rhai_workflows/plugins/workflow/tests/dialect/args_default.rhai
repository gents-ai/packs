let meta = #{ name: "args-default", description: "falls back when no args are passed" };
let count = if args == () { 3 } else { args.count };
complete(#{ count: count });
