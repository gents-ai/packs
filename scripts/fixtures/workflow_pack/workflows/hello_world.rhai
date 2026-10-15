let meta = #{
    name: "hello-world",
    description: "Greets the name in args.",
    when_to_use: "Fixture for the workflow pack tooling."
};

complete(#{ greeting: "hello, " + args.name });
