let meta = #{ name: "missing-map-keys", description: "reading a key a map lacks gives the unit value" };
let m = #{ a: 1 };
complete(#{
    prop: type_of(m.nope),
    index: type_of(m["nope"]),
    is_unit: m.nope == (),
    has: "nope" in m,
    fallback: if m.nope == () { "default" } else { m.nope },
});
