let meta = #{ name: "maps-and-arrays", description: "map and array building and reading" };
let m = #{ a: 1, b: 2 };
m.c = m.a + m.b;
m["d"] = 10;
let keys = m.keys();
keys.sort();
let xs = [5, 3, 9, 1];
xs.sort();
xs.push(7);
let total = 0;
for x in xs { total += x; }
complete(#{ keys: keys, xs: xs, total: total, first: xs[0], last: xs[xs.len() - 1], has: "c" in m });
