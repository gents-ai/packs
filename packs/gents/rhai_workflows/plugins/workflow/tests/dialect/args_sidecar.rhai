let meta = #{ name: "args-sidecar", description: "reads fields of the args beside this case" };
let names = args.names;
let out = [];
for n in names { out.push(n.to_upper()); }
complete(#{ count: args.count, names: out, nested: args.nested.depth });
