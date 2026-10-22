let meta = #{ name: "fingerprint-json-encode", description: "json_encode sorts keys, fingerprint is stable" };
let a = json_encode(#{ b: 2, a: 1, c: [1, 2, #{ z: 1, y: 2 }] });
let b = json_encode(#{ c: [1, 2, #{ y: 2, z: 1 }], a: 1, b: 2 });
complete(#{
    same: a == b,
    encoded: a,
    fp: fingerprint(a),
    fp_same: fingerprint(a) == fingerprint(b),
    empty: fingerprint(""),
    width: fingerprint("x").len(),
});
