let meta = #{ name: "sorted-map-iteration", description: "map keys and values come back in key order" };
let m = #{ zebra: 1, apple: 2, mango: 3 };
m.banana = 4;
let order = [];
for k in m.keys() { order.push(k); }
let values = [];
for v in m.values() { values.push(v); }
complete(#{ order: order, values: values, encoded: json_encode(m) });
