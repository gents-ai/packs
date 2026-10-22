let meta = #{ name: "string-operations", description: "common string operations" };
let t = "  Hello, Workflow  ";
t.trim();
let parts = t.split(", ");
let replaced = t;
replaced.replace("Hello", "Bye");
complete(#{
    trimmed: t,
    upper: t.to_upper(),
    lower: t.to_lower(),
    len: t.len(),
    parts: parts,
    joined: parts[1] + "!" + parts[0],
    sub: t.sub_string(0, 5),
    has: t.contains("Work"),
    replaced: replaced,
    interpolated: `n=${1 + 2}`,
});
