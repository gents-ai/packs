let meta = #{ name: "unicode-and-string-building", description: "multi-byte text, and strings built up piece by piece" };
let word = "héllo wörld ✓ 日本語";
let built = "";
for i in 0..4 { built += `${i}:`; }
built += word.sub_string(0, 5);
let chars = [];
for c in "añ日" { chars.push(c); }
complete(#{
    len: word.len(),
    upper: word.to_upper(),
    first: word.sub_string(0, 5),
    tail: word.sub_string(14, 3),
    built: built,
    chars: chars,
    has: word.contains("✓"),
});
