let meta = #{ name: "switch-statement", description: "switch over values, alternatives, ranges and a default" };
fn describe(x) {
    switch x {
        1 => "one",
        2 | 3 => "few",
        4..=6 => "some",
        "s" => "text",
        _ => "other",
    }
}
let out = [];
for x in [1, 3, 5, 9] { out.push(describe(x)); }
out.push(describe("s"));
out.push(describe("t"));
complete(out);
