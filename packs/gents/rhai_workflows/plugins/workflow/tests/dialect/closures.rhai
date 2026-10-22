let meta = #{ name: "closures", description: "closures capturing values, and function pointers" };
let base = 10;
let add = |x| x + base;
let twice = |f, x| f.call(f.call(x));
let evens = [1, 2, 3, 4, 5, 6].filter(|n| n % 2 == 0);
let doubled = evens.map(|n| n * 2);
fn square(x) { x * x }
complete(#{ added: add.call(5), twice: twice.call(add, 1), doubled: doubled, sq: square(7) });
